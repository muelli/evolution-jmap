// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapSendLaterExtension`: the `EExtension` on `EMsgComposer` that merges
//! the *File ▸ Send Later* submenu, gates it on the From account, and walks a
//! click through the composer's own message builder to
//! [`crate::send_later::submit::schedule_send`].
//!
//! The gate re-runs on every From switch (`EComposerHeaderTable`'s
//! `notify::identity-uid`): level 1 is the identity's transport backend name
//! — the evolution-ews chain, identity source → `[Mail Submission]`
//! `TransportUid` → transport source's backend name — and level 2 is a
//! session fetch off the main loop, whose answer lands back through
//! [`crate::dispatch`] guarded by a generation counter, so a slow fetch for
//! the previous account cannot sensitize the menu for the current one.
//!
//! On success the composer window is destroyed the way Evolution's own send
//! path destroys it; on refusal a modal message dialog says why, and — since
//! the draft-first shape can leave one behind — where a copy may remain.

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use eds_sys::{
    CamelAddress, CamelInternetAddress, CamelMedium, CamelMimeMessage,
    E_SOURCE_EXTENSION_MAIL_SUBMISSION, E_SOURCE_EXTENSION_MAIL_TRANSPORT, EExtension,
    EExtensionClass, ESource, ESourceBackend, ESourceMailSubmission, ESourceRegistry,
    camel_address_length, camel_internet_address_get, camel_medium_dup_headers,
    camel_medium_remove_header, camel_mime_message_get_from, camel_mime_message_get_recipients,
    camel_name_value_array_free, camel_name_value_array_get, camel_name_value_array_get_length,
    e_extension_get_extensible, e_extension_get_type, e_source_backend_get_backend_name,
    e_source_get_parent, e_source_mail_submission_get_transport_uid, e_source_registry_ref_source,
};
use evo_sys::{
    E_COMPOSER_HEADER_FROM, EMsgComposer, GTK_BUTTONS_CLOSE, GTK_DIALOG_DESTROY_WITH_PARENT,
    GTK_DIALOG_MODAL, GTK_MESSAGE_ERROR, GTK_RESPONSE_CANCEL, GTK_RESPONSE_OK, GtkAction,
    GtkActionGroup, GtkWindow, e_composer_header_get_registry,
    e_composer_header_table_dup_identity_uid, e_composer_header_table_get_header,
    e_date_edit_get_time, e_date_edit_new, e_date_edit_set_show_time, e_date_edit_set_time,
    e_html_editor_get_action_group, e_html_editor_get_ui_manager, e_msg_composer_get_editor,
    e_msg_composer_get_header_table, e_msg_composer_get_message, e_msg_composer_get_message_finish,
    e_msg_composer_get_type, gtk_action_get_name, gtk_action_group_add_action,
    gtk_action_group_get_action, gtk_action_new, gtk_action_set_sensitive, gtk_container_add,
    gtk_dialog_add_button, gtk_dialog_get_content_area, gtk_dialog_new, gtk_dialog_run,
    gtk_message_dialog_new, gtk_ui_manager_add_ui_from_string, gtk_ui_manager_ensure_update,
    gtk_widget_destroy, gtk_widget_show_all, gtk_window_set_title, gtk_window_set_transient_for,
};
use gio_sys::GAsyncResult;
use glib_sys::{GError, GFALSE, GTRUE, GType, g_error_free, g_free, gpointer};
use gobject_sys::{
    GObject, GObjectClass, GParamSpec, g_object_get_data, g_object_set, g_object_set_data_full,
    g_object_unref, g_signal_connect_data,
};
use jmap_backend_core::i18n::{N_, translate, translate_static, translate_with};
use jmap_backend_core::marshal::{extension_if_present, read_string};
use jmap_backend_core::mime::write_message;
use jmap_backend_core::subclass::{self, ObjectSubclass};
use jmap_backend_core::trampoline::guard;
use jmap_proto::mail::{Envelope, EnvelopeAddress};

use crate::dispatch;
use crate::link::{self, AccountLink};
use crate::send_later::schedule::{self, Preset};
use crate::send_later::submit;

/// See `vacation::extension` on why this literal is not imported.
const BACKEND_NAME: &CStr = c"jmap";

const STATE_KEY: &CStr = c"jmap-send-later-state";

const CHECKING: &CStr = N_(c"Checking whether the sending account offers scheduled send…");
const NOT_JMAP: &CStr = N_(c"The selected sending account is not a JMAP account");
const NOT_OFFERED: &CStr = N_(c"The account’s server does not offer scheduled send");
// TRANSLATORS: %1$s is the reason the mail server gave.
const NOT_REACHED: &CStr = N_(c"The account’s server could not be asked: %1$s");
const OFFERED: &CStr = N_(c"The server holds the message and sends it at the chosen time");
// TRANSLATORS: %1$s is the reason the mail server gave.
const REFUSED: &CStr = N_(
    c"The message could not be scheduled: %1$s (a copy may remain in the server’s Drafts folder)",
);
const NO_RECIPIENTS: &CStr = N_(c"The message names no recipients");
const NO_FROM: &CStr = N_(c"The message has no From address");
const TOO_FAR: &CStr = N_(c"The chosen time is further ahead than the server accepts");
const NO_CLOCK: &CStr = N_(c"The local calendar could not name the chosen time");
const NOT_FUTURE: &CStr = N_(c"That time has already passed — choose one in the future");

/// Per-composer state, boxed as qdata on the composer.
struct SendState {
    /// The connected account behind the current From identity, once the
    /// level-2 fetch answered — what a click submits through.
    link: Option<Arc<AccountLink>>,
    /// Guards a slow fetch against a From switch: only the newest wins.
    generation: u64,
    /// The submenu's own action, whose sensitivity and tooltip are the gate's
    /// visible half.
    menu_action: *mut GtkAction,
    /// The group the preset actions live in, so their labels can be brought
    /// up to date — "Today, 18:00" is only true until 18:00.
    action_group: *mut GtkActionGroup,
}

static GENERATIONS: AtomicU64 = AtomicU64::new(1);

/// The composer's [`SendState`], while it is alive.
///
/// # Safety
///
/// `composer` must be a live composer this extension decorated; the reference
/// must not outlive one main-loop callback.
unsafe fn state<'a>(composer: *mut GObject) -> Option<&'a RefCell<SendState>> {
    // SAFETY: set once in `constructed`; the cast is the one it stored.
    unsafe {
        g_object_get_data(composer, STATE_KEY.as_ptr())
            .cast::<RefCell<SendState>>()
            .as_ref()
    }
}

unsafe extern "C" fn drop_state(data: gpointer) {
    // SAFETY: reclaiming the box `constructed` leaked, exactly once.
    drop(unsafe { Box::from_raw(data.cast::<RefCell<SendState>>()) });
}

/// The instance: `EExtension`'s own state and nothing else.
#[repr(C)]
pub struct JmapSendLaterExtension {
    parent: EExtension,
}

/// The class: `EExtensionClass`'s own state and nothing else.
#[repr(C)]
pub struct JmapSendLaterExtensionClass {
    parent_class: EExtensionClass,
}

// SAFETY: both structs are #[repr(C)] and lead with EExtension's own structs;
// EExtension derives from GObject.
unsafe impl ObjectSubclass for JmapSendLaterExtension {
    const NAME: &'static CStr = c"JmapSendLaterExtension";
    type Instance = JmapSendLaterExtension;
    type Class = JmapSendLaterExtensionClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_extension_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` leads with `EExtensionClass`.
        unsafe { (*class).parent_class.extensible_type = e_msg_composer_get_type() };
        let object_class = class.cast::<GObjectClass>();
        // SAFETY: transitively leads with GObjectClass.
        unsafe { (*object_class).constructed = Some(constructed) };
    }
}

/// The three presets: action name, menu label, and activate handler. The
/// handlers differ only in the preset they pass on; three small trampolines
/// keep the connection data a plain composer pointer.
/// The action name of one preset's menu item.
fn action_name(preset: Preset) -> CString {
    CString::new(format!("jmap-send-later-{}", schedule::slug(preset))).unwrap_or_default()
}

/// The submenu's own XML, with one item per offered moment plus the custom
/// entry — built rather than a constant, because which moments are offered
/// is [`schedule::offered`]'s to say.
fn ui_definition() -> CString {
    let mut items = String::new();
    for preset in schedule::offered() {
        items.push_str(&format!(
            "<menuitem action='jmap-send-later-{}'/>",
            schedule::slug(preset)
        ));
    }
    items.push_str("<separator/><menuitem action='jmap-send-later-custom'/>");
    CString::new(format!(
        "<menubar name='main-menu'>\
<placeholder name='pre-edit-menu'><menu action='file-menu'>\
<placeholder name='custom-actions-placeholder'>\
<menu action='jmap-send-later-menu'>{items}</menu>\
</placeholder></menu></placeholder></menubar>"
    ))
    .unwrap_or_default()
}

/// Chains up, merges the menu (insensitive), and arms the gate.
unsafe extern "C" fn constructed(object: *mut GObject) {
    guard("JmapSendLaterExtension::constructed", (), || unsafe {
        // SAFETY: the parent class of a live instance is initialised and alive.
        let parent = subclass::parent_class::<GObjectClass>(JmapSendLaterExtension::parent_type());
        if let Some(chained) = parent.and_then(|class| class.constructed) {
            chained(object);
        }

        // SAFETY: GObject passes a live instance; the extensible is the
        // composer this extension was instantiated for.
        let composer: *mut EMsgComposer =
            e_extension_get_extensible(object.cast::<EExtension>()).cast();
        let editor = e_msg_composer_get_editor(composer);
        let ui_manager = e_html_editor_get_ui_manager(editor);
        let action_group = e_html_editor_get_action_group(editor, c"core".as_ptr());
        if ui_manager.is_null() || action_group.is_null() {
            return;
        }

        // The submenu's own action; label only, activated by its items.
        let menu_action = gtk_action_new(
            c"jmap-send-later-menu".as_ptr(),
            translate_static(N_(c"Send _Later")),
            translate_static(CHECKING),
            ptr::null(),
        );
        gtk_action_set_sensitive(menu_action, GFALSE);
        gtk_action_group_add_action(action_group, menu_action);

        // One action per offered moment, plus the custom entry. All of them
        // share `on_activate`, which recovers the moment from the action's
        // own name; the labels are filled in by `refresh_labels` below and
        // again on every re-gate, since what "Today, 18:00" means moves.
        let mut names: Vec<CString> = schedule::offered()
            .iter()
            .copied()
            .map(action_name)
            .collect();
        names.push(CString::new("jmap-send-later-custom").unwrap_or_default());
        for name in &names {
            let is_custom = name.to_bytes().ends_with(b"-custom");
            let action = gtk_action_new(
                name.as_ptr(),
                if is_custom {
                    translate_static(N_(c"_Custom Time…"))
                } else {
                    ptr::null()
                },
                ptr::null(),
                ptr::null(),
            );
            // SAFETY: the composer outlives its own menu's actions (the
            // action group is the editor's, owned by the composer), so a
            // plain pointer is the connection data evolution-ews itself uses.
            g_signal_connect_data(
                action.cast(),
                c"activate".as_ptr(),
                Some(std::mem::transmute::<
                    unsafe extern "C" fn(*mut GtkAction, gpointer),
                    unsafe extern "C" fn(),
                >(on_activate)),
                composer.cast(),
                None,
                0,
            );
            gtk_action_group_add_action(action_group, action);
            // The group holds its reference now.
            g_object_unref(action.cast());
        }
        refresh_labels(action_group);

        let ui = ui_definition();
        let mut error: *mut GError = ptr::null_mut();
        gtk_ui_manager_add_ui_from_string(ui_manager, ui.as_ptr(), -1, &mut error);
        if !error.is_null() {
            tracing::error!(
                message = ?read_string((*error).message),
                "the Send Later menu could not be merged"
            );
            g_error_free(error);
        }
        gtk_ui_manager_ensure_update(ui_manager);

        let state = Box::new(RefCell::new(SendState {
            link: None,
            generation: 0,
            menu_action,
            action_group,
        }));
        g_object_set_data_full(
            composer.cast(),
            STATE_KEY.as_ptr(),
            Box::into_raw(state).cast(),
            Some(drop_state),
        );
        // The menu action's reference now lives in the state (via the group);
        // the local one from gtk_action_new is the group's, released here.
        g_object_unref(menu_action.cast());

        // Re-gate on every From switch, and once for the identity the
        // composer opened with.
        let table = e_msg_composer_get_header_table(composer);
        g_signal_connect_data(
            table.cast(),
            c"notify::identity-uid".as_ptr(),
            Some(std::mem::transmute::<
                unsafe extern "C" fn(*mut GObject, *mut GParamSpec, gpointer),
                unsafe extern "C" fn(),
            >(on_identity_changed)),
            composer.cast(),
            None,
            0,
        );
        gate(composer.cast());
    });
}

/// # Safety
///
/// GLib's signal machinery; `composer` is the connection's data pointer.
unsafe extern "C" fn on_identity_changed(
    _table: *mut GObject,
    _pspec: *mut GParamSpec,
    composer: gpointer,
) {
    guard("JmapSendLaterExtension::identity-changed", (), || {
        // SAFETY: the composer is alive — its own header table emitted.
        unsafe { gate(composer.cast()) };
    });
}

/// Desensitize with `why`, dropping any stale link.
///
/// # Safety
///
/// `composer` must be a live decorated composer, on the main loop.
unsafe fn refuse_menu(composer: *mut GObject, why: &CStr) {
    let Some(state) = (unsafe { state(composer) }) else {
        return;
    };
    let mut state = state.borrow_mut();
    state.link = None;
    state.generation = GENERATIONS.fetch_add(1, Ordering::SeqCst);
    tracing::trace!(why = ?why, "send-later gate: refused");
    // SAFETY: the action is the group's, alive with the composer; a tooltip
    // is a plain property.
    unsafe {
        gtk_action_set_sensitive(state.menu_action, GFALSE);
        set_tooltip(state.menu_action, translate(why));
    }
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

/// The gate, both levels; runs on the main loop.
///
/// # Safety
///
/// `composer` must be a live decorated composer.
unsafe fn gate(composer: *mut GObject) {
    // Level 1, synchronous. `None` already refused the menu.
    let Some(connect_source) = (unsafe { jmap_connect_source(composer) }) else {
        return;
    };

    // Level 2: ask the server, off the main loop, newest ask wins.
    let generation = GENERATIONS.fetch_add(1, Ordering::SeqCst);
    {
        let Some(state) = (unsafe { state(composer) }) else {
            // SAFETY: releasing the reference level 1 handed over.
            unsafe { g_object_unref(connect_source.cast()) };
            return;
        };
        let mut state = state.borrow_mut();
        state.link = None;
        state.generation = generation;
        // SAFETY: the action and group live with the composer.
        unsafe {
            gtk_action_set_sensitive(state.menu_action, GFALSE);
            set_tooltip(state.menu_action, translate(CHECKING));
            refresh_labels(state.action_group);
        }
    }

    let source = SendSource(connect_source);
    let work = move || {
        let source = source;
        // SAFETY: the reference level 1 took is the worker's for the whole
        // closure, released right after.
        let outcome = unsafe { link::connect_account(source.0) };
        // SAFETY: releasing the worker's reference.
        unsafe { g_object_unref(source.0.cast()) };
        (generation, outcome)
    };
    let finish = |composer: *mut GObject, (generation, outcome)| {
        // SAFETY: `with_strong` hands back the live composer.
        unsafe { apply_gate(composer, generation, outcome) };
    };
    // SAFETY: a live composer per this function's contract.
    unsafe { dispatch::spawn_for(composer, work, finish) };
}

/// Level 1: the source to connect as, when the From identity sends through a
/// JMAP transport — an owned reference to the transport's collection (or the
/// transport itself without one). `None` refused the menu already.
///
/// # Safety
///
/// `composer` must be a live decorated composer, on the main loop.
unsafe fn jmap_connect_source(composer: *mut GObject) -> Option<*mut ESource> {
    // SAFETY: a live composer; every reference taken here is released before
    // return except the one handed to the caller. Registry, header and table
    // are the composer's own (borrowed).
    unsafe {
        let table = e_msg_composer_get_header_table(composer.cast());
        let from_header = e_composer_header_table_get_header(table, E_COMPOSER_HEADER_FROM);
        let registry = if from_header.is_null() {
            ptr::null_mut()
        } else {
            e_composer_header_get_registry(from_header)
        };
        let identity_uid = {
            // The alias out-parameters name which of the identity's aliases
            // the From line shows; the gate cares only whose account it is.
            let raw =
                e_composer_header_table_dup_identity_uid(table, ptr::null_mut(), ptr::null_mut());
            let uid = read_string(raw);
            g_free(raw.cast());
            uid
        };
        let (Some(identity_uid), false) = (identity_uid, registry.is_null()) else {
            refuse_menu(composer, NOT_JMAP);
            return None;
        };

        let identity_uid = CString::new(identity_uid).unwrap_or_default();
        let identity = e_source_registry_ref_source(registry, identity_uid.as_ptr());
        let transport = transport_of(registry, identity);
        if !identity.is_null() {
            g_object_unref(identity.cast());
        }
        let Some(transport) = transport else {
            refuse_menu(composer, NOT_JMAP);
            return None;
        };

        let is_jmap =
            extension_if_present::<ESourceBackend>(transport, E_SOURCE_EXTENSION_MAIL_TRANSPORT)
                .and_then(|backend| read_string(e_source_backend_get_backend_name(backend)))
                .as_deref()
                == BACKEND_NAME.to_str().ok();
        if !is_jmap {
            g_object_unref(transport.cast());
            refuse_menu(composer, NOT_JMAP);
            return None;
        }

        // The connection config lives on the collection when there is one.
        let parent = read_string(e_source_get_parent(transport))
            .and_then(|uid| CString::new(uid).ok())
            .map(|uid| e_source_registry_ref_source(registry, uid.as_ptr()))
            .filter(|source| !source.is_null());
        Some(match parent {
            Some(parent) => {
                g_object_unref(transport.cast());
                parent
            }
            None => transport,
        })
    }
}

/// The identity's transport source, an owned reference — the
/// `[Mail Submission] TransportUid` hop of the evolution-ews chain.
///
/// # Safety
///
/// `registry` must be a live registry; `identity` NULL or a live source.
unsafe fn transport_of(
    registry: *mut ESourceRegistry,
    identity: *mut ESource,
) -> Option<*mut ESource> {
    if identity.is_null() {
        return None;
    }
    // SAFETY: a live identity source; the extension and its string are its own.
    let transport_uid = unsafe {
        extension_if_present::<ESourceMailSubmission>(identity, E_SOURCE_EXTENSION_MAIL_SUBMISSION)
            .and_then(|submission| {
                read_string(e_source_mail_submission_get_transport_uid(submission))
            })
    }?;
    let transport_uid = CString::new(transport_uid).ok()?;
    // SAFETY: a live registry per the contract; the reference is the caller's.
    let transport = unsafe { e_source_registry_ref_source(registry, transport_uid.as_ptr()) };
    (!transport.is_null()).then_some(transport)
}

/// A source reference the worker thread may hold; see `vacation::page` on why
/// this is sound.
struct SendSource(*mut ESource);
// SAFETY: GObject refcounting is thread-safe and ESource getters take the
// source's property lock.
unsafe impl Send for SendSource {}

/// The level-2 answer, applied only if no newer gate superseded it.
///
/// # Safety
///
/// `composer` must be the live composer the gate ran for; main loop.
unsafe fn apply_gate(
    composer: *mut GObject,
    generation: u64,
    outcome: Result<AccountLink, String>,
) {
    let Some(state) = (unsafe { state(composer) }) else {
        return;
    };
    let mut state = state.borrow_mut();
    if state.generation != generation {
        return;
    }
    tracing::trace!(
        max_hold = ?outcome.as_ref().ok().and_then(|link| link.features.max_hold),
        error = ?outcome.as_ref().err(),
        "send-later gate: level 2 answered"
    );
    match outcome {
        Ok(link) if link.features.max_hold.is_some() => {
            // SAFETY: the action lives with the composer.
            unsafe {
                gtk_action_set_sensitive(state.menu_action, GTRUE);
                set_tooltip(state.menu_action, translate(OFFERED));
            }
            state.link = Some(Arc::new(link));
        }
        Ok(_) => {
            // SAFETY: as above.
            unsafe {
                gtk_action_set_sensitive(state.menu_action, GFALSE);
                set_tooltip(state.menu_action, translate(NOT_OFFERED));
            }
        }
        Err(message) => {
            // SAFETY: as above.
            unsafe {
                gtk_action_set_sensitive(state.menu_action, GFALSE);
                set_tooltip(state.menu_action, translate_with(NOT_REACHED, &[&message]));
            }
        }
    }
}

/// Re-label every preset item against the clock now.
///
/// Called at build and on every re-gate. It cannot catch a composer left
/// open across one of its own suggestions — GtkAction has no "about to be
/// shown" hook — which is why [`schedule::resolve`] is asked again at click
/// time: a stale *label* is cosmetic, a stale *target* would schedule into
/// the past.
///
/// # Safety
///
/// `action_group` must be a live `GtkActionGroup` holding this submenu's
/// actions, on the main loop.
unsafe fn refresh_labels(action_group: *mut GtkActionGroup) {
    if action_group.is_null() {
        return;
    }
    for preset in schedule::offered() {
        let Some(occurrence) = schedule::resolve(preset) else {
            continue;
        };
        let name = action_name(preset);
        // SAFETY: a live group per the contract; the action is the group's
        // own (or NULL, which is skipped), and the label is copied by the
        // property machinery.
        unsafe {
            let action = gtk_action_group_get_action(action_group, name.as_ptr());
            if action.is_null() {
                continue;
            }
            let label = CString::new(occurrence.label).unwrap_or_default();
            g_object_set(
                action.cast(),
                c"label".as_ptr(),
                label.as_ptr(),
                ptr::null::<std::ffi::c_char>(),
            );
        }
    }
}

/// The *Custom Time…* entry: ask for a moment, then schedule to it.
///
/// An `EDateEdit` with its time half shown is the whole dialog — the same
/// widget the vacation page uses for dates, which already knows how to parse
/// what a person types and offers a calendar besides.
///
/// # Safety
///
/// `composer` must be a live decorated composer, on the main loop.
unsafe fn activate_custom(composer: *mut GObject) {
    // The limit is the server's, so a dialog is pointless without a link.
    let Some(max_hold) = (unsafe { state(composer) })
        .and_then(|state| state.borrow().link.as_ref()?.features.max_hold)
    else {
        return;
    };

    // SAFETY: a live composer window per the contract; every widget is
    // freshly built and owned by the dialog, which is destroyed below.
    let picked = unsafe {
        let dialog = gtk_dialog_new();
        gtk_window_set_title(dialog.cast(), translate_static(N_(c"Send Later")));
        gtk_window_set_transient_for(dialog.cast(), composer.cast::<GtkWindow>());
        gtk_dialog_add_button(
            dialog.cast(),
            translate_static(N_(c"_Cancel")),
            GTK_RESPONSE_CANCEL,
        );
        gtk_dialog_add_button(
            dialog.cast(),
            translate_static(N_(c"_Send Later")),
            GTK_RESPONSE_OK,
        );

        let field = e_date_edit_new();
        // Both halves: a send time without a time of day is not a time.
        e_date_edit_set_show_time(field.cast(), GTRUE);
        // Seeded an hour out, so the dialog opens on a sane, valid moment
        // rather than on "now", which is already in the past by the time
        // anyone reads it.
        let now = glib_sys::g_date_time_new_now_local();
        if !now.is_null() {
            e_date_edit_set_time(field.cast(), glib_sys::g_date_time_to_unix(now) + 3600);
            glib_sys::g_date_time_unref(now);
        }
        gtk_container_add(gtk_dialog_get_content_area(dialog.cast()).cast(), field);
        gtk_widget_show_all(dialog);

        let response = gtk_dialog_run(dialog.cast());
        let chosen = (response == GTK_RESPONSE_OK).then(|| e_date_edit_get_time(field.cast()));
        gtk_widget_destroy(dialog);
        chosen
    };
    let Some(chosen) = picked else {
        return;
    };

    // SAFETY: no arguments; the object is released immediately.
    let now = unsafe {
        let now = glib_sys::g_date_time_new_now_local();
        if now.is_null() {
            None
        } else {
            let seconds = glib_sys::g_date_time_to_unix(now);
            glib_sys::g_date_time_unref(now);
            Some(seconds)
        }
    };
    let Some(now) = now else {
        // SAFETY: a live composer window per this function's contract.
        unsafe { refuse_send(composer, translate(NO_CLOCK)) };
        return;
    };

    // `e_date_edit_get_time` answers -1 for "no date set", which lands in
    // the past like any other stale choice, so one check covers both.
    let hold = chosen - now;
    if hold <= 0 {
        // SAFETY: a live composer window.
        unsafe { refuse_send(composer, translate(NOT_FUTURE)) };
        return;
    }
    let hold = hold as u64;
    if hold > max_hold {
        // SAFETY: as above.
        unsafe { refuse_send(composer, translate(TOO_FAR)) };
        return;
    }
    // SAFETY: as above.
    unsafe { schedule_hold(composer, hold) };
}

/// The one `activate` handler for the whole submenu: which moment was
/// clicked is recovered from the action's own name, so adding a preset is a
/// line in [`schedule::offered`] rather than another trampoline.
///
/// # Safety
///
/// GLib's signal machinery; `composer` is the connection's data pointer.
unsafe extern "C" fn on_activate(action: *mut GtkAction, composer: gpointer) {
    guard("JmapSendLaterExtension::activate", (), || {
        // SAFETY: a live action GLib is emitting on; the name is its own.
        let name = unsafe { read_string(gtk_action_get_name(action)) };
        let Some(slug) = name
            .as_deref()
            .and_then(|name| name.strip_prefix("jmap-send-later-"))
        else {
            return;
        };
        tracing::trace!(slug, "send-later: menu item activated");
        // SAFETY: the composer is alive — its own menu emitted.
        unsafe {
            if slug == "custom" {
                activate_custom(composer.cast());
            } else if let Some(preset) = schedule::from_slug(slug) {
                activate(composer.cast(), preset);
            }
        }
    });
}

/// What one click carries into the async message build.
struct PendingSchedule {
    link: Arc<AccountLink>,
    hold: u64,
}

/// A preset click. The moment is resolved *here* rather than read off the
/// menu label, so a composer left open past its own suggestion schedules to
/// the next occurrence instead of into the past.
///
/// # Safety
///
/// `composer` must be a live decorated composer, on the main loop.
unsafe fn activate(composer: *mut GObject, preset: Preset) {
    let Some(occurrence) = schedule::resolve(preset) else {
        // SAFETY: a live composer per this function's contract.
        unsafe { refuse_send(composer, translate(NO_CLOCK)) };
        return;
    };
    // SAFETY: as above.
    unsafe { schedule_hold(composer, occurrence.hold) };
}

/// Hold for `hold` seconds: check it against the server's limit, then ask the
/// composer for its message the way Send itself does. Shared by the presets
/// and by the custom-time dialog, which has already done its own bounds
/// check but is re-checked here so one place owns the rule.
///
/// # Safety
///
/// `composer` must be a live decorated composer, on the main loop.
unsafe fn schedule_hold(composer: *mut GObject, hold: u64) {
    let Some((link, max_hold)) = (unsafe { state(composer) }).and_then(|state| {
        let state = state.borrow();
        let link = state.link.clone()?;
        let max_hold = link.features.max_hold?;
        Some((link, max_hold))
    }) else {
        return; // the menu was insensitive; a race clicked it anyway
    };
    if hold > max_hold {
        // SAFETY: a live composer per this function's contract.
        unsafe { refuse_send(composer, translate(TOO_FAR)) };
        return;
    }

    let pending = Box::new(PendingSchedule { link, hold });
    // SAFETY: a live composer; the box travels through GIO to `got_message`,
    // which reclaims it exactly once.
    unsafe {
        e_msg_composer_get_message(
            composer.cast(),
            glib_sys::G_PRIORITY_DEFAULT,
            ptr::null_mut(),
            Some(got_message),
            Box::into_raw(pending).cast(),
        );
    }
}

/// The composer's finished message: strip the editor's bookkeeping headers,
/// write it out, read the envelope off it, and hand everything to the worker.
///
/// # Safety
///
/// GIO calls this with the composer as `source_object` and the box `activate`
/// leaked as `user_data`.
unsafe extern "C" fn got_message(
    source_object: *mut GObject,
    result: *mut GAsyncResult,
    user_data: gpointer,
) {
    guard("JmapSendLaterExtension::got-message", (), || {
        // SAFETY: reclaiming the box `activate` leaked, exactly once.
        let pending = unsafe { Box::from_raw(user_data.cast::<PendingSchedule>()) };
        let composer = source_object;

        // SAFETY: the composer and result GIO handed over; the message comes
        // back owned (transfer full), the error likewise, both released below.
        let (written, envelope) = unsafe {
            let mut error: *mut GError = ptr::null_mut();
            let message = e_msg_composer_get_message_finish(composer.cast(), result, &mut error);
            if message.is_null() {
                let why = if error.is_null() {
                    translate(c"no further detail was given")
                } else {
                    let why = read_string((*error).message)
                        .unwrap_or_else(|| translate(c"no further detail was given"));
                    g_error_free(error);
                    why
                };
                refuse_send(composer, translate_with(REFUSED, &[&why]));
                return;
            }
            strip_evolution_headers(message);
            let written = write_message(message);
            let envelope = envelope_of(message);
            g_object_unref(message.cast());
            (written, envelope)
        };

        let bytes = match written {
            Ok(bytes) => bytes,
            Err(unwritable) => {
                // SAFETY: a fresh GError this arm owns; message copied, freed.
                let why = unsafe {
                    let gerror = unwritable.into_gerror(gio_sys::g_io_error_quark(), 0);
                    let why = read_string((*gerror).message)
                        .unwrap_or_else(|| translate(c"no further detail was given"));
                    g_error_free(gerror);
                    why
                };
                // SAFETY: a live composer window, per GIO's source_object.
                unsafe { refuse_send(composer, translate_with(REFUSED, &[&why])) };
                return;
            }
        };
        let envelope = match envelope {
            Ok(envelope) => envelope,
            Err(why) => {
                // SAFETY: as above.
                unsafe { refuse_send(composer, translate(why)) };
                return;
            }
        };

        let PendingSchedule { link, hold } = *pending;
        let work = move || submit::schedule_send(&link, bytes, envelope, hold);
        let finish = |composer: *mut GObject, outcome: Result<String, String>| match outcome {
            Ok(send_at) => {
                tracing::debug!(send_at, "message scheduled; closing the composer");
                // SAFETY: `with_strong` hands back the live composer window,
                // and destroying it is what Evolution's own send success does.
                unsafe { gtk_widget_destroy(composer.cast()) };
            }
            // SAFETY: as above, still alive and shown.
            Err(why) => unsafe { refuse_send(composer, translate_with(REFUSED, &[&why])) },
        };
        // SAFETY: a live composer window, per GIO's source_object.
        unsafe { dispatch::spawn_for(composer, work, finish) };
    });
}

/// The composer writes `X-Evolution-*` bookkeeping headers (identity,
/// transport, Fcc) into the built message; none of them belongs on a server.
///
/// # Safety
///
/// `message` must be a live `CamelMimeMessage`.
unsafe fn strip_evolution_headers(message: *mut CamelMimeMessage) {
    // SAFETY: a live message per the contract; the dup'd array is freed, the
    // names are copied before any removal mutates the header list.
    unsafe {
        let medium = message.cast::<CamelMedium>();
        let headers = camel_medium_dup_headers(medium);
        if headers.is_null() {
            return;
        }
        let mut doomed = Vec::new();
        for index in 0..camel_name_value_array_get_length(headers) {
            let mut name: *const std::ffi::c_char = ptr::null();
            let mut value: *const std::ffi::c_char = ptr::null();
            if camel_name_value_array_get(headers, index, &mut name, &mut value) != GFALSE
                && let Some(name) = read_string(name)
                && name.to_ascii_lowercase().starts_with("x-evolution")
                && let Ok(name) = CString::new(name)
            {
                doomed.push(name);
            }
        }
        camel_name_value_array_free(headers);
        for name in doomed {
            camel_medium_remove_header(medium, name.as_ptr());
        }
    }
}

/// The SMTP envelope off the message's own headers: From as `mailFrom`, every
/// To/Cc/Bcc entry as a recipient — the same set the ordinary transport would
/// send to.
///
/// # Safety
///
/// `message` must be a live `CamelMimeMessage`.
unsafe fn envelope_of(message: *mut CamelMimeMessage) -> Result<Envelope, &'static CStr> {
    // SAFETY: a live message; the address objects and their strings are the
    // message's own, read before it is released.
    unsafe {
        let from = camel_mime_message_get_from(message);
        let mail_from = first_address(from).ok_or(NO_FROM)?;

        let mut recipients = Vec::new();
        for kind in [c"To", c"Cc", c"Bcc"] {
            let list = camel_mime_message_get_recipients(message, kind.as_ptr());
            if list.is_null() {
                continue;
            }
            for index in 0..camel_address_length(list.cast::<CamelAddress>()) {
                let mut name: *const std::ffi::c_char = ptr::null();
                let mut address: *const std::ffi::c_char = ptr::null();
                if camel_internet_address_get(list, index, &mut name, &mut address) != GFALSE
                    && let Some(address) = read_string(address)
                {
                    recipients.push(EnvelopeAddress::new(address));
                }
            }
        }
        if recipients.is_empty() {
            return Err(NO_RECIPIENTS);
        }
        Ok(Envelope::new(EnvelopeAddress::new(mail_from), recipients))
    }
}

/// The first address in `list`, when there is one.
///
/// # Safety
///
/// `list` must be NULL or a live `CamelInternetAddress`.
unsafe fn first_address(list: *mut CamelInternetAddress) -> Option<String> {
    if list.is_null() {
        return None;
    }
    let mut name: *const std::ffi::c_char = ptr::null();
    let mut address: *const std::ffi::c_char = ptr::null();
    // SAFETY: a live list per the contract; the strings are its own, copied.
    unsafe {
        (camel_internet_address_get(list, 0, &mut name, &mut address) != GFALSE)
            .then(|| read_string(address))
            .flatten()
    }
}

/// A modal explanation on the composer, which stays open.
///
/// # Safety
///
/// `composer` must be a live composer window, on the main loop.
unsafe fn refuse_send(composer: *mut GObject, text: String) {
    let text = CString::new(text).unwrap_or_default();
    // SAFETY: a live window per the contract; the dialog is run and destroyed
    // before this returns, and "%s" keeps the text out of printf's hands.
    unsafe {
        let dialog = gtk_message_dialog_new(
            composer.cast::<GtkWindow>(),
            GTK_DIALOG_MODAL | GTK_DIALOG_DESTROY_WITH_PARENT,
            GTK_MESSAGE_ERROR,
            GTK_BUTTONS_CLOSE,
            c"%s".as_ptr(),
            text.as_ptr(),
        );
        gtk_dialog_run(dialog.cast());
        gtk_widget_destroy(dialog);
    }
}
