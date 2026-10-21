//! Integrate the message composer extension.

use std::ffi::CStr;
use std::os::raw::{c_char, c_uint, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::ffi;

static TYPE_ID: AtomicUsize = AtomicUsize::new(0);

static PARENT_CLASS: AtomicUsize = AtomicUsize::new(0);

const COMPOSER_UI_DEF: &str = "\
<menubar name='main-menu'>\
  <placeholder name='pre-edit-menu'>\
    <menu action='file-menu'>\
      <placeholder name='external-editor-holder'>\
        <menuitem action='my-msg-composer-action'/>\
      </placeholder>\
    </menu>\
  </placeholder>\
</menubar>\
\n\
<toolbar name='main-toolbar'>\
  <toolitem action='my-msg-composer-action'/>\
</toolbar>\
\0";

const GETTEXT_PACKAGE: &CStr = c"example-module";

unsafe extern "C" fn action_msg_composer_cb(_action: *mut ffi::GtkAction, extension: *mut c_void) {
    unsafe {
        let composer = ffi::e_extension_get_extensible(extension) as *mut c_void;
        let title_ptr = ffi::gtk_window_get_title(composer);

        let title = if title_ptr.is_null() {
            "(null)".to_owned()
        } else {
            std::ffi::CStr::from_ptr(title_ptr)
                .to_str()
                .unwrap_or("(invalid UTF-8)")
                .to_owned()
        };

        eprintln!("action_msg_composer_cb: for composer '{title}'");
    }
}

unsafe fn add_composer_ui(extension: *mut ffi::GObject, composer: *mut ffi::EMsgComposer) {
    unsafe {
        let html_editor = ffi::e_msg_composer_get_editor(composer);
        let ui_manager = ffi::e_html_editor_get_ui_manager(html_editor);
        let action_group = ffi::e_html_editor_get_action_group(html_editor, c"core".as_ptr());

        let entries: [ffi::GtkActionEntry; 1] = [ffi::GtkActionEntry {
            name: c"my-msg-composer-action".as_ptr(),
            stock_id: c"document-new".as_ptr(),
            label: c"M_y Message Composer Action...".as_ptr(),
            accelerator: std::ptr::null(),
            tooltip: c"My Message Composer Action".as_ptr(),
            callback: Some(action_msg_composer_cb),
        }];

        ffi::e_action_group_add_actions_localized(
            action_group,
            GETTEXT_PACKAGE.as_ptr(),
            entries.as_ptr(),
            entries.len() as c_uint,
            extension as *mut c_void,
        );

        let mut error: *mut ffi::GError = std::ptr::null_mut();
        ffi::gtk_ui_manager_add_ui_from_string(
            ui_manager,
            COMPOSER_UI_DEF.as_ptr() as *const c_char,
            -1,
            &mut error,
        );
        if !error.is_null() {
            eprintln!("add_composer_ui: failed to merge UI definition");
            ffi::g_error_free(error);
        }

        ffi::gtk_ui_manager_ensure_update(ui_manager);
    }
}

unsafe extern "C" fn instance_constructed(object: *mut ffi::GObject) {
    unsafe {
        let parent = PARENT_CLASS.load(Ordering::Acquire) as *mut ffi::GObjectClass;
        if let Some(f) = (*parent).constructed {
            f(object);
        }

        let extensible = ffi::e_extension_get_extensible(object as *mut c_void);
        add_composer_ui(object, extensible as *mut ffi::EMsgComposer);
    }
}

unsafe extern "C" fn class_init(klass: *mut c_void, _data: *mut c_void) {
    unsafe {
        PARENT_CLASS.store(
            ffi::g_type_class_peek_parent(klass) as usize,
            Ordering::Release,
        );

        let obj_class = &mut *(klass as *mut ffi::GObjectClass);
        obj_class.constructed = Some(instance_constructed);

        let ext_class = &mut *(klass as *mut ffi::EExtensionClass);
        // Resolve the type by name. Do not call a getter here.
        // A getter can register a type inside class init.
        // This can deadlock with another registration thread.
        // `register_type` has already registered this type.
        ext_class.extensible_type = ffi::g_type_from_name(c"EMsgComposer".as_ptr());
    }
}

unsafe extern "C" fn class_finalize(_klass: *mut c_void, _data: *mut c_void) {}

unsafe extern "C" fn instance_init(_instance: *mut c_void, _klass: *mut c_void) {}

pub unsafe fn register_type(type_module: *mut ffi::GTypeModule) {
    unsafe {
        let parent_type = ffi::e_extension_get_type();
        // Register the extensible type before `class_init` resolves it.
        ffi::e_msg_composer_get_type();

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
            c"MMsgComposerExtension".as_ptr(),
            &type_info,
            0,
        );

        TYPE_ID.store(type_id, Ordering::Release);
    }
}
