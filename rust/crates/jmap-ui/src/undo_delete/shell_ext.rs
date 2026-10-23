// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapUndoDeleteShellExtension`: the info bar with "Undo" after a collection
//! delete.
//!
//! An `EExtension` on `E_TYPE_SHELL_VIEW`. It works only on the views with the
//! names `"calendar"` and `"addressbook"`, and each view shows only its own kind
//! of collection. The view watches `ESourceRegistry::source-changed`. When a
//! source gets a due time, the view submits an `EAlert` to its shell content.
//! The alert has an Undo button and closes at the due time.
//!
//! `e_alert_add_widget` adds the button, not `e_alert_add_action`. The action
//! type of that function is `GtkAction` before 3.55 and `EUIAction` after it.
//! A widget is the same in both versions.

use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::{CStr, CString, c_char};
use std::ptr;
use std::time::SystemTime;

use eds_sys::{
    EExtension, ESource, ESourceRegistry, e_extension_get_extensible, e_source_get_uid,
    e_source_registry_list_sources, e_source_write, e_source_write_finish,
};
use evo_sys::{
    EAlert, EShellView, GTK_RESPONSE_CLOSE, e_alert_add_widget, e_alert_new, e_alert_response,
    e_alert_set_primary_text, e_alert_sink_submit_alert, e_alert_start_timer, e_shell_get_default,
    e_shell_get_registry, e_shell_view_get_name, e_shell_view_get_shell_content,
    e_shell_view_get_type, gtk_button_new_with_mnemonic, gtk_widget_show,
};
use gio_sys::GAsyncResult;
use glib_sys::{GError, GFALSE, GList, GType, g_error_free, g_list_free_full, gpointer};
use gobject_sys::{
    GClosure, GObject, g_object_get_data, g_object_ref, g_object_set_data_full, g_object_unref,
    g_signal_connect_data, g_signal_connect_object,
};
use jmap_backend_core::callback::gcallback;
use jmap_backend_core::extension::{Extension, ExtensionObject};
use jmap_backend_core::i18n::{N_, translate_static, translate_with};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::trampoline::guard;
use jmap_backend_core::weak::WeakBackend;
use jmap_collection_sync::ChildKind;

use crate::undo_delete::notice::{Deleted, Notice, undo, view_kind};

/// The Undo of a deferred delete in the calendar and contacts views.
pub struct UndoDeleteShell;

/// The registered `JmapUndoDeleteShellExtension` type.
pub type JmapUndoDeleteShellExtension = ExtensionObject<UndoDeleteShell>;

impl Extension for UndoDeleteShell {
    const TYPE_NAME: &'static CStr = c"JmapUndoDeleteShellExtension";
    const EXTENSIBLE_NAME: &'static CStr = c"EShellView";

    fn extensible_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_shell_view_get_type() }
    }

    unsafe fn constructed(extension: *mut EExtension) {
        // SAFETY: the contract of this method.
        unsafe { constructed(extension) }
    }
}

/// The key of the set of shown deletes on the view.
const SHOWN_KEY: &CStr = c"jmap-undo-delete-shown";

/// The deletes that a view showed an info bar for: the uid and the due time.
/// A new delete of the same source has a new due time, and gets a new info bar.
type Shown = RefCell<HashSet<(String, i64)>>;

/// On a calendar or contacts view, connects to the registry and shows the deletes that are pending now.
unsafe fn constructed(extension: *mut EExtension) {
    unsafe {
        // SAFETY: `extension` is live. The extensible is the shell
        // view of this extension.
        let view: *mut EShellView = e_extension_get_extensible(extension).cast();
        let Some(kind) = read_string(e_shell_view_get_name(view))
            .as_deref()
            .and_then(view_kind)
        else {
            return;
        };
        let shell = e_shell_get_default();
        if shell.is_null() {
            return;
        }
        let registry = e_shell_get_registry(shell);
        if registry.is_null() {
            return;
        }

        let shown: *mut Shown = Box::into_raw(Box::default());
        g_object_set_data_full(
            view.cast(),
            SHOWN_KEY.as_ptr(),
            shown.cast(),
            Some(free_shown),
        );

        // The view is the object of the connection, so GLib disconnects the
        // handler when the view is finalized.
        g_signal_connect_object(
            registry.cast(),
            c"source-changed".as_ptr(),
            gcallback(
                on_source_changed
                    as unsafe extern "C" fn(*mut ESourceRegistry, *mut ESource, gpointer),
            ),
            view.cast(),
            0,
        );

        // A delete can be pending when Evolution starts.
        let sources: *mut GList = e_source_registry_list_sources(registry, ptr::null());
        let mut link = sources;
        while !link.is_null() {
            consider(view, kind, (*link).data.cast());
            link = (*link).next;
        }
        g_list_free_full(sources, Some(unref));
    }
}

unsafe extern "C" fn free_shown(data: gpointer) {
    // SAFETY: the box that `constructed` gave to the view, freed one time.
    drop(unsafe { Box::from_raw(data.cast::<Shown>()) });
}

unsafe extern "C" fn unref(object: gpointer) {
    // SAFETY: a reference that the list owns.
    unsafe { g_object_unref(object.cast()) };
}

/// # Safety
///
/// The GLib signal system calls this. `view` is the shell view of the
/// connection, and it is alive.
unsafe extern "C" fn on_source_changed(
    _registry: *mut ESourceRegistry,
    source: *mut ESource,
    view: gpointer,
) {
    guard(
        "JmapUndoDeleteShellExtension::source-changed",
        (),
        || unsafe {
            let view = view.cast::<EShellView>();
            // SAFETY: a live view, and its name is a static string.
            if let Some(kind) = read_string(e_shell_view_get_name(view))
                .as_deref()
                .and_then(view_kind)
            {
                consider(view, kind, source);
            }
        },
    );
}

/// Shows the info bar for `source` if a delete of it is pending, and the view
/// did not show it before.
///
/// # Safety
///
/// `view` must be a live shell view that `constructed` gave the set of shown
/// deletes. `source` must be a valid `ESource`.
unsafe fn consider(view: *mut EShellView, kind: ChildKind, source: *mut ESource) {
    // SAFETY: a valid source by the contract of this function.
    let Some(deleted) = (unsafe { Deleted::read(source) }) else {
        return;
    };
    if deleted.kind != kind {
        return;
    }
    let Some(notice) = deleted.notice(SystemTime::now()) else {
        return;
    };
    let Some(due) = deleted.due else {
        return;
    };
    // SAFETY: a valid source. The uid is a string that the source owns.
    let Some(uid) = (unsafe { read_string(e_source_get_uid(source)) }) else {
        return;
    };
    // SAFETY: `constructed` set the data before it connected the handler, and
    // the view frees it only when it is finalized.
    let shown = unsafe { g_object_get_data(view.cast(), SHOWN_KEY.as_ptr()) }.cast::<Shown>();
    if shown.is_null() {
        return;
    }
    // SAFETY: only the main thread uses the set, and no borrow is open.
    if !unsafe { &*shown }.borrow_mut().insert((uid, due.stamp())) {
        return;
    }
    // SAFETY: the contract of this function.
    unsafe { show(view, source, &notice) };
}

/// The data of the Undo button: the source and the alert of the info bar.
struct UndoData {
    /// A strong reference.
    source: *mut ESource,
    /// A strong reference. The content shows an error if the undo fails.
    content: *mut GObject,
    /// A weak reference. The alert holds the button, so a strong reference
    /// makes a cycle.
    alert: WeakBackend,
}

impl Drop for UndoData {
    fn drop(&mut self) {
        // SAFETY: the references that `show` took, released one time.
        unsafe {
            g_object_unref(self.source.cast());
            g_object_unref(self.content);
        }
    }
}

/// Makes the alert and submits it to the content of `view`.
///
/// # Safety
///
/// `view` must be a live shell view, and `source` a valid `ESource`.
unsafe fn show(view: *mut EShellView, source: *mut ESource, notice: &Notice) {
    let (Ok(primary), Ok(secondary)) = (
        CString::new(notice.primary.as_str()),
        CString::new(notice.secondary.as_str()),
    ) else {
        return;
    };
    // SAFETY: the view is live by the contract of this function. The view owns
    // the content. `system:simple-info` has one argument, the secondary text,
    // and the list of arguments ends with NULL.
    unsafe {
        let content = e_shell_view_get_shell_content(view);
        let alert: *mut EAlert = e_alert_new(
            c"system:simple-info".as_ptr(),
            secondary.as_ptr(),
            ptr::null::<c_char>(),
        );
        if alert.is_null() {
            return;
        }
        e_alert_set_primary_text(alert, primary.as_ptr());

        let button = gtk_button_new_with_mnemonic(translate_static(N_(c"_Undo")));
        gtk_widget_show(button);
        let data = Box::new(UndoData {
            source: g_object_ref(source.cast()).cast(),
            content: g_object_ref(content.cast()).cast(),
            alert: WeakBackend::new(alert.cast()),
        });
        g_signal_connect_data(
            button.cast(),
            c"clicked".as_ptr(),
            gcallback(on_undo_clicked as unsafe extern "C" fn(*mut GObject, gpointer)),
            Box::into_raw(data).cast(),
            Some(free_undo_data),
            0,
        );
        // The alert takes the floating reference of the button.
        e_alert_add_widget(alert, button);
        e_alert_start_timer(alert, notice.seconds);
        e_alert_sink_submit_alert(content.cast(), alert);
        g_object_unref(alert.cast());
    }
}

unsafe extern "C" fn free_undo_data(data: gpointer, _closure: *mut GClosure) {
    // SAFETY: the box that `show` gave to the handler, freed one time.
    drop(unsafe { Box::from_raw(data.cast::<UndoData>()) });
}

/// Undoes the delete, sends the source to the registry and closes the info bar.
///
/// # Safety
///
/// The GLib signal system calls this. `data` is the [`UndoData`] of the button.
unsafe extern "C" fn on_undo_clicked(_button: *mut GObject, data: gpointer) {
    guard("JmapUndoDeleteShellExtension::undo", (), || unsafe {
        // SAFETY: the handler owns the data until the button is finalized.
        let data = &*data.cast::<UndoData>();
        undo(data.source);
        // The callback owns the new reference to the content.
        e_source_write(
            data.source,
            ptr::null_mut(),
            Some(on_written),
            g_object_ref(data.content).cast(),
        );
        data.alert.with_strong(|alert| {
            // SAFETY: a strong reference for the call.
            e_alert_response(alert.cast(), GTK_RESPONSE_CLOSE);
        });
    });
}

/// Shows an error in the content if the registry refused the undo.
///
/// # Safety
///
/// GIO calls this with the source of the write. `content` is a reference that
/// this function owns.
unsafe extern "C" fn on_written(
    source: *mut GObject,
    result: *mut GAsyncResult,
    content: gpointer,
) {
    guard("JmapUndoDeleteShellExtension::written", (), || unsafe {
        let mut error: *mut GError = ptr::null_mut();
        if e_source_write_finish(source.cast(), result, &mut error) == GFALSE {
            let reason = if error.is_null() {
                String::new()
            } else {
                read_string((*error).message).unwrap_or_default()
            };
            tracing::warn!("the undo of a collection delete was not saved: {reason}");
            let text = translate_with(
                // TRANSLATORS: %1$s is the reason from the registry.
                c"The delete could not be undone: %1$s",
                &[&reason],
            );
            if let Ok(text) = CString::new(text) {
                let alert = e_alert_new(
                    c"system:simple-error".as_ptr(),
                    text.as_ptr(),
                    ptr::null::<c_char>(),
                );
                if !alert.is_null() {
                    e_alert_sink_submit_alert(content.cast(), alert);
                    g_object_unref(alert.cast());
                }
            }
            if !error.is_null() {
                g_error_free(error);
            }
        }
        g_object_unref(content.cast());
    });
}
