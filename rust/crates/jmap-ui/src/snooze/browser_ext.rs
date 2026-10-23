// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapSnoozeBrowserExtension`: the snooze submenu in the detached message
//! window.
//!
//! This window has its own menu layer (3.52: a GtkUIManager; 3.55+: an
//! EUIManager, which the code gets through the `EMailReader` interface, as for
//! the mail view of the shell). The window *is* its own `EMailReader`.
//!
//! At this time, this extension is separate from the extension in
//! `snooze::shell_ext`. `src/modules/mdn/evolution-mdn.c` shows that one
//! extension registered on `E_TYPE_MAIL_READER` covers this window and the
//! mail view of the shell. Thus, it is possible to merge the two extensions
//! of this crate into one.

use std::ffi::CStr;

use eds_sys::{EExtension, e_extension_get_extensible};
#[cfg(evolution_eui_manager)]
use evo_sys::e_mail_reader_get_ui_manager;
#[cfg(not(evolution_eui_manager))]
use evo_sys::{
    E_MAIL_READER_ACTION_GROUP_STANDARD, e_mail_browser_get_ui_manager,
    e_mail_reader_get_action_group,
};
use evo_sys::{EMailBrowser, e_mail_browser_get_type};
use glib_sys::{GType, gpointer, guint};
use gobject_sys::{GObject, g_signal_connect_data};
use jmap_backend_core::callback::gcallback;
use jmap_backend_core::extension::{Extension, ExtensionObject};
use jmap_backend_core::trampoline::guard;

use crate::snooze::action;

/// The snooze submenu of the detached message window.
pub struct SnoozeBrowser;

/// The registered `JmapSnoozeBrowserExtension` type.
pub type JmapSnoozeBrowserExtension = ExtensionObject<SnoozeBrowser>;

impl Extension for SnoozeBrowser {
    const TYPE_NAME: &'static CStr = c"JmapSnoozeBrowserExtension";
    const EXTENSIBLE_NAME: &'static CStr = c"EMailBrowser";

    fn extensible_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_mail_browser_get_type() }
    }

    unsafe fn constructed(extension: *mut EExtension) {
        // SAFETY: the contract of this method.
        unsafe { constructed(extension) }
    }
}

/// Installs the submenu immediately.
/// When a browser constructs its extensions, it is a complete reader and has
/// its UI manager.
unsafe fn constructed(extension: *mut EExtension) {
    unsafe {
        // SAFETY: `extension` is live. The extensible is the browser,
        // which implements EMailReader (an interface pointer is the instance
        // pointer).
        let browser: *mut EMailBrowser = e_extension_get_extensible(extension).cast();
        #[cfg(not(evolution_eui_manager))]
        action::install(
            browser.cast(),
            browser.cast(),
            e_mail_browser_get_ui_manager(browser),
            e_mail_reader_get_action_group(browser.cast(), E_MAIL_READER_ACTION_GROUP_STANDARD),
        );
        // 3.55+: both windows use the same EMailReader getter for the EUIManager.
        #[cfg(evolution_eui_manager)]
        action::install(
            browser.cast(),
            browser.cast(),
            e_mail_reader_get_ui_manager(browser.cast()),
        );

        // In a browser, the reader signal gives the selection changes.
        g_signal_connect_data(
            browser.cast(),
            c"update-actions".as_ptr(),
            gcallback(on_update_actions as unsafe extern "C" fn(*mut GObject, guint, gpointer)),
            std::ptr::null_mut(),
            None,
            0,
        );
        action::update_sensitivity(browser.cast());
    }
}

/// This handler has three arguments. The handler in `snooze::shell_ext` has two.
/// `EMailReader::update-actions` has one `G_TYPE_UINT` parameter and uses
/// `g_cclosure_marshal_VOID__UINT` (`e-mail-reader.c:5374-5382`).
/// The `EShellView` signal with the same name is `VOID__VOID`.
///
/// `state` is the `EMailReaderActionGroup` bitmask of the groups to update.
/// This extension ignores it, because it has one submenu and always reads the
/// selection again. But the handler must *declare* the parameter.
/// If not, the bitmask goes into the user-data slot.
/// `tests/signals.rs` tests this shape.
///
/// # Safety
///
/// The GLib signal system calls this; `browser` is the reader that sent the signal.
unsafe extern "C" fn on_update_actions(browser: *mut GObject, _state: guint, _data: gpointer) {
    guard(
        "JmapSnoozeBrowserExtension::update-actions",
        (),
        || unsafe {
            // SAFETY: the browser that sent the signal is alive.
            action::update_sensitivity(browser);
        },
    );
}
