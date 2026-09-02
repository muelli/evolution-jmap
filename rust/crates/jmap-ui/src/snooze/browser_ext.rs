// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapSnoozeBrowserExtension`: the same snooze submenu in the detached
//! message window, which has a GtkUIManager of its own and *is* its own
//! `EMailReader` — the duplication 3.52's design asks of every module
//! (there is no shared reader-actions extension point; worth an upstream
//! issue, tracked in the project plan).

use std::ffi::CStr;

use eds_sys::{EExtension, EExtensionClass, e_extension_get_extensible, e_extension_get_type};
use evo_sys::{
    E_MAIL_READER_ACTION_GROUP_STANDARD, EMailBrowser, e_mail_browser_get_type,
    e_mail_browser_get_ui_manager, e_mail_reader_get_action_group,
};
use glib_sys::{GType, gpointer};
use gobject_sys::{GObject, GObjectClass, g_signal_connect_data};
use jmap_backend_core::subclass::{self, ObjectSubclass};
use jmap_backend_core::trampoline::guard;

use crate::snooze::action;

/// The instance: `EExtension`'s own state and nothing else.
#[repr(C)]
pub struct JmapSnoozeBrowserExtension {
    parent: EExtension,
}

/// The class: `EExtensionClass`'s own state and nothing else.
#[repr(C)]
pub struct JmapSnoozeBrowserExtensionClass {
    parent_class: EExtensionClass,
}

// SAFETY: both structs are #[repr(C)] and lead with EExtension's own structs;
// EExtension derives from GObject.
unsafe impl ObjectSubclass for JmapSnoozeBrowserExtension {
    const NAME: &'static CStr = c"JmapSnoozeBrowserExtension";
    type Instance = JmapSnoozeBrowserExtension;
    type Class = JmapSnoozeBrowserExtensionClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_extension_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` leads with `EExtensionClass`.
        unsafe { (*class).parent_class.extensible_type = e_mail_browser_get_type() };
        let object_class = class.cast::<GObjectClass>();
        // SAFETY: transitively leads with GObjectClass.
        unsafe { (*object_class).constructed = Some(constructed) };
    }
}

/// Chains up and installs immediately: by the time a browser constructs its
/// extensions it is a complete reader with its UI manager in hand.
unsafe extern "C" fn constructed(object: *mut GObject) {
    guard("JmapSnoozeBrowserExtension::constructed", (), || unsafe {
        // SAFETY: the parent class of a live instance is initialised and alive.
        let parent =
            subclass::parent_class::<GObjectClass>(JmapSnoozeBrowserExtension::parent_type());
        if let Some(chained) = parent.and_then(|class| class.constructed) {
            chained(object);
        }

        // SAFETY: GObject passes a live instance; the extensible is the
        // browser, which implements EMailReader (an interface pointer is the
        // instance pointer).
        let browser: *mut EMailBrowser =
            e_extension_get_extensible(object.cast::<EExtension>()).cast();
        action::install(
            browser.cast(),
            browser.cast(),
            e_mail_browser_get_ui_manager(browser),
            e_mail_reader_get_action_group(browser.cast(), E_MAIL_READER_ACTION_GROUP_STANDARD),
        );

        // Selection changes in a browser go through the reader's own signal.
        g_signal_connect_data(
            browser.cast(),
            c"update-actions".as_ptr(),
            Some(std::mem::transmute::<
                unsafe extern "C" fn(*mut GObject, gpointer),
                unsafe extern "C" fn(),
            >(on_update_actions)),
            std::ptr::null_mut(),
            None,
            0,
        );
        action::update_sensitivity(browser.cast());
    });
}

/// # Safety
///
/// GLib's signal machinery; `browser` is the emitting reader.
unsafe extern "C" fn on_update_actions(browser: *mut GObject, _data: gpointer) {
    guard(
        "JmapSnoozeBrowserExtension::update-actions",
        (),
        || unsafe {
            // SAFETY: the emitting browser is alive.
            action::update_sensitivity(browser);
        },
    );
}
