// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The snooze submenu itself, shared by the two window extensions: the merge
//! into a GtkUIManager, the sensitivity that follows the selection and the
//! account, and the activation that snoozes what is selected.
//!
//! The gate is the module's usual two levels, with the second one cached
//! module-wide ([`crate::session_cache::shared`]): the folder's provider
//! protocol answers synchronously on every `update-actions`, and the server's
//! snooze capability is fetched once per account off the main loop, every
//! reader window sharing the answer. Until it is known the submenu sits
//! insensitive under a "checking" tooltip; on a server without the extension
//! it stays that way and the tooltip says why.

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::ptr;

use eds_sys::{
    CamelFolder, ESource, camel_folder_get_parent_store, camel_service_get_provider,
    camel_service_get_uid, e_source_get_parent, e_source_registry_ref_source,
};
use evo_sys::{
    EMailReader, EShellContent, GTK_BUTTONS_CLOSE, GTK_MESSAGE_ERROR, GtkAction, GtkActionGroup,
    GtkUIManager, e_mail_reader_get_selected_uids, e_mail_reader_ref_folder, e_shell_get_default,
    e_shell_get_registry, gtk_action_group_add_action, gtk_action_new, gtk_action_set_sensitive,
    gtk_dialog_run, gtk_message_dialog_new, gtk_ui_manager_add_ui_from_string,
    gtk_ui_manager_ensure_update, gtk_widget_destroy,
};
use glib_sys::{GError, GFALSE, GTRUE, g_error_free, gpointer};
use gobject_sys::{
    GObject, g_object_get, g_object_get_data, g_object_set, g_object_set_data_full, g_object_unref,
    g_signal_connect_data,
};
use jmap_backend_core::i18n::{N_, translate, translate_static, translate_with};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::trampoline::guard;
use jmap_proto::Id;
use jmap_proto::mail::SnoozeDetails;

use crate::dispatch;
use crate::link::{self, AccountLink};
use crate::send_later::schedule::{self, Preset};
use crate::session_cache;

/// See `vacation::extension` on why this literal is not imported.
const BACKEND_NAME: &CStr = c"jmap";

const STATE_KEY: &CStr = c"jmap-snooze-state";

const NOT_JMAP: &CStr = N_(c"The folder does not belong to a JMAP account");
const CHECKING: &CStr = N_(c"Checking whether the account’s server offers snooze…");
const NOT_OFFERED: &CStr = N_(c"The account’s server does not offer snooze");
// TRANSLATORS: %1$s is the reason the mail server gave.
const NOT_REACHED: &CStr = N_(c"The account’s server could not be asked: %1$s");
const OFFERED: &CStr = N_(c"The server hides the message until then and wakes it in the inbox");
// TRANSLATORS: %1$s is the reason the mail server gave.
const SNOOZE_FAILED: &CStr = N_(c"The message could not be snoozed: %1$s");
const NO_CLOCK: &CStr = N_(c"The local calendar could not name the chosen time");

/// One reader window's snooze state, boxed as qdata on the extensible.
struct SnoozeState {
    /// The reader whose selection and folder the actions read: the mail view
    /// (shell case) or the browser itself — a child, or the object itself,
    /// alive as long as the owner is.
    reader: *mut EMailReader,
    /// The submenu's action, the gate's visible half.
    menu_action: *mut GtkAction,
    /// The service uid a capability fetch is in flight for, so one slow
    /// account is asked once and not per `update-actions`.
    pending: Option<String>,
}

/// The owner's [`SnoozeState`], while it is alive.
///
/// # Safety
///
/// `owner` must be a live extensible [`install`] decorated; the reference must
/// not outlive one main-loop callback.
unsafe fn state<'a>(owner: *mut GObject) -> Option<&'a RefCell<SnoozeState>> {
    // SAFETY: set once in `install`; the cast is the one it stored.
    unsafe {
        g_object_get_data(owner, STATE_KEY.as_ptr())
            .cast::<RefCell<SnoozeState>>()
            .as_ref()
    }
}

unsafe extern "C" fn drop_state(data: gpointer) {
    // SAFETY: reclaiming the box `install` leaked, exactly once.
    drop(unsafe { Box::from_raw(data.cast::<RefCell<SnoozeState>>()) });
}

/// The three presets, sharing `send_later`'s moments on purpose: one set of
/// habits, two menus.
const PRESETS: &[(&CStr, &CStr, unsafe extern "C" fn(*mut GtkAction, gpointer))] = &[
    (c"jmap-snooze-hour", N_(c"For One _Hour"), activate_hour),
    (
        c"jmap-snooze-tomorrow",
        N_(c"Until _Tomorrow Morning"),
        activate_tomorrow,
    ),
    (
        c"jmap-snooze-monday",
        N_(c"Until Next _Monday Morning"),
        activate_monday,
    ),
];

/// Appended straight to the popup: 3.52's `/mail-message-popup` has no
/// third-party placeholder (the similarly named one belongs to the preview
/// pane's popup), so a direct child is the only seat at this table.
const UI: &CStr = c"<popup name='mail-message-popup'>\
<menu action='jmap-snooze-menu'>\
<menuitem action='jmap-snooze-hour'/>\
<menuitem action='jmap-snooze-tomorrow'/>\
<menuitem action='jmap-snooze-monday'/>\
</menu></popup>";

/// Merge the submenu into `ui_manager`/`action_group` and hang the state on
/// `owner`. Idempotent per owner: a second call finds the state and leaves.
///
/// # Safety
///
/// `owner` must be a live GObject outliving `reader`, `ui_manager` and
/// `action_group`, all live and belonging to the same window; main loop only.
pub(crate) unsafe fn install(
    owner: *mut GObject,
    reader: *mut EMailReader,
    ui_manager: *mut GtkUIManager,
    action_group: *mut GtkActionGroup,
) {
    if unsafe { state(owner) }.is_some() {
        return;
    }
    if reader.is_null() || ui_manager.is_null() || action_group.is_null() {
        return;
    }

    // SAFETY: live objects per this function's contract; each action's
    // reference passes to the group.
    let menu_action = unsafe {
        let menu_action = gtk_action_new(
            c"jmap-snooze-menu".as_ptr(),
            translate_static(N_(c"S_nooze")),
            translate_static(CHECKING),
            ptr::null(),
        );
        gtk_action_set_sensitive(menu_action, GFALSE);
        gtk_action_group_add_action(action_group, menu_action);

        for (name, label, handler) in PRESETS {
            let action = gtk_action_new(
                name.as_ptr(),
                translate_static(label),
                ptr::null(),
                ptr::null(),
            );
            g_signal_connect_data(
                action.cast(),
                c"activate".as_ptr(),
                Some(std::mem::transmute::<
                    unsafe extern "C" fn(*mut GtkAction, gpointer),
                    unsafe extern "C" fn(),
                >(*handler)),
                owner.cast(),
                None,
                0,
            );
            gtk_action_group_add_action(action_group, action);
            g_object_unref(action.cast());
        }

        let mut error: *mut GError = ptr::null_mut();
        gtk_ui_manager_add_ui_from_string(ui_manager, UI.as_ptr(), -1, &mut error);
        if !error.is_null() {
            tracing::error!(
                message = ?read_string((*error).message),
                "the snooze submenu could not be merged"
            );
            g_error_free(error);
        }
        gtk_ui_manager_ensure_update(ui_manager);
        menu_action
    };

    tracing::trace!("snooze submenu merged into a JMAP-capable reader window");
    let snooze = Box::new(RefCell::new(SnoozeState {
        reader,
        menu_action,
        pending: None,
    }));
    // SAFETY: `owner` is live; the box's ownership passes to the qdata.
    unsafe {
        g_object_set_data_full(
            owner,
            STATE_KEY.as_ptr(),
            Box::into_raw(snooze).cast(),
            Some(drop_state),
        );
        g_object_unref(menu_action.cast());
        update_sensitivity(owner);
    }
}

/// The `update-actions` half: sensitivity from the selection, the folder's
/// provider, and the cached server answer — kicking the fetch on a miss.
///
/// # Safety
///
/// `owner` must be a live installed extensible, on the main loop.
pub(crate) unsafe fn update_sensitivity(owner: *mut GObject) {
    let Some(cell) = (unsafe { state(owner) }) else {
        return;
    };
    let (reader, menu_action) = {
        let snooze = cell.borrow();
        (snooze.reader, snooze.menu_action)
    };

    // SAFETY: the reader is alive with its owner; the uid array and folder
    // are owned and released here.
    let (selected, service_uid) = unsafe {
        let uids = e_mail_reader_get_selected_uids(reader);
        let selected = if uids.is_null() { 0 } else { (*uids).len };
        if !uids.is_null() {
            glib_sys::g_ptr_array_unref(uids);
        }
        (selected, jmap_service_uid(reader))
    };

    let verdict = match (&service_uid, selected) {
        (None, _) | (_, 0) => Some((false, translate(NOT_JMAP))),
        (Some(uid), _) => match session_cache::shared().lookup(uid, std::time::Instant::now()) {
            Some(features) if features.snooze => Some((true, translate(OFFERED))),
            Some(_) => Some((false, translate(NOT_OFFERED))),
            None => None,
        },
    };

    match verdict {
        Some((sensitive, tooltip)) => {
            // SAFETY: the action lives with the owner's window.
            unsafe {
                gtk_action_set_sensitive(menu_action, if sensitive { GTRUE } else { GFALSE });
                set_tooltip(menu_action, tooltip);
            }
        }
        None => {
            let uid = service_uid.unwrap_or_default();
            {
                let mut snooze = cell.borrow_mut();
                if snooze.pending.as_deref() == Some(uid.as_str()) {
                    return; // this account is already being asked
                }
                snooze.pending = Some(uid.clone());
            }
            // SAFETY: as above.
            unsafe {
                gtk_action_set_sensitive(menu_action, GFALSE);
                set_tooltip(menu_action, translate(CHECKING));
            }
            // SAFETY: the reader is alive; the source reference is the
            // worker's, released there.
            let Some(source) = (unsafe { connect_source_of(reader) }) else {
                // Un-poison the gate: without this, one failed resolution
                // would block every later fetch for this account.
                tracing::debug!(%uid, "snooze gate: the account source could not be resolved");
                if let Some(cell) = unsafe { state(owner) } {
                    cell.borrow_mut().pending = None;
                }
                return;
            };
            let source = SendSource(source);
            let fetch_uid = uid.clone();
            let work = move || {
                let source = source;
                // SAFETY: the reference `connect_source_of` took is the
                // worker's for the whole closure.
                let outcome = unsafe { link::connect_account(source.0) };
                // SAFETY: releasing the worker's reference.
                unsafe { g_object_unref(source.0.cast()) };
                (fetch_uid, outcome)
            };
            let finish =
                |owner: *mut GObject, (uid, outcome): (String, Result<AccountLink, String>)| {
                    match &outcome {
                        Ok(link) => {
                            tracing::debug!(
                                snooze = link.features.snooze,
                                "snooze gate: capability fetched"
                            );
                            session_cache::shared().store(
                                &uid,
                                link.features.clone(),
                                std::time::Instant::now(),
                            );
                        }
                        Err(message) => {
                            tracing::debug!(%message, "snooze gate: the server could not be asked");
                        }
                    }
                    // SAFETY: `with_strong` hands back the live owner.
                    unsafe {
                        if let Some(cell) = state(owner) {
                            let mut snooze = cell.borrow_mut();
                            if snooze.pending.as_deref() == Some(uid.as_str()) {
                                snooze.pending = None;
                            }
                            if let Err(message) = outcome {
                                let menu_action = snooze.menu_action;
                                drop(snooze);
                                gtk_action_set_sensitive(menu_action, GFALSE);
                                set_tooltip(menu_action, translate_with(NOT_REACHED, &[&message]));
                                return;
                            }
                        }
                        update_sensitivity(owner);
                    }
                };
            // SAFETY: a live owner per this function's contract.
            unsafe { dispatch::spawn_for(owner, work, finish) };
        }
    }
}

/// The folder's Camel service uid, when the folder belongs to this provider.
///
/// # Safety
///
/// `reader` must be a live `EMailReader`, on the main loop.
unsafe fn jmap_service_uid(reader: *mut EMailReader) -> Option<String> {
    // SAFETY: a live reader; the folder reference is released here, the store
    // and its strings are the folder's own.
    unsafe {
        let folder = e_mail_reader_ref_folder(reader);
        if folder.is_null() {
            return None;
        }
        let uid = service_uid_of(folder);
        g_object_unref(folder.cast());
        uid
    }
}

/// # Safety
///
/// `folder` must be a live `CamelFolder`.
unsafe fn service_uid_of(folder: *mut CamelFolder) -> Option<String> {
    // SAFETY: per the contract; store and provider are borrowed from it.
    unsafe {
        let store = camel_folder_get_parent_store(folder);
        if store.is_null() {
            return None;
        }
        let provider = camel_service_get_provider(store.cast());
        if provider.is_null() {
            return None;
        }
        let protocol = read_string((*provider).protocol)?;
        if protocol != BACKEND_NAME.to_str().ok()? {
            return None;
        }
        read_string(camel_service_get_uid(store.cast()))
    }
}

/// The source to connect as for the reader's folder: the service's own source
/// resolved in the registry, lifted to its collection when it has one. An
/// owned reference.
///
/// # Safety
///
/// `reader` must be a live `EMailReader`, on the main loop.
unsafe fn connect_source_of(reader: *mut EMailReader) -> Option<*mut ESource> {
    let service_uid = unsafe { jmap_service_uid(reader) }?;
    let service_uid = CString::new(service_uid).ok()?;
    // SAFETY: the shell singleton and its registry are the process's own;
    // every source reference taken is released except the one handed back.
    unsafe {
        let shell = e_shell_get_default();
        if shell.is_null() {
            return None;
        }
        let registry = e_shell_get_registry(shell);
        if registry.is_null() {
            return None;
        }
        let account = e_source_registry_ref_source(registry, service_uid.as_ptr());
        if account.is_null() {
            return None;
        }
        let parent = read_string(e_source_get_parent(account))
            .and_then(|uid| CString::new(uid).ok())
            .map(|uid| e_source_registry_ref_source(registry, uid.as_ptr()))
            .filter(|source| !source.is_null());
        Some(match parent {
            Some(parent) => {
                g_object_unref(account.cast());
                parent
            }
            None => account,
        })
    }
}

/// A source reference the worker thread may hold; see `vacation::page`.
struct SendSource(*mut ESource);
// SAFETY: GObject refcounting is thread-safe and ESource getters take the
// source's property lock.
unsafe impl Send for SendSource {}

unsafe extern "C" fn activate_hour(_action: *mut GtkAction, owner: gpointer) {
    guard("snooze::hour", (), || unsafe {
        // SAFETY: the owner is alive — its own menu emitted.
        activate(owner.cast(), Preset::InOneHour);
    });
}

unsafe extern "C" fn activate_tomorrow(_action: *mut GtkAction, owner: gpointer) {
    guard("snooze::tomorrow", (), || unsafe {
        // SAFETY: as `activate_hour`.
        activate(owner.cast(), Preset::TomorrowMorning);
    });
}

unsafe extern "C" fn activate_monday(_action: *mut GtkAction, owner: gpointer) {
    guard("snooze::monday", (), || unsafe {
        // SAFETY: as `activate_hour`.
        activate(owner.cast(), Preset::MondayMorning);
    });
}

/// A click: snapshot the selection on the main loop, snooze it on a worker.
///
/// # Safety
///
/// `owner` must be a live installed extensible, on the main loop.
unsafe fn activate(owner: *mut GObject, preset: Preset) {
    let Some(cell) = (unsafe { state(owner) }) else {
        return;
    };
    let reader = cell.borrow().reader;

    // SAFETY: the reader is alive with its owner; the uid array is owned and
    // released here, its strings copied first.
    let uids: Vec<String> = unsafe {
        let array = e_mail_reader_get_selected_uids(reader);
        if array.is_null() {
            return;
        }
        let uids = (0..(*array).len)
            .filter_map(|i| read_string((*array).pdata.add(i as usize).read().cast()))
            .collect();
        glib_sys::g_ptr_array_unref(array);
        uids
    };
    if uids.is_empty() {
        return;
    }

    let Some(until) = schedule::hold_seconds(preset).and_then(schedule::utc_in) else {
        warn_dialog(translate(NO_CLOCK));
        return;
    };
    // SAFETY: a live reader; the reference is the worker's.
    let Some(source) = (unsafe { connect_source_of(reader) }) else {
        return;
    };
    let source = SendSource(source);

    let work = move || {
        let source = source;
        // SAFETY: the reference `connect_source_of` took is the worker's.
        let outcome = unsafe { link::connect_account(source.0) };
        // SAFETY: releasing the worker's reference.
        unsafe { g_object_unref(source.0.cast()) };
        outcome.and_then(|link| snooze_all(&link, &uids, &until))
    };
    let finish = |_owner: *mut GObject, outcome: Result<usize, String>| match outcome {
        Ok(count) => tracing::debug!(count, "messages snoozed"),
        Err(message) => warn_dialog(translate_with(SNOOZE_FAILED, &[&message])),
    };
    // SAFETY: a live owner per this function's contract.
    unsafe { dispatch::spawn_for(owner, work, finish) };
}

/// The blocking half: the snoozed-role mailbox once, then one `Email/set`
/// per message. Camel uids are JMAP email ids on this provider, which is what
/// lets the selection cross as strings.
fn snooze_all(link: &AccountLink, uids: &[String], until: &str) -> Result<usize, String> {
    if !link.features.snooze {
        return Err(translate(NOT_OFFERED));
    }
    let account_id = &link.features.account_id;
    let snoozed = link
        .call(|client| client.snoozed_mailbox(account_id))
        .map_err(|error| crate::link::describe(&error))?;
    let snoozed_id = snoozed
        .id
        .ok_or_else(|| translate(c"the server named no id for the snoozed mailbox"))?;

    // `moveToMailboxId` stays unset: the extension's default wake target is
    // the inbox, which is exactly what snooze means here.
    let details = SnoozeDetails::new(until);
    for uid in uids {
        let email_id = Id::new(uid.clone());
        link.call(|client| client.snooze_email(account_id, &email_id, &snoozed_id, &details))
            .map_err(|error| crate::link::describe(&error))?;
    }
    Ok(uids.len())
}

/// # Safety
///
/// `action` must be a live GtkAction.
unsafe fn set_tooltip(action: *mut GtkAction, text: String) {
    let text = CString::new(text).unwrap_or_default();
    // SAFETY: per the contract; the property machinery copies the string.
    unsafe {
        g_object_set(
            action.cast(),
            c"tooltip".as_ptr(),
            text.as_ptr(),
            ptr::null::<std::ffi::c_char>(),
        );
    }
}

/// A modal explanation, parentless: the reader window a snooze failed in may
/// itself be a popup's ephemeral context, and the message stands alone.
fn warn_dialog(text: String) {
    let text = CString::new(text).unwrap_or_default();
    // SAFETY: a fresh dialog, run and destroyed before returning; "%s" keeps
    // the text out of printf's hands.
    unsafe {
        let dialog = gtk_message_dialog_new(
            ptr::null_mut(),
            0,
            GTK_MESSAGE_ERROR,
            GTK_BUTTONS_CLOSE,
            c"%s".as_ptr(),
            text.as_ptr(),
        );
        gtk_dialog_run(dialog.cast());
        gtk_widget_destroy(dialog);
    }
}

/// The reader behind a mail shell view's content: its `mail-view` property,
/// which implements `EMailReader` (an interface pointer is the instance
/// pointer). Borrowed transfer-full and released: the view itself keeps it
/// alive for the owner's life.
///
/// # Safety
///
/// `content` must be NULL or a live mail `EShellContent`.
pub(crate) unsafe fn reader_of_content(content: *mut EShellContent) -> *mut EMailReader {
    if content.is_null() {
        return ptr::null_mut();
    }
    let mut view: *mut GObject = ptr::null_mut();
    // SAFETY: a live content per the contract; the property hands back a
    // reference this call owns and releases (the content keeps its own).
    unsafe {
        g_object_get(
            content.cast(),
            c"mail-view".as_ptr(),
            &mut view as *mut *mut GObject,
            ptr::null::<std::ffi::c_char>(),
        );
        if view.is_null() {
            return ptr::null_mut();
        }
        g_object_unref(view);
        view.cast()
    }
}
