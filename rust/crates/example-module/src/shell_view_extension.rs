//! Shell view extension integration.

use std::ffi::CStr;
use std::os::raw::{c_char, c_uint, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::ffi;

static TYPE_ID: AtomicUsize = AtomicUsize::new(0);

static PARENT_CLASS: AtomicUsize = AtomicUsize::new(0);

const PRIV_KEY: &CStr = c"m-shell-view-ext-priv";

struct Private {
    current_ui_id: c_uint,
    actions_initialized: bool,
    view_ui_def: Option<&'static str>,
}

unsafe extern "C" fn free_private(ptr: *mut c_void) {
    unsafe {
        drop(Box::from_raw(ptr as *mut Private));
    }
}

/// Get private instance state.
///
/// # Safety
/// `obj` must be a live extension with attached private data.
unsafe fn get_private(obj: *mut ffi::GObject) -> &'static mut Private {
    unsafe {
        let ptr = ffi::g_object_get_data(obj, PRIV_KEY.as_ptr());
        debug_assert!(!ptr.is_null(), "MShellViewExtension: private data is null");
        &mut *(ptr as *mut Private)
    }
}

// Use NUL-terminated UI strings for `gtk_ui_manager_add_ui_from_string`.

const MAIL_UI_DEF: &str = "\
<menubar name='main-menu'>\
  <placeholder name='custom-menus'>\
    <menu action='mail-message-menu'>\
      <placeholder name='mail-message-custom-menus'>\
        <menuitem action='my-mail-ui-message-action'/>\
      </placeholder>\
    </menu>\
  </placeholder>\
</menubar>\
\n\
<popup name='mail-folder-popup'>\
  <placeholder name='mail-folder-popup-actions'>\
    <menuitem action='my-mail-ui-folder-action'/>\
  </placeholder>\
</popup>\
\0";

const CALENDAR_UI_DEF: &str = "\
<menubar name='main-menu'>\
  <placeholder name='custom-menus'>\
    <menu action='calendar-actions-menu'>\
      <menuitem action='my-calendar-ui-action'/>\
    </menu>\
  </placeholder>\
</menubar>\
\n\
<popup name='calendar-event-popup'>\
  <placeholder name='event-popup-actions'>\
    <menuitem action='my-calendar-ui-event-action'/>\
  </placeholder>\
</popup>\
\0";

const GETTEXT_PACKAGE: &CStr = c"example-module";

unsafe extern "C" fn action_mail_folder_cb(_action: *mut ffi::GtkAction, _shell_view: *mut c_void) {
    eprintln!("action_mail_folder_cb: My Maildir Folder Action executed");
}

unsafe extern "C" fn action_mail_message_cb(
    _action: *mut ffi::GtkAction,
    _shell_view: *mut c_void,
) {
    eprintln!("action_mail_message_cb: My Message Action executed");
}

unsafe extern "C" fn action_calendar_event_cb(
    _action: *mut ffi::GtkAction,
    _shell_view: *mut c_void,
) {
    eprintln!("action_calendar_event_cb: My Event Action executed");
}

unsafe extern "C" fn action_calendar_menu_cb(
    _action: *mut ffi::GtkAction,
    _shell_view: *mut c_void,
) {
    eprintln!("action_calendar_menu_cb: My Calendar Action executed");
}

/// Read the GType from an instance.
unsafe fn instance_gtype(instance: *mut c_void) -> ffi::GType {
    unsafe { (*(*instance.cast::<ffi::GTypeInstance>()).g_class).g_type }
}

/// Register view actions and return the matching UI XML.
unsafe fn init_view_ui(
    shell_view: *mut ffi::EShellView,
    shell_window: *mut ffi::EShellWindow,
) -> Option<&'static str> {
    unsafe {
        let view_gtype = instance_gtype(shell_view as *mut c_void);

        if ffi::g_type_is_a(view_gtype, ffi::e_mail_shell_view_get_type()) != 0 {
            let action_group = ffi::e_shell_window_get_action_group(shell_window, c"mail".as_ptr());

            let entries: [ffi::GtkActionEntry; 2] = [
                ffi::GtkActionEntry {
                    name: c"my-mail-ui-folder-action".as_ptr(),
                    stock_id: c"folder-new".as_ptr(),
                    label: c"M_y Maildir Folder Action...".as_ptr(),
                    accelerator: std::ptr::null(),
                    tooltip: c"My Maildir Folder Action".as_ptr(),
                    callback: Some(action_mail_folder_cb),
                },
                ffi::GtkActionEntry {
                    name: c"my-mail-ui-message-action".as_ptr(),
                    stock_id: c"document-new".as_ptr(),
                    label: c"M_y Message Action...".as_ptr(),
                    accelerator: std::ptr::null(),
                    tooltip: c"My Message Action".as_ptr(),
                    callback: Some(action_mail_message_cb),
                },
            ];

            ffi::e_action_group_add_actions_localized(
                action_group,
                GETTEXT_PACKAGE.as_ptr(),
                entries.as_ptr(),
                entries.len() as c_uint,
                shell_view as *mut c_void,
            );

            Some(MAIL_UI_DEF)
        } else if ffi::g_type_is_a(view_gtype, ffi::e_cal_shell_view_get_type()) != 0 {
            let action_group =
                ffi::e_shell_window_get_action_group(shell_window, c"calendar".as_ptr());

            let entries: [ffi::GtkActionEntry; 2] = [
                ffi::GtkActionEntry {
                    name: c"my-calendar-ui-event-action".as_ptr(),
                    stock_id: c"folder-new".as_ptr(),
                    label: c"M_y Event Action...".as_ptr(),
                    accelerator: std::ptr::null(),
                    tooltip: c"My Event Action".as_ptr(),
                    callback: Some(action_calendar_event_cb),
                },
                ffi::GtkActionEntry {
                    name: c"my-calendar-ui-action".as_ptr(),
                    stock_id: c"document-new".as_ptr(),
                    label: c"M_y Calendar Action...".as_ptr(),
                    accelerator: std::ptr::null(),
                    tooltip: c"My Calendar Action".as_ptr(),
                    callback: Some(action_calendar_menu_cb),
                },
            ];

            ffi::e_action_group_add_actions_localized(
                action_group,
                GETTEXT_PACKAGE.as_ptr(),
                entries.as_ptr(),
                entries.len() as c_uint,
                shell_view as *mut c_void,
            );

            Some(CALENDAR_UI_DEF)
        } else {
            None
        }
    }
}

/// Handle `EShellView::toggled`.
unsafe extern "C" fn shell_view_toggled_cb(
    shell_view: *mut ffi::EShellView,
    extension: *mut ffi::GObject,
) {
    unsafe {
        let shell_window = ffi::e_shell_view_get_shell_window(shell_view);
        let ui_manager = ffi::e_shell_window_get_ui_manager(shell_window);
        let priv_ = get_private(extension);

        let need_update = priv_.current_ui_id != 0;
        if priv_.current_ui_id != 0 {
            ffi::gtk_ui_manager_remove_ui(ui_manager, priv_.current_ui_id);
            priv_.current_ui_id = 0;
        }

        let is_active = ffi::e_shell_view_is_active(shell_view) != 0;
        if !is_active {
            if need_update {
                ffi::gtk_ui_manager_ensure_update(ui_manager);
            }
            return;
        }

        if !priv_.actions_initialized {
            priv_.view_ui_def = init_view_ui(shell_view, shell_window);
            priv_.actions_initialized = true;
        }

        if let Some(ui_def) = priv_.view_ui_def {
            let mut error: *mut ffi::GError = std::ptr::null_mut();
            priv_.current_ui_id = ffi::gtk_ui_manager_add_ui_from_string(
                ui_manager,
                ui_def.as_ptr() as *const c_char,
                -1,
                &mut error,
            );
            if !error.is_null() {
                eprintln!("shell_view_toggled_cb: failed to merge UI definition");
                ffi::g_error_free(error);
            }
        }

        ffi::gtk_ui_manager_ensure_update(ui_manager);
    }
}

/// Chain up, then connect `EShellView::toggled`.
unsafe extern "C" fn instance_constructed(object: *mut ffi::GObject) {
    unsafe {
        let parent = PARENT_CLASS.load(Ordering::Acquire) as *mut ffi::GObjectClass;
        if let Some(f) = (*parent).constructed {
            f(object);
        }

        let extensible = ffi::e_extension_get_extensible(object as *mut c_void);
        // Cast to the `GCallback` ABI with signature `void (*)(void)`.
        let cb: unsafe extern "C" fn(*mut ffi::EShellView, *mut ffi::GObject) =
            shell_view_toggled_cb;
        ffi::g_signal_connect_data(
            extensible as *mut c_void,
            c"toggled".as_ptr(),
            Some(std::mem::transmute::<
                unsafe extern "C" fn(*mut ffi::EShellView, *mut ffi::GObject),
                unsafe extern "C" fn(),
            >(cb)),
            object as *mut c_void,
            None,
            0,
        );
    }
}

/// Chain up to `GObjectClass.finalize`.
unsafe extern "C" fn instance_finalize(object: *mut ffi::GObject) {
    unsafe {
        let parent = PARENT_CLASS.load(Ordering::Acquire) as *mut ffi::GObjectClass;
        if let Some(f) = (*parent).finalize {
            f(object);
        }
    }
}

/// Populate the dynamic class vtable for this type.
unsafe extern "C" fn class_init(klass: *mut c_void, _data: *mut c_void) {
    unsafe {
        PARENT_CLASS.store(
            ffi::g_type_class_peek_parent(klass) as usize,
            Ordering::Release,
        );

        let obj_class = &mut *(klass as *mut ffi::GObjectClass);
        obj_class.constructed = Some(instance_constructed);
        obj_class.finalize = Some(instance_finalize);

        let ext_class = &mut *(klass as *mut ffi::EExtensionClass);
        // Resolve the extensible type by name.
        // Do not call a getter in `class_init`.
        // A getter can register this type and deadlock with another thread.
        // `register_type` already performs registration.
        ext_class.extensible_type = ffi::g_type_from_name(c"EShellView".as_ptr());
    }
}

unsafe extern "C" fn class_finalize(_klass: *mut c_void, _data: *mut c_void) {}

/// Allocate private instance data and attach it.
unsafe extern "C" fn instance_init(instance: *mut c_void, _klass: *mut c_void) {
    unsafe {
        let obj = instance as *mut ffi::GObject;
        let priv_data = Box::new(Private {
            current_ui_id: 0,
            actions_initialized: false,
            view_ui_def: None,
        });
        ffi::g_object_set_data_full(
            obj,
            PRIV_KEY.as_ptr(),
            Box::into_raw(priv_data) as *mut c_void,
            Some(free_private),
        );
    }
}

/// Register the dynamic type from `e_module_load`.
pub unsafe fn register_type(type_module: *mut ffi::GTypeModule) {
    unsafe {
        let parent_type = ffi::e_extension_get_type();
        // Register this extensible type before `class_init` resolves it.
        ffi::e_shell_view_get_type();

        // Reuse parent class and instance sizes.
        let mut query = std::mem::MaybeUninit::<ffi::GTypeQuery>::uninit();
        ffi::g_type_query(parent_type, query.as_mut_ptr());
        let query = query.assume_init();

        let type_info = ffi::GTypeInfo {
            class_size: query.class_size as u16,
            base_init: None,
            base_finalize: None,
            class_init: Some(class_init),
            class_finalize: Some(class_finalize),
            class_data: std::ptr::null(),
            instance_size: query.instance_size as u16,
            n_preallocs: 0,
            instance_init: Some(instance_init),
            value_table: std::ptr::null(),
        };

        let type_id = ffi::g_type_module_register_type(
            type_module,
            parent_type,
            c"MShellViewExtension".as_ptr(),
            &type_info,
            0,
        );

        TYPE_ID.store(type_id, Ordering::Release);
    }
}
