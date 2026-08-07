//! Shell-view extension — mirrors `m-shell-view-extension.c`.
//!
//! `MShellViewExtension` is registered as an `EExtension` for *every*
//! `EShellView` (mail, calendar, …).  When a view is toggled active the
//! extension adds view-specific menu/toolbar items; when it goes inactive they
//! are removed again.

use std::os::raw::{c_char, c_uint, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::ffi;

// ── Module-level state ────────────────────────────────────────────────────────

/// The `GType` assigned to `MShellViewExtension` after `register_type`.
static TYPE_ID: AtomicUsize = AtomicUsize::new(0);

/// Pointer to the parent class (stored during `class_init` for chain-up).
static PARENT_CLASS: AtomicUsize = AtomicUsize::new(0);

// ── Per-instance private data ─────────────────────────────────────────────────

const PRIV_KEY: &[u8] = b"m-shell-view-ext-priv\0";

/// Rust-managed per-instance data, attached via `g_object_set_data_full`.
struct Private {
    /// The UI-manager merge ID currently active, or 0 when no UI is merged.
    current_ui_id: c_uint,
    /// Set to `true` after the first activation (prevents repeated action
    /// registration).
    actions_initialized: bool,
    /// The UI XML string for this view type.  `None` when the view type is
    /// unknown (e.g., tasks or contacts — not handled by this template).
    view_ui_def: Option<&'static str>,
}

/// Destroy notifier called by GLib when the object is finalized.
unsafe extern "C" fn free_private(ptr: *mut c_void) {
    drop(Box::from_raw(ptr as *mut Private));
}

/// Retrieve the `Private` data attached to a GObject instance.
///
/// # Safety
/// `obj` must be a live `MShellViewExtension` instance with private data
/// already attached.
unsafe fn get_private(obj: *mut ffi::GObject) -> &'static mut Private {
    let ptr = ffi::g_object_get_data(obj, PRIV_KEY.as_ptr() as *const c_char);
    debug_assert!(!ptr.is_null(), "MShellViewExtension: private data is null");
    &mut *(ptr as *mut Private)
}

// ── UI XML strings ────────────────────────────────────────────────────────────
//
// Each string is NUL-terminated so it can be passed to
// `gtk_ui_manager_add_ui_from_string` with `length = -1`.

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

const GETTEXT_PACKAGE: &[u8] = b"example-module\0";

// ── Action callbacks ──────────────────────────────────────────────────────────

/// Triggered by the mail folder context-menu action.
unsafe extern "C" fn action_mail_folder_cb(_action: *mut ffi::GtkAction, _shell_view: *mut c_void) {
    eprintln!("action_mail_folder_cb: My Maildir Folder Action executed");
}

/// Triggered by the mail message menu action.
unsafe extern "C" fn action_mail_message_cb(
    _action: *mut ffi::GtkAction,
    _shell_view: *mut c_void,
) {
    eprintln!("action_mail_message_cb: My Message Action executed");
}

/// Triggered by the calendar event context-menu action.
unsafe extern "C" fn action_calendar_event_cb(
    _action: *mut ffi::GtkAction,
    _shell_view: *mut c_void,
) {
    eprintln!("action_calendar_event_cb: My Event Action executed");
}

/// Triggered by the calendar menu action.
unsafe extern "C" fn action_calendar_menu_cb(
    _action: *mut ffi::GtkAction,
    _shell_view: *mut c_void,
) {
    eprintln!("action_calendar_menu_cb: My Calendar Action executed");
}

// ── Helper: determine view type, register actions, return UI definition ───────

/// Read the `GType` of a GObject instance by following the class pointer.
///
/// Equivalent to the C macro `G_TYPE_FROM_INSTANCE`.
unsafe fn instance_gtype(instance: *mut c_void) -> ffi::GType {
    (*(*instance.cast::<ffi::GTypeInstance>()).g_class).g_type
}

/// Called once per extension instance the first time its view is activated.
///
/// Determines which view type this extension is attached to, registers the
/// appropriate `GtkAction` entries into the shell window's action group, and
/// returns the matching UI XML definition (or `None` for unsupported views).
unsafe fn init_view_ui(
    shell_view: *mut ffi::EShellView,
    shell_window: *mut ffi::EShellWindow,
) -> Option<&'static str> {
    let view_gtype = instance_gtype(shell_view as *mut c_void);

    if ffi::g_type_is_a(view_gtype, ffi::e_mail_shell_view_get_type()) != 0 {
        // ── Mail view ─────────────────────────────────────────────────────────
        let action_group =
            ffi::e_shell_window_get_action_group(shell_window, b"mail\0".as_ptr() as *const c_char);

        let entries: [ffi::GtkActionEntry; 2] = [
            ffi::GtkActionEntry {
                name: b"my-mail-ui-folder-action\0".as_ptr() as *const c_char,
                stock_id: b"folder-new\0".as_ptr() as *const c_char,
                label: b"M_y Maildir Folder Action...\0".as_ptr() as *const c_char,
                accelerator: std::ptr::null(),
                tooltip: b"My Maildir Folder Action\0".as_ptr() as *const c_char,
                callback: Some(action_mail_folder_cb),
            },
            ffi::GtkActionEntry {
                name: b"my-mail-ui-message-action\0".as_ptr() as *const c_char,
                stock_id: b"document-new\0".as_ptr() as *const c_char,
                label: b"M_y Message Action...\0".as_ptr() as *const c_char,
                accelerator: std::ptr::null(),
                tooltip: b"My Message Action\0".as_ptr() as *const c_char,
                callback: Some(action_mail_message_cb),
            },
        ];

        ffi::e_action_group_add_actions_localized(
            action_group,
            GETTEXT_PACKAGE.as_ptr() as *const c_char,
            entries.as_ptr(),
            entries.len() as c_uint,
            shell_view as *mut c_void,
        );

        Some(MAIL_UI_DEF)
    } else if ffi::g_type_is_a(view_gtype, ffi::e_cal_shell_view_get_type()) != 0 {
        // ── Calendar view ─────────────────────────────────────────────────────
        let action_group = ffi::e_shell_window_get_action_group(
            shell_window,
            b"calendar\0".as_ptr() as *const c_char,
        );

        let entries: [ffi::GtkActionEntry; 2] = [
            ffi::GtkActionEntry {
                name: b"my-calendar-ui-event-action\0".as_ptr() as *const c_char,
                stock_id: b"folder-new\0".as_ptr() as *const c_char,
                label: b"M_y Event Action...\0".as_ptr() as *const c_char,
                accelerator: std::ptr::null(),
                tooltip: b"My Event Action\0".as_ptr() as *const c_char,
                callback: Some(action_calendar_event_cb),
            },
            ffi::GtkActionEntry {
                name: b"my-calendar-ui-action\0".as_ptr() as *const c_char,
                stock_id: b"document-new\0".as_ptr() as *const c_char,
                label: b"M_y Calendar Action...\0".as_ptr() as *const c_char,
                accelerator: std::ptr::null(),
                tooltip: b"My Calendar Action\0".as_ptr() as *const c_char,
                callback: Some(action_calendar_menu_cb),
            },
        ];

        ffi::e_action_group_add_actions_localized(
            action_group,
            GETTEXT_PACKAGE.as_ptr() as *const c_char,
            entries.as_ptr(),
            entries.len() as c_uint,
            shell_view as *mut c_void,
        );

        Some(CALENDAR_UI_DEF)
    } else {
        // Tasks, memos, contacts, … are not handled by this template.
        None
    }
}

// ── Signal callbacks ──────────────────────────────────────────────────────────

/// Called whenever an `EShellView` is toggled active or inactive.
///
/// Signature matches the GObject "toggled" signal:
/// `void (*toggled) (EShellView *shell_view, gpointer user_data)`
unsafe extern "C" fn shell_view_toggled_cb(
    shell_view: *mut ffi::EShellView,
    extension: *mut ffi::GObject,
) {
    let shell_window = ffi::e_shell_view_get_shell_window(shell_view);
    let ui_manager = ffi::e_shell_window_get_ui_manager(shell_window);
    let priv_ = get_private(extension);

    // Remove any UI we merged during a previous activation.
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

    // On first activation: detect view type and register GtkActions.
    if !priv_.actions_initialized {
        priv_.view_ui_def = init_view_ui(shell_view, shell_window);
        priv_.actions_initialized = true;
    }

    // Merge the UI definition (XML) into the UI manager.
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

// ── GObject virtual-method overrides ─────────────────────────────────────────

/// `GObjectClass.constructed` override.
///
/// Chains up, attaches private data, then connects to the shell view's
/// "toggled" signal so we can add/remove UI whenever the view activates.
unsafe extern "C" fn instance_constructed(object: *mut ffi::GObject) {
    // Chain up to the parent class.
    let parent = PARENT_CLASS.load(Ordering::Acquire) as *mut ffi::GObjectClass;
    if let Some(f) = (*parent).constructed {
        f(object);
    }

    // Connect the "toggled" signal of the EShellView we are extending.
    let extensible = ffi::e_extension_get_extensible(object as *mut c_void);
    // `GCallback` is `void (*)(void)` — transmute the concrete type to it.
    let cb: unsafe extern "C" fn(*mut ffi::EShellView, *mut ffi::GObject) = shell_view_toggled_cb;
    ffi::g_signal_connect_data(
        extensible as *mut c_void,
        b"toggled\0".as_ptr() as *const c_char,
        Some(std::mem::transmute::<
            unsafe extern "C" fn(*mut ffi::EShellView, *mut ffi::GObject),
            unsafe extern "C" fn(),
        >(cb)),
        object as *mut c_void, // user_data = our extension instance
        None,
        0, // G_CONNECT_DEFAULT
    );
}

/// `GObjectClass.finalize` override — chains up (private data is freed by the
/// `g_object_set_data_full` destroy notifier).
unsafe extern "C" fn instance_finalize(object: *mut ffi::GObject) {
    let parent = PARENT_CLASS.load(Ordering::Acquire) as *mut ffi::GObjectClass;
    if let Some(f) = (*parent).finalize {
        f(object);
    }
}

// ── GTypeModule callbacks ─────────────────────────────────────────────────────

/// Initialise the *class* struct.  Called once by GLib when the type is first
/// used.
unsafe extern "C" fn class_init(klass: *mut c_void, _data: *mut c_void) {
    // Save the parent class pointer so we can chain-up from our overrides.
    PARENT_CLASS.store(
        ffi::g_type_class_peek_parent(klass) as usize,
        Ordering::Release,
    );

    // Override GObjectClass virtual methods.
    let obj_class = &mut *(klass as *mut ffi::GObjectClass);
    obj_class.constructed = Some(instance_constructed);
    obj_class.finalize = Some(instance_finalize);

    // Set the extensible type — the type of object this extension attaches to.
    let ext_class = &mut *(klass as *mut ffi::EExtensionClass);
    ext_class.extensible_type = ffi::e_shell_view_get_type();
}

/// Finalise the class struct when the module is unloaded.
unsafe extern "C" fn class_finalize(_klass: *mut c_void, _data: *mut c_void) {}

/// Initialise a new *instance* struct.  Attaches the Rust private data.
unsafe extern "C" fn instance_init(instance: *mut c_void, _klass: *mut c_void) {
    let obj = instance as *mut ffi::GObject;
    let priv_data = Box::new(Private {
        current_ui_id: 0,
        actions_initialized: false,
        view_ui_def: None,
    });
    ffi::g_object_set_data_full(
        obj,
        PRIV_KEY.as_ptr() as *const c_char,
        Box::into_raw(priv_data) as *mut c_void,
        Some(free_private),
    );
}

// ── Public: type registration ─────────────────────────────────────────────────

/// Register `MShellViewExtension` into `type_module`.
///
/// Must be called from `e_module_load`.
pub unsafe fn register_type(type_module: *mut ffi::GTypeModule) {
    let parent_type = ffi::e_extension_get_type();

    // Query the parent's class/instance sizes so we can use them verbatim
    // (our type adds no extra inline fields).
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
        b"MShellViewExtension\0".as_ptr() as *const c_char,
        &type_info,
        0, // G_TYPE_FLAG_NONE
    );

    TYPE_ID.store(type_id, Ordering::Release);
}
