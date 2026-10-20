// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The snooze submenu, shared by the two window extensions.
//! This file merges the submenu into the menu layer and sets its sensitivity.
//! It also snoozes the selected messages when the user clicks a preset.
//!
//! The `evolution_eui_manager` cfg selects one of two menu layers at compile
//! time (see `evo-sys/build.rs`):
//!
//! - Evolution < 3.55 uses `GtkUIManager` and a `GtkActionGroup` that the
//!   caller supplies (the "mail" group of the window, or the standard group
//!   of the browser).
//! - Evolution >= 3.55 uses `EUIManager`. `install` creates and names its own
//!   group. One call, `e_ui_manager_add_actions_with_eui_data`, adds the
//!   actions and the menu placement.
//!
//! Both layers fill the same [`SnoozeState`] and use the same selection, gate
//! and activation logic. Only the registration and the setters are different.
//!
//! The gate has two levels. The first level is the provider protocol of the
//! folder, which the code reads synchronously on each `update-actions`. The
//! second level is the snooze capability of the server. A worker thread gets
//! it one time for each account. The cache ([`crate::session_cache::shared`])
//! shares the result with all reader windows. Until the result is known, the
//! submenu is insensitive and shows a "checking" tooltip. If the server does
//! not have the extension, the submenu stays insensitive and the tooltip
//! gives the reason.

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::ptr;

use eds_sys::{
    CamelFolder, ESource, camel_folder_get_parent_store, camel_service_get_provider,
    camel_service_get_uid, e_source_get_parent, e_source_registry_ref_source,
};
use evo_sys::{
    EMailReader, EShellContent, GTK_BUTTONS_CLOSE, GTK_MESSAGE_ERROR,
    e_mail_reader_get_selected_uids, e_mail_reader_ref_folder, e_shell_get_default,
    e_shell_get_registry, gtk_dialog_run, gtk_message_dialog_new, gtk_widget_destroy,
};
#[cfg(evolution_eui_manager)]
use evo_sys::{
    EUIAction, EUIActionEntry, EUIManager, GVariant, e_ui_action_group_get_action,
    e_ui_action_set_sensitive, e_ui_action_set_tooltip, e_ui_manager_add_actions_with_eui_data,
    e_ui_manager_get_action_group,
};
#[cfg(not(evolution_eui_manager))]
use evo_sys::{
    GtkAction, GtkActionGroup, GtkUIManager, gtk_action_group_add_action, gtk_action_new,
    gtk_action_set_sensitive, gtk_ui_manager_add_ui_from_string, gtk_ui_manager_ensure_update,
};
#[cfg(not(evolution_eui_manager))]
use glib_sys::{GError, g_error_free};
use glib_sys::{GFALSE, GTRUE, gpointer};
use gobject_sys::{
    GObject, g_object_get, g_object_get_data, g_object_set_data_full, g_object_unref,
};
#[cfg(not(evolution_eui_manager))]
use gobject_sys::{g_object_set, g_signal_connect_data};
#[cfg(not(evolution_eui_manager))]
use jmap_backend_core::callback::gcallback;
use jmap_backend_core::i18n::{N_, translate, translate_static, translate_with};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::trampoline::guard;
use jmap_proto::Id;
use jmap_proto::mail::SnoozeDetails;

use crate::dispatch;
use crate::link::{self, AccountLink, SendSource};
use crate::send_later::schedule::{self, Preset};
use crate::session_cache;

/// See `vacation::extension` for the reason that this literal is not imported.
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

/// The action of the submenu, for the menu layer that is compiled in.
#[cfg(not(evolution_eui_manager))]
type MenuAction = *mut GtkAction;
#[cfg(evolution_eui_manager)]
type MenuAction = *mut EUIAction;

/// The snooze state of one reader window, kept in a box as qdata on the extensible.
struct SnoozeState {
    /// The reader that supplies the selection and the folder.
    /// It is the mail view (shell window) or the browser itself.
    /// It stays alive while the owner is alive.
    reader: *mut EMailReader,
    /// The action of the submenu. Its sensitivity shows the result of the gate.
    menu_action: MenuAction,
    /// The service uid of the capability fetch that is in progress.
    /// It prevents a new fetch for the same account on each `update-actions`.
    pending: Option<String>,
}

/// The [`SnoozeState`] of the owner, while the owner is alive.
///
/// # Safety
///
/// `owner` must be a live extensible that [`install`] decorated.
/// Do not keep the reference after the main-loop callback ends.
unsafe fn state<'a>(owner: *mut GObject) -> Option<&'a RefCell<SnoozeState>> {
    // SAFETY: `install` sets the data one time, with this type.
    unsafe {
        g_object_get_data(owner, STATE_KEY.as_ptr())
            .cast::<RefCell<SnoozeState>>()
            .as_ref()
    }
}

unsafe extern "C" fn drop_state(data: gpointer) {
    // SAFETY: this takes back the box that `install` leaked, one time only.
    drop(unsafe { Box::from_raw(data.cast::<RefCell<SnoozeState>>()) });
}

/// The `activate` handler of one preset, in the callback shape of the
/// compiled-in menu layer.
#[cfg(not(evolution_eui_manager))]
type ActivateFn = unsafe extern "C" fn(*mut GtkAction, gpointer);
#[cfg(evolution_eui_manager)]
type ActivateFn = unsafe extern "C" fn(*mut EUIAction, *mut GVariant, gpointer);

/// The three presets. They use the same [`Preset`] values as `send_later`,
/// so that the two menus give the same times.
const PRESETS: &[(&CStr, &CStr, ActivateFn)] = &[
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

/// The group name of this extension under `EUIManager`.
/// It is not an Evolution group name. Upstream, `mailing-list-actions.c` and
/// `e-composer-to-meeting.c` also register their own groups.
/// The merged `.eui` finds actions in all groups of the manager,
/// thus the placement does not depend on this name.
#[cfg(evolution_eui_manager)]
const GROUP_NAME: &CStr = c"jmap-snooze";

/// The submenu is a direct child of `/mail-message-popup`
/// (3.52: `<popup name='mail-message-popup'>`; 3.55+: `<menu id='mail-message-popup'>`).
///
/// That popup has a `mail-message-popup-common-actions` placeholder
/// (in the installed 3.52.3 `evolution-mail.ui` and `evolution-mail-reader.ui`).
/// This fragment does not use the placeholder.
/// The placeholder `mail-message-popup-actions` is in `mail-preview-popup`, not in this popup.
/// `mail-to-task.c` also merges its submenu as a direct child of this popup,
/// and its merge fragment also does not set `is-popup`.
#[cfg(not(evolution_eui_manager))]
const UI: &CStr = c"<popup name='mail-message-popup'>\
<menu action='jmap-snooze-menu'>\
<menuitem action='jmap-snooze-hour'/>\
<menuitem action='jmap-snooze-tomorrow'/>\
<menuitem action='jmap-snooze-monday'/>\
</menu></popup>";
#[cfg(evolution_eui_manager)]
const UI: &CStr = c"<eui>\
<menu id='mail-message-popup'>\
<submenu action='jmap-snooze-menu'>\
<item action='jmap-snooze-hour'/>\
<item action='jmap-snooze-tomorrow'/>\
<item action='jmap-snooze-monday'/>\
</submenu>\
</menu>\
</eui>";

/// Merges the submenu into `ui_manager` and attaches the state to `owner`.
/// On 3.52, the caller also supplies `action_group`.
/// On 3.55+, the function creates its own group (see `GROUP_NAME`).
/// The function is idempotent for each owner: a second call finds the state
/// and returns.
///
/// # Safety
///
/// `owner` must be a live GObject that lives longer than `reader` and
/// `ui_manager` (3.52: and `action_group`). All must be live and belong to the
/// same window. Call on the main loop only.
#[cfg(not(evolution_eui_manager))]
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

    // SAFETY: the objects are live per the contract of this function.
    // The reference of each action goes to the group.
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
                gcallback(*handler),
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
    // SAFETY: `owner` is live; the ownership of the box goes to the qdata.
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

/// # Safety
///
/// `owner` must be a live GObject that lives longer than `reader` and
/// `ui_manager`. Both must be live and belong to the same window.
/// Call on the main loop only.
#[cfg(evolution_eui_manager)]
pub(crate) unsafe fn install(
    owner: *mut GObject,
    reader: *mut EMailReader,
    ui_manager: *mut EUIManager,
) {
    if unsafe { state(owner) }.is_some() {
        return;
    }
    if reader.is_null() || ui_manager.is_null() {
        return;
    }

    // SAFETY: the manager is live per the contract of this function.
    // Each pointer in an entry is 'static (the preset names, PRESETS) or a
    // `translate_static` string that lives for the process.
    // The array must only live for the duration of the call, and this
    // function owns it for that time.
    let menu_action = unsafe {
        let mut entries = vec![EUIActionEntry {
            name: c"jmap-snooze-menu".as_ptr(),
            icon_name: ptr::null(),
            label: translate_static(N_(c"S_nooze")),
            accel: ptr::null(),
            tooltip: translate_static(CHECKING),
            activate: None,
            parameter_type: ptr::null(),
            state: ptr::null(),
            change_state: None,
        }];
        entries.extend(PRESETS.iter().map(|(name, label, handler)| EUIActionEntry {
            name: name.as_ptr(),
            icon_name: ptr::null(),
            label: translate_static(label),
            accel: ptr::null(),
            tooltip: ptr::null(),
            activate: Some(*handler),
            parameter_type: ptr::null(),
            state: ptr::null(),
            change_state: None,
        }));

        e_ui_manager_add_actions_with_eui_data(
            ui_manager,
            GROUP_NAME.as_ptr(),
            ptr::null(),
            entries.as_ptr(),
            entries.len() as std::ffi::c_int,
            owner.cast(),
            UI.as_ptr(),
        );

        let group = e_ui_manager_get_action_group(ui_manager, GROUP_NAME.as_ptr());
        e_ui_action_group_get_action(group, c"jmap-snooze-menu".as_ptr())
    };
    if menu_action.is_null() {
        tracing::error!("the snooze submenu could not be merged");
        return;
    }
    // SAFETY: `menu_action` is the live action from `e_ui_action_group_get_action`.
    // The check above makes sure that it is not NULL.
    unsafe { set_action_sensitive(menu_action, false) };

    tracing::trace!("snooze submenu merged into a JMAP-capable reader window");
    let snooze = Box::new(RefCell::new(SnoozeState {
        reader,
        menu_action,
        pending: None,
    }));
    // SAFETY: `owner` is live; the ownership of the box goes to the qdata.
    // `menu_action` is a borrowed reference from the group of the manager
    // (`e_ui_action_group_get_action` does not transfer a reference).
    // Thus, unlike the 3.52 path, there is no reference to release here.
    unsafe {
        g_object_set_data_full(
            owner,
            STATE_KEY.as_ptr(),
            Box::into_raw(snooze).cast(),
            Some(drop_state),
        );
        update_sensitivity(owner);
    }
}

/// # Safety
///
/// `action` must be a live action of whichever menu layer is compiled in.
#[cfg(not(evolution_eui_manager))]
unsafe fn set_action_sensitive(action: MenuAction, sensitive: bool) {
    // SAFETY: per the contract.
    unsafe { gtk_action_set_sensitive(action, if sensitive { GTRUE } else { GFALSE }) };
}
#[cfg(evolution_eui_manager)]
unsafe fn set_action_sensitive(action: MenuAction, sensitive: bool) {
    // SAFETY: per the contract.
    unsafe { e_ui_action_set_sensitive(action, if sensitive { GTRUE } else { GFALSE }) };
}

/// The `update-actions` handler. It sets the sensitivity from the selection,
/// the folder provider, and the cached server answer.
/// If the cache has no answer, it starts a fetch.
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

    // SAFETY: the reader is alive while its owner is alive.
    // This code owns the uid array and the folder and releases them.
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
            // SAFETY: the action is alive while the window of the owner is alive.
            unsafe {
                set_action_sensitive(menu_action, sensitive);
                set_tooltip(menu_action, tooltip);
            }
        }
        None => {
            let uid = service_uid.unwrap_or_default();
            {
                let mut snooze = cell.borrow_mut();
                if snooze.pending.as_deref() == Some(uid.as_str()) {
                    return; // a fetch for this account is in progress
                }
                snooze.pending = Some(uid.clone());
            }
            // SAFETY: as above.
            unsafe {
                set_action_sensitive(menu_action, false);
                set_tooltip(menu_action, translate(CHECKING));
            }
            // SAFETY: the reader is alive. The worker owns the source
            // reference and releases it.
            let Some(source) = (unsafe { connect_source_of(reader) }) else {
                // Clear `pending`, or one failure blocks all later fetches for this account.
                tracing::debug!(%uid, "snooze gate: the account source could not be resolved");
                if let Some(cell) = unsafe { state(owner) } {
                    cell.borrow_mut().pending = None;
                }
                return;
            };
            // SAFETY: this gives the reference from `connect_source_of` to the wrapper.
            let source = unsafe { SendSource::adopt(source) };
            let fetch_uid = uid.clone();
            let work = move || {
                // SAFETY: the worker owns the reference from
                // `connect_source_of` for the full closure.
                let outcome = unsafe { link::connect_account(source.as_ptr()) };
                drop(source);
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
                    // SAFETY: `with_strong` gives a live owner.
                    unsafe {
                        if let Some(cell) = state(owner) {
                            let mut snooze = cell.borrow_mut();
                            if snooze.pending.as_deref() == Some(uid.as_str()) {
                                snooze.pending = None;
                            }
                            if let Err(message) = outcome {
                                let menu_action = snooze.menu_action;
                                drop(snooze);
                                set_action_sensitive(menu_action, false);
                                set_tooltip(menu_action, translate_with(NOT_REACHED, &[&message]));
                                return;
                            }
                        }
                        update_sensitivity(owner);
                    }
                };
            // SAFETY: the owner is live per the contract of this function.
            unsafe { dispatch::spawn_for(owner, work, finish) };
        }
    }
}

/// The Camel service uid of the folder, if the folder belongs to this provider.
///
/// # Safety
///
/// `reader` must be a live `EMailReader`, on the main loop.
unsafe fn jmap_service_uid(reader: *mut EMailReader) -> Option<String> {
    // SAFETY: the reader is live. This code releases the folder reference.
    // The folder owns the store and its strings.
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
    // SAFETY: per the contract. The store and the provider are borrowed from the folder.
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

/// The source to connect with for the folder of the reader.
/// It is the source of the service in the registry, or its collection source
/// if it has one. The caller owns the returned reference.
///
/// # Safety
///
/// `reader` must be a live `EMailReader`, on the main loop.
unsafe fn connect_source_of(reader: *mut EMailReader) -> Option<*mut ESource> {
    let service_uid = unsafe { jmap_service_uid(reader) }?;
    let service_uid = CString::new(service_uid).ok()?;
    // SAFETY: the process owns the shell singleton and its registry.
    // This code releases each source reference, but not the returned reference.
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

#[cfg(not(evolution_eui_manager))]
unsafe extern "C" fn activate_hour(_action: *mut GtkAction, owner: gpointer) {
    guard("snooze::hour", (), || unsafe {
        // SAFETY: the owner is alive, because its menu sent the signal.
        activate(owner.cast(), Preset::InOneHour);
    });
}
#[cfg(evolution_eui_manager)]
unsafe extern "C" fn activate_hour(
    _action: *mut EUIAction,
    _value: *mut GVariant,
    owner: gpointer,
) {
    guard("snooze::hour", (), || unsafe {
        // SAFETY: the same as the 3.52 trampoline with this name.
        activate(owner.cast(), Preset::InOneHour);
    });
}

#[cfg(not(evolution_eui_manager))]
unsafe extern "C" fn activate_tomorrow(_action: *mut GtkAction, owner: gpointer) {
    guard("snooze::tomorrow", (), || unsafe {
        // SAFETY: the same as `activate_hour`.
        activate(owner.cast(), Preset::TimeOfDay(8));
    });
}
#[cfg(evolution_eui_manager)]
unsafe extern "C" fn activate_tomorrow(
    _action: *mut EUIAction,
    _value: *mut GVariant,
    owner: gpointer,
) {
    guard("snooze::tomorrow", (), || unsafe {
        // SAFETY: the same as `activate_hour`.
        activate(owner.cast(), Preset::TimeOfDay(8));
    });
}

#[cfg(not(evolution_eui_manager))]
unsafe extern "C" fn activate_monday(_action: *mut GtkAction, owner: gpointer) {
    guard("snooze::monday", (), || unsafe {
        // SAFETY: the same as `activate_hour`.
        activate(owner.cast(), Preset::NextWorkday);
    });
}
#[cfg(evolution_eui_manager)]
unsafe extern "C" fn activate_monday(
    _action: *mut EUIAction,
    _value: *mut GVariant,
    owner: gpointer,
) {
    guard("snooze::monday", (), || unsafe {
        // SAFETY: the same as `activate_hour`.
        activate(owner.cast(), Preset::NextWorkday);
    });
}

/// Handles a click. It copies the selection on the main loop and snoozes the
/// messages on a worker thread.
///
/// # Safety
///
/// `owner` must be a live installed extensible, on the main loop.
unsafe fn activate(owner: *mut GObject, preset: Preset) {
    let Some(cell) = (unsafe { state(owner) }) else {
        return;
    };
    let reader = cell.borrow().reader;

    // SAFETY: the reader is alive while its owner is alive. This code owns and
    // releases the uid array, after it copies the strings.
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

    let Some(until) = schedule::resolve(preset)
        .map(|occurrence| occurrence.hold)
        .and_then(schedule::utc_in)
    else {
        warn_dialog(translate(NO_CLOCK));
        return;
    };
    // SAFETY: the reader is live. The worker owns the reference.
    let Some(source) = (unsafe { connect_source_of(reader) }) else {
        return;
    };
    // SAFETY: this gives the reference from `connect_source_of` to the wrapper.
    let source = unsafe { SendSource::adopt(source) };

    let work = move || {
        // SAFETY: the worker owns the reference from `connect_source_of`.
        let outcome = unsafe { link::connect_account(source.as_ptr()) };
        drop(source);
        outcome.and_then(|link| snooze_all(&link, &uids, &until))
    };
    let finish = |_owner: *mut GObject, outcome: Result<usize, String>| match outcome {
        Ok(count) => tracing::debug!(count, "messages snoozed"),
        Err(message) => warn_dialog(translate_with(SNOOZE_FAILED, &[&message])),
    };
    // SAFETY: the owner is live per the contract of this function.
    unsafe { dispatch::spawn_for(owner, work, finish) };
}

/// The blocking part. It gets the mailbox with the snoozed role one time,
/// then sends one `Email/set` for each message.
/// On this provider, Camel uids are JMAP email ids, thus the selection can go
/// to the worker as strings.
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

    // `moveToMailboxId` is not set: the default wake target of the extension is the inbox.
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
/// `action` must be a live action of whichever menu layer is compiled in.
#[cfg(not(evolution_eui_manager))]
unsafe fn set_tooltip(action: MenuAction, text: String) {
    let text = CString::new(text).unwrap_or_default();
    // SAFETY: per the contract. The property system copies the string.
    unsafe {
        g_object_set(
            action.cast(),
            c"tooltip".as_ptr(),
            text.as_ptr(),
            ptr::null::<std::ffi::c_char>(),
        );
    }
}
/// # Safety
///
/// `action` must be a live `EUIAction`.
#[cfg(evolution_eui_manager)]
unsafe fn set_tooltip(action: MenuAction, text: String) {
    let text = CString::new(text).unwrap_or_default();
    // SAFETY: per the contract. The setter copies the string.
    unsafe { e_ui_action_set_tooltip(action, text.as_ptr()) };
}

/// Shows a modal error dialog with no parent window.
/// The reader window can be the temporary context of a popup, and the message
/// is complete without a parent.
fn warn_dialog(text: String) {
    let text = CString::new(text).unwrap_or_default();
    // SAFETY: the dialog is new, and this code runs and destroys it before it returns.
    // "%s" prevents printf from reading format directives in the text.
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

/// The reader of the content of a mail shell view: its `mail-view` property.
/// The property implements `EMailReader` (an interface pointer is the instance
/// pointer). The getter gives a full reference, which this function releases.
/// The content keeps the view alive while the owner is alive.
///
/// # Safety
///
/// `content` must be NULL or a live mail `EShellContent`.
pub(crate) unsafe fn reader_of_content(content: *mut EShellContent) -> *mut EMailReader {
    if content.is_null() {
        return ptr::null_mut();
    }
    let mut view: *mut GObject = ptr::null_mut();
    // SAFETY: the content is live per the contract. The property gives a
    // reference that this call owns and releases (the content keeps its own).
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
