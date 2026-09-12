// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapSnoozeShellExtension`: the snooze submenu in the mail window.
//!
//! An `EExtension` on `E_TYPE_SHELL_VIEW`, instantiated for every view of
//! every shell window and gated to the one named `"mail"`. Two ways to catch
//! the moment the merge may happen, chosen by `evolution_eui_manager`:
//!
//! - Evolution < 3.55 waits for the view's first `"toggled"` (the example
//!   module's pattern): only then is the window's `"mail"` action group
//!   guaranteed to exist, and only the mail view ever emits it for this
//!   extension's purposes.
//! - 3.55+ has no `"toggled"` signal on `EShellView` at all — confirmed live,
//!   not assumed: connecting it the old way logs a GLib-CRITICAL
//!   ("signal 'toggled' is invalid for instance ... EMailShellView") and never
//!   fires, because the whole GtkToggleAction-based view switcher it belonged
//!   to went with the rest of the GtkAction era. `e-rss-shell-view-extension.c`
//!   proves merging the menu in `constructed()` itself is safe there (its own
//!   `EUIManager` merge runs there, unconditionally), but proves nothing about
//!   the *reader* (the `"mail-view"` property) being populated that early —
//!   nothing upstream needed it at construction the way this extension does.
//!   So the merge is attempted in `constructed()` (cheap, and normally enough)
//!   and retried, idempotently, on the first `"update-actions"` — the one
//!   signal both eras keep — if the reader was not there yet.
//!
//! Sensitivity always follows `EShellView::update-actions`, which the shell
//! emits on every selection and folder change.

use std::ffi::CStr;

use eds_sys::{EExtension, EExtensionClass, e_extension_get_extensible, e_extension_get_type};
#[cfg(evolution_eui_manager)]
use evo_sys::e_shell_view_get_ui_manager;
use evo_sys::{
    EShellView, e_shell_view_get_name, e_shell_view_get_shell_content, e_shell_view_get_type,
};
#[cfg(not(evolution_eui_manager))]
use evo_sys::{
    e_shell_view_get_shell_window, e_shell_window_get_action_group, e_shell_window_get_ui_manager,
};
use glib_sys::{GType, gpointer};
use gobject_sys::{GObject, GObjectClass, g_signal_connect_data};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::subclass::{self, ObjectSubclass};
use jmap_backend_core::trampoline::guard;

use crate::snooze::action;

/// The instance: `EExtension`'s own state and nothing else.
#[repr(C)]
pub struct JmapSnoozeShellExtension {
    parent: EExtension,
}

/// The class: `EExtensionClass`'s own state and nothing else.
#[repr(C)]
pub struct JmapSnoozeShellExtensionClass {
    parent_class: EExtensionClass,
}

// SAFETY: both structs are #[repr(C)] and lead with EExtension's own structs;
// EExtension derives from GObject.
unsafe impl ObjectSubclass for JmapSnoozeShellExtension {
    const NAME: &'static CStr = c"JmapSnoozeShellExtension";
    type Instance = JmapSnoozeShellExtension;
    type Class = JmapSnoozeShellExtensionClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_extension_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` leads with `EExtensionClass`.
        unsafe { (*class).parent_class.extensible_type = e_shell_view_get_type() };
        let object_class = class.cast::<GObjectClass>();
        // SAFETY: transitively leads with GObjectClass.
        unsafe { (*object_class).constructed = Some(constructed) };
    }
}

/// Chains up and, on the mail view, arms whichever signals this menu layer
/// needs; everything else is theirs to do when they fire.
#[cfg(not(evolution_eui_manager))]
unsafe extern "C" fn constructed(object: *mut GObject) {
    guard("JmapSnoozeShellExtension::constructed", (), || unsafe {
        // SAFETY: the parent class of a live instance is initialised and alive.
        let parent =
            subclass::parent_class::<GObjectClass>(JmapSnoozeShellExtension::parent_type());
        if let Some(chained) = parent.and_then(|class| class.constructed) {
            chained(object);
        }

        // SAFETY: GObject passes a live instance; the extensible is the shell
        // view this extension was instantiated for.
        let view: *mut EShellView = e_extension_get_extensible(object.cast::<EExtension>()).cast();
        if read_string(e_shell_view_get_name(view)).as_deref() != Some("mail") {
            return;
        }

        for (signal, handler) in [
            (
                c"toggled",
                on_toggled as unsafe extern "C" fn(*mut GObject, gpointer),
            ),
            (c"update-actions", on_update_actions),
        ] {
            g_signal_connect_data(
                view.cast(),
                signal.as_ptr(),
                Some(std::mem::transmute::<
                    unsafe extern "C" fn(*mut GObject, gpointer),
                    unsafe extern "C" fn(),
                >(handler)),
                std::ptr::null_mut(),
                None,
                0,
            );
        }
    });
}

/// # Safety
///
/// GObject passes a live instance being constructed.
#[cfg(evolution_eui_manager)]
unsafe extern "C" fn constructed(object: *mut GObject) {
    guard("JmapSnoozeShellExtension::constructed", (), || unsafe {
        // SAFETY: the parent class of a live instance is initialised and alive.
        let parent =
            subclass::parent_class::<GObjectClass>(JmapSnoozeShellExtension::parent_type());
        if let Some(chained) = parent.and_then(|class| class.constructed) {
            chained(object);
        }

        // SAFETY: GObject passes a live instance; the extensible is the shell
        // view this extension was instantiated for.
        let view: *mut EShellView = e_extension_get_extensible(object.cast::<EExtension>()).cast();
        if read_string(e_shell_view_get_name(view)).as_deref() != Some("mail") {
            return;
        }

        try_install(view);

        g_signal_connect_data(
            view.cast(),
            c"update-actions".as_ptr(),
            Some(std::mem::transmute::<
                unsafe extern "C" fn(*mut GObject, gpointer),
                unsafe extern "C" fn(),
            >(on_update_actions)),
            std::ptr::null_mut(),
            None,
            0,
        );
    });
}

/// Fetch the manager and reader off `view` and merge the submenu; a no-op if
/// already installed ([`action::install`] is idempotent per owner) or if the
/// reader is not there yet, in which case `on_update_actions` tries again.
///
/// # Safety
///
/// `view` must be a live shell view named `"mail"`.
#[cfg(evolution_eui_manager)]
unsafe fn try_install(view: *mut EShellView) {
    // SAFETY: a live shell view per this function's contract; the manager and
    // content are its own.
    unsafe {
        let reader = action::reader_of_content(e_shell_view_get_shell_content(view));
        action::install(view.cast(), reader, e_shell_view_get_ui_manager(view));
    }
}

/// The view became the window's active one: the moment the window's `"mail"`
/// action group exists and the merge may happen (once; `install` is
/// idempotent).
///
/// # Safety
///
/// GLib's signal machinery; `view` is the emitting shell view.
#[cfg(not(evolution_eui_manager))]
unsafe extern "C" fn on_toggled(view: *mut GObject, _data: gpointer) {
    guard("JmapSnoozeShellExtension::toggled", (), || unsafe {
        // SAFETY: the emitting view is alive; window, manager, group and
        // content are its own.
        let shell_view = view.cast::<EShellView>();
        let window = e_shell_view_get_shell_window(shell_view);
        let reader = action::reader_of_content(e_shell_view_get_shell_content(shell_view));
        action::install(
            view,
            reader,
            e_shell_window_get_ui_manager(window),
            e_shell_window_get_action_group(window, c"mail".as_ptr()),
        );
    });
}

/// # Safety
///
/// GLib's signal machinery; `view` is the emitting shell view.
#[cfg(not(evolution_eui_manager))]
unsafe extern "C" fn on_update_actions(view: *mut GObject, _data: gpointer) {
    guard("JmapSnoozeShellExtension::update-actions", (), || unsafe {
        // SAFETY: the emitting view is alive.
        action::update_sensitivity(view);
    });
}

/// # Safety
///
/// GLib's signal machinery; `view` is the emitting shell view.
#[cfg(evolution_eui_manager)]
unsafe extern "C" fn on_update_actions(view: *mut GObject, _data: gpointer) {
    guard("JmapSnoozeShellExtension::update-actions", (), || unsafe {
        // SAFETY: the emitting view is alive; harmless if `constructed`
        // already installed (idempotent), needed if the reader was not
        // populated yet at that point.
        try_install(view.cast());
        action::update_sensitivity(view);
    });
}
