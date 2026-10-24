// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapSendLaterExtension`: the `EExtension` on `EMsgComposer`. It adds the
//! *File ▸ Send Later* submenu and gates it on the From account. It sends a
//! click through the message builder of the composer to
//! [`crate::send_later::submit::schedule_send`].
//!
//! `evolution_eui_manager` selects one of two menu layers at compile time
//! (see `evo-sys/build.rs`):
//! - Evolution < 3.55 uses `GtkUIManager` and the "core" `GtkActionGroup` of
//!   the editor.
//! - Evolution >= 3.55 uses `EUIManager`. It registers the actions under a
//!   group name of this extension, not under an existing group. Upstream
//!   `e-composer-to-meeting.c` registers its own "composer" group for this
//!   extensible in the same way. On this layer, one call
//!   (`e_ui_manager_add_actions_with_eui_data`) makes the actions and adds
//!   the `.eui` fragment. The GTK layer needs two steps for this.
//!
//! The gate runs again at each From change (`notify::identity-uid` of
//! `EComposerHeaderTable`). Level 1 is the transport backend name of the
//! identity. It follows the evolution-ews chain: the identity source, then
//! `[Mail Submission]` `TransportUid`, then the backend name of the transport
//! source. Level 2 is a session fetch off the main loop. Its answer comes
//! back through [`crate::dispatch`], and a generation counter guards it. Thus
//! a slow fetch for the previous account cannot make the menu sensitive for
//! the current account.
//!
//! On success, the extension destroys the composer window, as the usual send
//! path of Evolution does. On refusal, a modal message dialog tells why. The
//! message goes to Drafts first, thus the dialog also tells that a copy can
//! remain there.

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use eds_sys::{
    CamelAddress, CamelInternetAddress, CamelMedium, CamelMimeMessage,
    E_SOURCE_EXTENSION_MAIL_SUBMISSION, E_SOURCE_EXTENSION_MAIL_TRANSPORT, EExtension, ESource,
    ESourceBackend, ESourceMailSubmission, ESourceRegistry, camel_address_length,
    camel_internet_address_get, camel_medium_dup_headers, camel_medium_remove_header,
    camel_mime_message_get_from, camel_mime_message_get_recipients, camel_name_value_array_free,
    camel_name_value_array_get, camel_name_value_array_get_length, e_extension_get_extensible,
    e_source_backend_dup_backend_name, e_source_dup_parent,
    e_source_mail_submission_dup_transport_uid, e_source_registry_ref_source,
};
use evo_sys::{
    E_COMPOSER_HEADER_FROM, EHTMLEditor, EMsgComposer, GTK_BUTTONS_CLOSE,
    GTK_DIALOG_DESTROY_WITH_PARENT, GTK_DIALOG_MODAL, GTK_MESSAGE_ERROR, GTK_RESPONSE_CANCEL,
    GTK_RESPONSE_OK, GtkWindow, e_composer_header_get_registry,
    e_composer_header_table_dup_identity_uid, e_composer_header_table_get_header,
    e_date_edit_get_time, e_date_edit_new, e_date_edit_set_show_time, e_date_edit_set_time,
    e_html_editor_get_ui_manager, e_msg_composer_get_editor, e_msg_composer_get_header_table,
    e_msg_composer_get_message, e_msg_composer_get_message_finish, e_msg_composer_get_type,
    gtk_container_add, gtk_dialog_add_button, gtk_dialog_get_content_area, gtk_dialog_new,
    gtk_dialog_run, gtk_message_dialog_new, gtk_widget_destroy, gtk_widget_show_all,
    gtk_window_set_title, gtk_window_set_transient_for,
};
#[cfg(evolution_eui_manager)]
use evo_sys::{
    EUIAction, EUIActionEntry, EUIActionGroup, GVariant, e_ui_action_group_get_action,
    e_ui_action_set_label, e_ui_action_set_sensitive, e_ui_action_set_tooltip,
    e_ui_manager_add_actions_with_eui_data, e_ui_manager_get_action_group, g_action_get_name,
};
#[cfg(not(evolution_eui_manager))]
use evo_sys::{
    GtkAction, GtkActionGroup, e_html_editor_get_action_group, gtk_action_get_name,
    gtk_action_group_add_action, gtk_action_group_get_action, gtk_action_new,
    gtk_action_set_sensitive, gtk_ui_manager_add_ui_from_string, gtk_ui_manager_ensure_update,
};
use gio_sys::GAsyncResult;
use glib_sys::{GError, GFALSE, GTRUE, GType, g_error_free, g_free, gpointer};
#[cfg(not(evolution_eui_manager))]
use gobject_sys::g_object_set;
use gobject_sys::{GObject, GParamSpec, g_object_unref, g_signal_connect_data};
use jmap_backend_core::BACKEND_NAME;
use jmap_backend_core::callback::gcallback;
use jmap_backend_core::extension::{Extension, ExtensionObject};
use jmap_backend_core::i18n::{N_, translate, translate_static, translate_with};
use jmap_backend_core::marshal::{extension_if_present, read_string, take_string};
use jmap_backend_core::mime::write_message;
use jmap_backend_core::object_data::ObjectData;
use jmap_backend_core::trampoline::guard;
use jmap_proto::mail::{Envelope, EnvelopeAddress};

use crate::dispatch;
use crate::link::{self, AccountLink, SendSource};
use crate::send_later::schedule::{self, Preset};
use crate::send_later::submit;

const STATE: ObjectData<RefCell<SendState>> = ObjectData::new(c"jmap-send-later-state");

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

/// The action of the submenu, for the compiled menu layer.
#[cfg(not(evolution_eui_manager))]
type MenuAction = *mut GtkAction;
#[cfg(evolution_eui_manager)]
type MenuAction = *mut EUIAction;

/// The group that holds the preset actions. The code uses it to update their
/// labels later: "Today, 18:00" is correct only until 18:00.
#[cfg(not(evolution_eui_manager))]
type MenuActionGroup = *mut GtkActionGroup;
#[cfg(evolution_eui_manager)]
type MenuActionGroup = *mut EUIActionGroup;

/// The state of one composer, kept in a box as qdata on the composer.
struct SendState {
    /// The connected account of the current From identity, after the level-2
    /// fetch answers. A click submits through this account.
    link: Option<Arc<AccountLink>>,
    /// Protects against a slow fetch after a From change: only the newest
    /// fetch applies.
    generation: u64,
    /// The action of the submenu. Its sensitivity and tooltip are the visible
    /// part of the gate.
    menu_action: MenuAction,
    /// The group that holds the preset actions, used to update their labels.
    action_group: MenuActionGroup,
}

static GENERATIONS: AtomicU64 = AtomicU64::new(1);

/// The [`SendState`] of the composer, while the composer is alive.
///
/// # Safety
///
/// `composer` must be a live composer that this extension decorated. The
/// reference must not live longer than one main-loop callback.
unsafe fn state<'a>(composer: *mut GObject) -> Option<&'a RefCell<SendState>> {
    // SAFETY: per the contract. `constructed` sets the data one time.
    unsafe { STATE.get(composer) }
}

/// The Send Later menu of the composer.
pub struct SendLater;

/// The registered `JmapSendLaterExtension` type.
pub type JmapSendLaterExtension = ExtensionObject<SendLater>;

impl Extension for SendLater {
    const TYPE_NAME: &'static CStr = c"JmapSendLaterExtension";
    const EXTENSIBLE_NAME: &'static CStr = c"EMsgComposer";

    fn extensible_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_msg_composer_get_type() }
    }

    unsafe fn constructed(extension: *mut EExtension) {
        // SAFETY: the contract of this method.
        unsafe { constructed(extension) }
    }
}

/// The action name of the menu item for one preset.
fn action_name(preset: Preset) -> CString {
    CString::new(format!("jmap-send-later-{}", schedule::slug(preset))).unwrap_or_default()
}

/// The XML of the submenu: one item for each offered moment, plus the custom
/// item. The function builds it, not a constant, because
/// [`schedule::offered`] sets which moments to offer.
///
/// 3.52 merges into the `pre-edit-menu`/`custom-actions-placeholder`
/// placeholders of the `<menubar name='main-menu'>` root. The composer
/// extensions of evolution-ews use the same pair. 3.55+ merges at the same
/// position, under `<eui><menu id='main-menu'>`, with `id` placeholders and
/// `<item>` in place of `<menuitem>`. A check against the in-tree composer
/// extension `e-composer-to-meeting.c` confirms this structure. That file
/// puts one `<item>` where this file puts a full `<submenu>`.
#[cfg(not(evolution_eui_manager))]
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
#[cfg(evolution_eui_manager)]
fn ui_definition() -> CString {
    let mut items = String::new();
    for preset in schedule::offered() {
        items.push_str(&format!(
            "<item action='jmap-send-later-{}'/>",
            schedule::slug(preset)
        ));
    }
    items.push_str("<separator/><item action='jmap-send-later-custom'/>");
    CString::new(format!(
        "<eui>\
<menu id='main-menu'>\
<placeholder id='pre-edit-menu'><submenu action='file-menu'>\
<placeholder id='custom-actions-placeholder'>\
<submenu action='jmap-send-later-menu'>{items}</submenu>\
</placeholder></submenu></placeholder>\
</menu>\
</eui>"
    ))
    .unwrap_or_default()
}

/// The group name of this extension under `EUIManager`. In the same way,
/// `e-composer-to-meeting.c` registers its own "composer" group for this
/// extensible and does not use an existing group.
#[cfg(evolution_eui_manager)]
const GROUP_NAME: &CStr = c"jmap-send-later";

/// Builds the action of the submenu and the group for its presets, through
/// the compiled menu layer. 3.52 uses the existing "core" group and a
/// `gtk_ui_manager_add_ui_from_string` merge. 3.55+ uses a new group, and
/// one `e_ui_manager_add_actions_with_eui_data` call does both steps.
///
/// # Safety
///
/// `composer` must be a live composer; `editor` must be its editor.
#[cfg(not(evolution_eui_manager))]
unsafe fn build_menu(
    composer: *mut EMsgComposer,
    editor: *mut EHTMLEditor,
) -> Option<(MenuAction, MenuActionGroup)> {
    // SAFETY: the contract of this function makes the editor live.
    let ui_manager = unsafe { e_html_editor_get_ui_manager(editor) };
    // SAFETY: as above.
    let action_group = unsafe { e_html_editor_get_action_group(editor, c"core".as_ptr()) };
    if ui_manager.is_null() || action_group.is_null() {
        return None;
    }

    // SAFETY: the objects are live (checked above). The reference of each
    // action goes to the group.
    unsafe {
        // The submenu action has only a label; its items have the handler.
        let menu_action = gtk_action_new(
            c"jmap-send-later-menu".as_ptr(),
            translate_static(N_(c"Send _Later")),
            translate_static(CHECKING),
            ptr::null(),
        );
        gtk_action_set_sensitive(menu_action, GFALSE);
        gtk_action_group_add_action(action_group, menu_action);

        // One action for each offered moment, plus the custom item. All use
        // `on_activate`, which gets the moment from the action name.
        // `refresh_labels` sets the labels below and at each gate run,
        // because the meaning of "Today, 18:00" changes.
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
            // SAFETY: the composer lives longer than the actions of its menu
            // (the editor owns the group, and the composer owns the editor).
            // Thus a plain pointer is safe connection data, as in evolution-ews.
            g_signal_connect_data(
                action.cast(),
                c"activate".as_ptr(),
                gcallback(on_activate as unsafe extern "C" fn(*mut GtkAction, gpointer)),
                composer.cast(),
                None,
                0,
            );
            gtk_action_group_add_action(action_group, action);
            // The group holds its own reference now.
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
        Some((menu_action, action_group))
    }
}

/// # Safety
///
/// `composer` must be a live composer; `editor` must be its editor.
#[cfg(evolution_eui_manager)]
unsafe fn build_menu(
    composer: *mut EMsgComposer,
    editor: *mut EHTMLEditor,
) -> Option<(MenuAction, MenuActionGroup)> {
    // SAFETY: the contract of this function makes the editor live.
    let ui_manager = unsafe { e_html_editor_get_ui_manager(editor) };
    if ui_manager.is_null() {
        return None;
    }

    let mut names: Vec<CString> = schedule::offered()
        .iter()
        .copied()
        .map(action_name)
        .collect();
    names.push(CString::new("jmap-send-later-custom").unwrap_or_default());

    let mut entries: Vec<EUIActionEntry> = Vec::with_capacity(1 + names.len());
    entries.push(EUIActionEntry {
        name: c"jmap-send-later-menu".as_ptr(),
        icon_name: ptr::null(),
        label: translate_static(N_(c"Send _Later")),
        accel: ptr::null(),
        tooltip: translate_static(CHECKING),
        activate: None,
        parameter_type: ptr::null(),
        state: ptr::null(),
        change_state: None,
    });
    // One action for each offered moment, plus the custom item. All use
    // `on_activate`, which gets the moment from the action name.
    for name in &names {
        let is_custom = name.to_bytes().ends_with(b"-custom");
        entries.push(EUIActionEntry {
            name: name.as_ptr(),
            icon_name: ptr::null(),
            // `e_ui_action_group_add` warns about a NULL label. It runs inside
            // `e_ui_manager_add_actions_with_eui_data`, before `refresh_labels`
            // sets the real label. The menu never shows this placeholder.
            label: if is_custom {
                translate_static(N_(c"_Custom Time…"))
            } else {
                translate_static(N_(c"…"))
            },
            accel: ptr::null(),
            tooltip: ptr::null(),
            activate: Some(on_activate),
            parameter_type: ptr::null(),
            state: ptr::null(),
            change_state: None,
        });
    }

    let ui = ui_definition();
    // SAFETY: the manager is live. The pointers in `entries` are 'static or
    // process-lifetime strings from `translate_static`. Thus the array must
    // stay alive only during this call.
    unsafe {
        e_ui_manager_add_actions_with_eui_data(
            ui_manager,
            GROUP_NAME.as_ptr(),
            ptr::null(),
            entries.as_ptr(),
            entries.len() as std::ffi::c_int,
            composer.cast(),
            ui.as_ptr(),
        );
    }

    // SAFETY: the manager is live.
    let action_group = unsafe { e_ui_manager_get_action_group(ui_manager, GROUP_NAME.as_ptr()) };
    if action_group.is_null() {
        tracing::error!("the Send Later menu could not be merged");
        return None;
    }
    // SAFETY: the group is live; the code found it immediately before.
    let menu_action =
        unsafe { e_ui_action_group_get_action(action_group, c"jmap-send-later-menu".as_ptr()) };
    if menu_action.is_null() {
        return None;
    }
    // SAFETY: the action is live; the code found it immediately before.
    unsafe { set_action_sensitive(menu_action, false) };
    // SAFETY: the group is live; the code found it immediately before.
    unsafe { refresh_labels(action_group) };
    Some((menu_action, action_group))
}

/// Adds the menu (insensitive) and starts the gate.
unsafe fn constructed(extension: *mut EExtension) {
    unsafe {
        // SAFETY: `extension` is live. The extensible is the
        // composer for which GObject made this extension.
        let composer: *mut EMsgComposer = e_extension_get_extensible(extension).cast();
        let editor = e_msg_composer_get_editor(composer);
        let Some((menu_action, action_group)) = build_menu(composer, editor) else {
            return;
        };

        let state = RefCell::new(SendState {
            link: None,
            generation: 0,
            menu_action,
            action_group,
        });
        STATE.set(composer.cast(), state);
        // 3.52 only: the group holds a reference to the menu action. This
        // releases the local reference from `gtk_action_new`. On 3.55+,
        // `menu_action` is borrowed from the group, thus nothing is released.
        #[cfg(not(evolution_eui_manager))]
        g_object_unref(menu_action.cast());

        // Run the gate at each From change, and one time now for the first
        // identity.
        let table = e_msg_composer_get_header_table(composer);
        g_signal_connect_data(
            table.cast(),
            c"notify::identity-uid".as_ptr(),
            gcallback(
                on_identity_changed
                    as unsafe extern "C" fn(*mut GObject, *mut GParamSpec, gpointer),
            ),
            composer.cast(),
            None,
            0,
        );
        gate(composer.cast());
    }
}

/// # Safety
///
/// The GLib signal system calls this; `composer` is the data pointer of the
/// connection.
unsafe extern "C" fn on_identity_changed(
    _table: *mut GObject,
    _pspec: *mut GParamSpec,
    composer: gpointer,
) {
    guard("JmapSendLaterExtension::identity-changed", (), || {
        // SAFETY: the composer is alive, because its header table emitted the
        // signal.
        unsafe { gate(composer.cast()) };
    });
}

/// Makes the menu insensitive with `why` as the tooltip, and removes any old
/// link.
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
    // SAFETY: the group owns the action, which lives as long as the composer.
    // A tooltip is a plain property.
    unsafe {
        set_action_sensitive(state.menu_action, false);
        set_tooltip(state.menu_action, translate(why));
    }
}

/// # Safety
///
/// `action` must be a live action of the compiled menu layer.
#[cfg(not(evolution_eui_manager))]
unsafe fn set_tooltip(action: MenuAction, text: String) {
    let text = CString::new(text).unwrap_or_default();
    // SAFETY: the contract makes the action live; the property system copies
    // the string.
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
    // SAFETY: the contract makes the action live; the setter copies the string.
    unsafe { e_ui_action_set_tooltip(action, text.as_ptr()) };
}

/// # Safety
///
/// `action` must be a live action of the compiled menu layer.
#[cfg(not(evolution_eui_manager))]
unsafe fn set_action_sensitive(action: MenuAction, sensitive: bool) {
    // SAFETY: the contract makes the action live.
    unsafe { gtk_action_set_sensitive(action, if sensitive { GTRUE } else { GFALSE }) };
}
#[cfg(evolution_eui_manager)]
unsafe fn set_action_sensitive(action: MenuAction, sensitive: bool) {
    // SAFETY: the contract makes the action live.
    unsafe { e_ui_action_set_sensitive(action, if sensitive { GTRUE } else { GFALSE }) };
}

/// The gate, both levels. It runs on the main loop.
///
/// # Safety
///
/// `composer` must be a live decorated composer.
unsafe fn gate(composer: *mut GObject) {
    // Level 1 is synchronous. On `None`, it has already refused the menu.
    let Some(connect_source) = (unsafe { jmap_connect_source(composer) }) else {
        return;
    };

    // Level 2: ask the server off the main loop. The newest request wins.
    let generation = GENERATIONS.fetch_add(1, Ordering::SeqCst);
    {
        let Some(state) = (unsafe { state(composer) }) else {
            // SAFETY: this releases the reference that level 1 gave.
            unsafe { g_object_unref(connect_source.cast()) };
            return;
        };
        let mut state = state.borrow_mut();
        state.link = None;
        state.generation = generation;
        // SAFETY: the action and the group live as long as the composer.
        unsafe {
            set_action_sensitive(state.menu_action, false);
            set_tooltip(state.menu_action, translate(CHECKING));
            refresh_labels(state.action_group);
        }
    }

    // SAFETY: this takes the reference that level 1 gave.
    let source = unsafe { SendSource::adopt(connect_source) };
    let work = move || {
        // SAFETY: the worker owns the level-1 reference during the closure
        // and releases it immediately after this call.
        let outcome = unsafe { link::connect_account(source.as_ptr()) };
        drop(source);
        (generation, outcome)
    };
    let finish = |composer: *mut GObject, (generation, outcome)| {
        // SAFETY: `with_strong` gives back the live composer.
        unsafe { apply_gate(composer, generation, outcome) };
    };
    // SAFETY: the contract of this function makes the composer live.
    unsafe { dispatch::spawn_for(composer, work, finish) };
}

/// Level 1: the source to connect as, if the From identity sends through a
/// JMAP transport. The result is an owned reference to the collection of the
/// transport, or to the transport if it has no collection. On `None`, this
/// function has already refused the menu.
///
/// # Safety
///
/// `composer` must be a live decorated composer, on the main loop.
unsafe fn jmap_connect_source(composer: *mut GObject) -> Option<*mut ESource> {
    // SAFETY: the composer is live. The code releases each reference that it
    // takes, except the reference for the caller. The registry, the header,
    // and the table belong to the composer (borrowed).
    unsafe {
        let table = e_msg_composer_get_header_table(composer.cast());
        let from_header = e_composer_header_table_get_header(table, E_COMPOSER_HEADER_FROM);
        let registry = if from_header.is_null() {
            ptr::null_mut()
        } else {
            e_composer_header_get_registry(from_header)
        };
        let identity_uid = {
            // The alias out-parameters tell which alias the From line shows.
            // The gate needs only the account.
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
                .and_then(|backend| take_string(e_source_backend_dup_backend_name(backend)))
                .as_deref()
                == BACKEND_NAME.to_str().ok();
        if !is_jmap {
            g_object_unref(transport.cast());
            refuse_menu(composer, NOT_JMAP);
            return None;
        }

        // The connection configuration is on the collection, if one exists.
        let parent = take_string(e_source_dup_parent(transport))
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

/// The transport source of the identity, as an owned reference: the
/// `[Mail Submission] TransportUid` step of the evolution-ews chain.
///
/// # Safety
///
/// `registry` must be a live registry; `identity` must be NULL or a live
/// source.
unsafe fn transport_of(
    registry: *mut ESourceRegistry,
    identity: *mut ESource,
) -> Option<*mut ESource> {
    if identity.is_null() {
        return None;
    }
    // SAFETY: the identity source is live, and the extension belongs to it.
    // The copy of the string belongs to this function.
    let transport_uid = unsafe {
        extension_if_present::<ESourceMailSubmission>(identity, E_SOURCE_EXTENSION_MAIL_SUBMISSION)
            .and_then(|submission| {
                take_string(e_source_mail_submission_dup_transport_uid(submission))
            })
    }?;
    let transport_uid = CString::new(transport_uid).ok()?;
    // SAFETY: the contract makes the registry live; the caller owns the
    // reference.
    let transport = unsafe { e_source_registry_ref_source(registry, transport_uid.as_ptr()) };
    (!transport.is_null()).then_some(transport)
}

/// Applies the level-2 answer, but only if no newer gate run replaced it.
///
/// # Safety
///
/// `composer` must be the live composer for which the gate ran, on the main
/// loop.
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
            // SAFETY: the action lives as long as the composer.
            unsafe {
                set_action_sensitive(state.menu_action, true);
                set_tooltip(state.menu_action, translate(OFFERED));
            }
            state.link = Some(Arc::new(link));
        }
        Ok(_) => {
            // SAFETY: as above.
            unsafe {
                set_action_sensitive(state.menu_action, false);
                set_tooltip(state.menu_action, translate(NOT_OFFERED));
            }
        }
        Err(message) => {
            // SAFETY: as above.
            unsafe {
                set_action_sensitive(state.menu_action, false);
                set_tooltip(state.menu_action, translate_with(NOT_REACHED, &[&message]));
            }
        }
    }
}

/// Sets the label of each preset item from the current clock.
///
/// The menu build and each gate run call this function. It cannot update a
/// composer that stays open past one of its suggestions, because GtkAction
/// has no "about to show" hook. Thus [`schedule::resolve`] runs again at
/// click time. A stale *label* is only cosmetic. A stale *target* would
/// schedule into the past.
///
/// # Safety
///
/// `action_group` must be a live `GtkActionGroup` that holds the actions of
/// this submenu, on the main loop.
#[cfg(not(evolution_eui_manager))]
unsafe fn refresh_labels(action_group: MenuActionGroup) {
    if action_group.is_null() {
        return;
    }
    for preset in schedule::offered() {
        let Some(occurrence) = schedule::resolve(preset) else {
            continue;
        };
        let name = action_name(preset);
        // SAFETY: the contract makes the group live. The group owns the action
        // (the loop skips NULL), and the property system copies the label.
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
/// # Safety
///
/// `action_group` must be a live `EUIActionGroup` that holds the actions of
/// this submenu, on the main loop.
#[cfg(evolution_eui_manager)]
unsafe fn refresh_labels(action_group: MenuActionGroup) {
    if action_group.is_null() {
        return;
    }
    for preset in schedule::offered() {
        let Some(occurrence) = schedule::resolve(preset) else {
            continue;
        };
        let name = action_name(preset);
        // SAFETY: the contract makes the group live. The group owns the action
        // (the loop skips NULL), and the setter copies the label.
        unsafe {
            let action = e_ui_action_group_get_action(action_group, name.as_ptr());
            if action.is_null() {
                continue;
            }
            let label = CString::new(occurrence.label).unwrap_or_default();
            e_ui_action_set_label(action, label.as_ptr());
        }
    }
}

/// The *Custom Time…* item: ask for a moment, then schedule the message for
/// it.
///
/// The dialog contains only an `EDateEdit` that shows its time part. The
/// vacation page uses the same widget for dates. It parses typed text and
/// also shows a calendar.
///
/// # Safety
///
/// `composer` must be a live decorated composer, on the main loop.
unsafe fn activate_custom(composer: *mut GObject) {
    // The server sets the limit, thus the dialog is not necessary without a
    // link.
    let Some(max_hold) = (unsafe { state(composer) })
        .and_then(|state| state.borrow().link.as_ref()?.features.max_hold)
    else {
        return;
    };

    // SAFETY: the contract makes the composer window live. Each widget is new,
    // and the dialog owns it. The code destroys the dialog below.
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
        // Show the time part too: a send time needs a time of day.
        e_date_edit_set_show_time(field.cast(), GTRUE);
        // Start one hour ahead, not at "now". "Now" is already in the past
        // when the user reads it.
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

    // SAFETY: no arguments; the code releases the object immediately.
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
        // SAFETY: the contract of this function makes the composer window live.
        unsafe { refuse_send(composer, translate(NO_CLOCK)) };
        return;
    };

    // `e_date_edit_get_time` returns -1 for "no date set". That value is in
    // the past, thus one check covers both cases.
    let hold = chosen - now;
    if hold <= 0 {
        // SAFETY: the composer window is live.
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

/// The `activate` handler of all submenu items. It reads the action name and
/// gives it to [`dispatch_activation`].
///
/// # Safety
///
/// The GLib signal system calls this; `composer` is the data pointer of the
/// connection.
#[cfg(not(evolution_eui_manager))]
unsafe extern "C" fn on_activate(action: *mut GtkAction, composer: gpointer) {
    guard("JmapSendLaterExtension::activate", (), || {
        // SAFETY: GLib emits on this live action; the name belongs to the
        // action.
        let name = unsafe { read_string(gtk_action_get_name(action)) };
        dispatch_activation(name.as_deref(), composer);
    });
}
#[cfg(evolution_eui_manager)]
unsafe extern "C" fn on_activate(
    action: *mut EUIAction,
    _value: *mut GVariant,
    composer: gpointer,
) {
    guard("JmapSendLaterExtension::activate", (), || {
        // SAFETY: GLib emits on this live action. `EUIAction` implements
        // `GAction`, and the `GAction` API gives the name. The accessors of
        // `EUIAction` give the *map* name of a stateful action, which is a
        // different value.
        let name = unsafe { read_string(g_action_get_name(action.cast())) };
        dispatch_activation(name.as_deref(), composer);
    });
}

/// The dispatch that the `activate` handlers of both menu layers share. It
/// gets the clicked moment from the action name (each layer reads the name
/// differently, see the two `on_activate` functions). Thus a new preset needs
/// only a line in [`schedule::offered`], not a new trampoline.
fn dispatch_activation(name: Option<&str>, composer: gpointer) {
    // SAFETY: the composer is alive, because its menu emitted the signal.
    unsafe {
        match chosen(name) {
            Some(Choice::Custom) => activate_custom(composer.cast()),
            Some(Choice::Preset(preset)) => activate(composer.cast(), preset),
            None => {}
        }
    }
}

/// The menu item that an action name identifies.
#[derive(Debug, PartialEq, Eq)]
enum Choice {
    Custom,
    Preset(Preset),
}

/// Reads the clicked menu item from the action name. Returns `None` for a name
/// that this extension does not own.
fn chosen(name: Option<&str>) -> Option<Choice> {
    let slug = name?.strip_prefix("jmap-send-later-")?;
    tracing::trace!(slug, "send-later: menu item activated");
    if slug == "custom" {
        Some(Choice::Custom)
    } else {
        schedule::from_slug(slug).map(Choice::Preset)
    }
}

/// The data that one click carries into the async message build.
struct PendingSchedule {
    link: Arc<AccountLink>,
    hold: u64,
}

/// A preset click. The function calculates the moment *here*, not from the
/// menu label. Thus a composer that stays open past its suggestion schedules
/// to the next occurrence, not into the past.
///
/// # Safety
///
/// `composer` must be a live decorated composer, on the main loop.
unsafe fn activate(composer: *mut GObject, preset: Preset) {
    let Some(occurrence) = schedule::resolve(preset) else {
        // SAFETY: the contract of this function makes the composer live.
        unsafe { refuse_send(composer, translate(NO_CLOCK)) };
        return;
    };
    // SAFETY: as above.
    unsafe { schedule_hold(composer, occurrence.hold) };
}

/// Holds for `hold` seconds: checks the value against the server limit, then
/// asks the composer for its message, as Send does. The presets and the
/// custom-time dialog both use this function. The dialog checks the limit
/// too, but this function checks it again, so that one location owns the
/// rule.
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
        return; // A race let a click through the insensitive menu.
    };
    if hold > max_hold {
        // SAFETY: the contract of this function makes the composer live.
        unsafe { refuse_send(composer, translate(TOO_FAR)) };
        return;
    }

    let pending = Box::new(PendingSchedule { link, hold });
    // SAFETY: the composer is live. The box goes through GIO to `got_message`,
    // which takes it back one time only.
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

/// Receives the finished message of the composer. It removes the bookkeeping
/// headers of the editor, writes the message, reads the envelope from it, and
/// gives all of it to the worker.
///
/// # Safety
///
/// GIO calls this with the composer as `source_object` and the box that
/// `schedule_hold` leaked as `user_data`.
unsafe extern "C" fn got_message(
    source_object: *mut GObject,
    result: *mut GAsyncResult,
    user_data: gpointer,
) {
    guard("JmapSendLaterExtension::got-message", (), || {
        // SAFETY: this takes back the box that `schedule_hold` leaked, one time
        // only.
        let pending = unsafe { Box::from_raw(user_data.cast::<PendingSchedule>()) };
        let composer = source_object;

        // SAFETY: GIO gave the composer and the result. The message and the
        // error come back owned (transfer full); the code releases both below.
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
                // SAFETY: this arm owns a new GError; the code copies its message
                // and frees it.
                let why = unsafe {
                    let gerror = unwritable.into_gerror(gio_sys::g_io_error_quark(), 0);
                    let why = read_string((*gerror).message)
                        .unwrap_or_else(|| translate(c"no further detail was given"));
                    g_error_free(gerror);
                    why
                };
                // SAFETY: the GIO source_object is a live composer window.
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
                // SAFETY: `with_strong` gives back the live composer window.
                // Evolution also destroys it after a successful send.
                unsafe { gtk_widget_destroy(composer.cast()) };
            }
            // SAFETY: as above; the window is still alive and shown.
            Err(why) => unsafe { refuse_send(composer, translate_with(REFUSED, &[&why])) },
        };
        // SAFETY: the GIO source_object is a live composer window.
        unsafe { dispatch::spawn_for(composer, work, finish) };
    });
}

/// Removes the `X-Evolution-*` bookkeeping headers (identity, transport, Fcc)
/// that the composer writes into the message. The server must not get them.
///
/// # Safety
///
/// `message` must be a live `CamelMimeMessage`.
unsafe fn strip_evolution_headers(message: *mut CamelMimeMessage) {
    // SAFETY: the contract makes the message live. The code frees the
    // duplicated array and copies the names before a removal changes the
    // header list.
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

/// The SMTP envelope from the headers of the message: From is `mailFrom`,
/// and each To/Cc/Bcc entry is a recipient. The usual transport sends to the
/// same set.
///
/// # Safety
///
/// `message` must be a live `CamelMimeMessage`.
unsafe fn envelope_of(message: *mut CamelMimeMessage) -> Result<Envelope, &'static CStr> {
    // SAFETY: the message is live. The address objects and their strings
    // belong to the message, and the code reads them before its release.
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

/// The first address in `list`, if there is one.
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
    // SAFETY: the contract makes the list live; the code copies the strings,
    // which belong to the list.
    unsafe {
        (camel_internet_address_get(list, 0, &mut name, &mut address) != GFALSE)
            .then(|| read_string(address))
            .flatten()
    }
}

/// Shows a modal explanation on the composer. The composer stays open.
///
/// # Safety
///
/// `composer` must be a live composer window, on the main loop.
unsafe fn refuse_send(composer: *mut GObject, text: String) {
    let text = CString::new(text).unwrap_or_default();
    // SAFETY: the contract makes the window live. The code runs and destroys
    // the dialog before it returns. The "%s" format stops printf from parsing
    // the text.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_offered_action_name_reads_back_as_its_preset() {
        for preset in schedule::offered() {
            let name = action_name(preset);
            let name = name.to_str().unwrap();
            assert_eq!(chosen(Some(name)), Some(Choice::Preset(preset)), "{name}");
        }
    }

    #[test]
    fn the_custom_item_and_foreign_names_are_told_apart() {
        assert_eq!(chosen(Some("jmap-send-later-custom")), Some(Choice::Custom));
        assert_eq!(chosen(Some("jmap-send-later-nonsense")), None);
        assert_eq!(chosen(Some("custom")), None);
        assert_eq!(chosen(None), None);
    }

    #[test]
    fn the_menu_definition_names_every_action() {
        let ui = ui_definition();
        let ui = ui.to_str().unwrap();
        let names = schedule::offered().into_iter().map(action_name);
        for name in names.chain([c"jmap-send-later-custom".to_owned()]) {
            let attribute = format!("action='{}'", name.to_str().unwrap());
            assert!(ui.contains(&attribute), "{attribute} is not in {ui}");
        }
        assert!(ui.contains("action='jmap-send-later-menu'"), "{ui}");
    }
}
