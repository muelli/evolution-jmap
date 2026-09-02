// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapSnoozeShellExtension`: the snooze submenu in the mail window.
//!
//! An `EExtension` on `E_TYPE_SHELL_VIEW`, instantiated for every view of
//! every shell window and gated to the one named `"mail"`. The merge happens
//! on the view's first `toggled` (the example module's pattern): only then is
//! the window's `"mail"` action group guaranteed to exist, and only the mail
//! view ever emits it for this extension's purposes. Sensitivity follows
//! `EShellView::update-actions`, which the shell emits on every selection and
//! folder change.

use std::ffi::CStr;

use eds_sys::{EExtension, EExtensionClass, e_extension_get_extensible, e_extension_get_type};
use evo_sys::{
    EShellView, e_shell_view_get_name, e_shell_view_get_shell_content,
    e_shell_view_get_shell_window, e_shell_view_get_type, e_shell_window_get_action_group,
    e_shell_window_get_ui_manager,
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

/// Chains up and, on the mail view, arms the two signals; everything else is
/// theirs to do when they fire.
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

/// The view became the window's active one: the moment the window's `"mail"`
/// action group exists and the merge may happen (once; `install` is
/// idempotent).
///
/// # Safety
///
/// GLib's signal machinery; `view` is the emitting shell view.
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
unsafe extern "C" fn on_update_actions(view: *mut GObject, _data: gpointer) {
    guard("JmapSnoozeShellExtension::update-actions", (), || unsafe {
        // SAFETY: the emitting view is alive.
        action::update_sensitivity(view);
    });
}
