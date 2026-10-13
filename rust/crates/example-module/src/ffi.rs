//! Raw FFI declarations used by the example module.
//!
//! `build.rs` handles all linker flags through `pkg-config`.

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::os::raw::{c_char, c_int, c_uint, c_ulong, c_void};

pub type GType = usize;

// This module assumes 64-bit `GType`.
const _: () = assert!(
    std::mem::size_of::<usize>() == 8,
    "GType is assumed to be 8 bytes (64-bit platforms only)"
);

pub type GBoolean = c_int;

// Opaque handles are pointer-only in this crate.

pub type GTypeModule = c_void;

pub type GObject = c_void;

pub type GError = c_void;

// Evolution opaque handles.
pub type EExtensible = c_void;
pub type EShellView = c_void;
pub type EShellWindow = c_void;
pub type EMsgComposer = c_void;
pub type EHTMLEditor = c_void;

// GTK 3 opaque handles.
pub type GtkUIManager = c_void;
pub type GtkActionGroup = c_void;
pub type GtkAction = c_void;

#[repr(C)]
pub struct GTypeClass {
    pub g_type: GType,
}

#[repr(C)]
pub struct GTypeInstance {
    pub g_class: *mut GTypeClass,
}

// Must match `struct _GObjectClass` from `<gobject/gobject.h>` on LP64.

#[repr(C)]
pub struct GObjectClass {
    pub g_type_class: GTypeClass,          // 8
    pub construct_properties: *mut c_void, // 8
    pub constructor: Option<unsafe extern "C" fn(GType, c_uint, *mut c_void) -> *mut GObject>, // 8
    pub set_property:
        Option<unsafe extern "C" fn(*mut GObject, c_uint, *const c_void, *const c_void)>, // 8
    pub get_property:
        Option<unsafe extern "C" fn(*mut GObject, c_uint, *mut c_void, *const c_void)>, // 8
    pub dispose: Option<unsafe extern "C" fn(*mut GObject)>, // 8
    pub finalize: Option<unsafe extern "C" fn(*mut GObject)>, // 8
    pub dispatch_properties_changed:
        Option<unsafe extern "C" fn(*mut GObject, c_uint, *mut *mut c_void)>, // 8
    pub notify: Option<unsafe extern "C" fn(*mut GObject, *mut c_void)>, // 8
    pub constructed: Option<unsafe extern "C" fn(*mut GObject)>, // 8
    pub flags: usize,                      // 8
    pub n_construct_properties: usize,     // 8
    pub pspecs: *mut c_void,               // 8
    pub n_pspecs: usize,                   // 8
    pub pdummy: [*mut c_void; 3],          // 24
                                           // total: 136
}

// Matches `struct _EExtensionClass` from `<libebackend/e-extension.h>`.

#[repr(C)]
pub struct EExtensionClass {
    pub parent_class: GObjectClass,   // 136
    pub extensible_type: GType,       //   8
    pub _reserved: [*mut c_void; 16], // 128
}

// Written during class init and read afterwards.
unsafe impl Sync for EExtensionClass {}
unsafe impl Send for EExtensionClass {}

// Matches `struct _GTypeInfo` from `<gobject/gtype.h>`.

#[repr(C)]
pub struct GTypeInfo {
    pub class_size: u16,
    pub base_init: Option<unsafe extern "C" fn(*mut c_void)>,
    pub base_finalize: Option<unsafe extern "C" fn(*mut c_void)>,
    pub class_init: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    pub class_finalize: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    pub class_data: *const c_void,
    pub instance_size: u16,
    pub n_preallocs: u16,
    pub instance_init: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    pub value_table: *const c_void,
}

// Contains only function pointers and static pointers.
unsafe impl Sync for GTypeInfo {}

// Matches `struct _GTypeQuery` from `<gobject/gtype.h>`.

#[repr(C)]
pub struct GTypeQuery {
    pub type_: GType,
    pub type_name: *const c_char,
    pub class_size: c_uint,
    pub instance_size: c_uint,
}

// Matches `GtkActionEntry` from `<gtk/gtkactiongroup.h>`.

#[repr(C)]
pub struct GtkActionEntry {
    pub name: *const c_char,
    pub stock_id: *const c_char,
    pub label: *const c_char,
    pub accelerator: *const c_char,
    pub tooltip: *const c_char,
    pub callback: Option<unsafe extern "C" fn(*mut GtkAction, *mut c_void)>,
}

// Fields are null or static pointers.
unsafe impl Sync for GtkActionEntry {}

extern "C" {

    pub fn g_type_query(type_: GType, query: *mut GTypeQuery);

    pub fn g_type_name(type_: GType) -> *const c_char;

    pub fn g_type_is_a(type_: GType, is_a_type: GType) -> GBoolean;

    pub fn g_type_class_peek_parent(g_class: *mut c_void) -> *mut c_void;

    pub fn g_type_module_register_type(
        module: *mut GTypeModule,
        parent_type: GType,
        type_name: *const c_char,
        info: *const GTypeInfo,
        flags: c_uint,
    ) -> GType;

    pub fn g_object_set_data_full(
        object: *mut GObject,
        key: *const c_char,
        data: *mut c_void,
        destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    );

    pub fn g_object_get_data(object: *mut GObject, key: *const c_char) -> *mut c_void;

    pub fn g_signal_connect_data(
        instance: *mut c_void,
        detailed_signal: *const c_char,
        c_handler: Option<unsafe extern "C" fn()>,
        data: *mut c_void,
        destroy_data: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
        connect_flags: c_uint,
    ) -> c_ulong;

    pub fn g_error_free(error: *mut GError);

    pub fn g_free(mem: *mut c_void);

    pub fn e_extension_get_type() -> GType;

    pub fn e_extension_get_extensible(extension: *mut c_void) -> *mut EExtensible;

    pub fn e_shell_view_get_type() -> GType;

    pub fn e_shell_view_is_active(view: *mut EShellView) -> GBoolean;

    pub fn e_shell_view_get_shell_window(view: *mut EShellView) -> *mut EShellWindow;

    pub fn e_shell_window_get_ui_manager(window: *mut EShellWindow) -> *mut GtkUIManager;

    pub fn e_shell_window_get_action_group(
        window: *mut EShellWindow,
        group_name: *const c_char,
    ) -> *mut GtkActionGroup;

    pub fn e_lookup_action_group(
        ui_manager: *mut GtkUIManager,
        group_name: *const c_char,
    ) -> *mut GtkActionGroup;

    pub fn e_action_group_add_actions_localized(
        action_group: *mut GtkActionGroup,
        translation_domain: *const c_char,
        entries: *const GtkActionEntry,
        n_entries: c_uint,
        user_data: *mut c_void,
    );

    pub fn e_mail_shell_view_get_type() -> GType;

    pub fn e_msg_composer_get_type() -> GType;

    pub fn e_msg_composer_get_editor(composer: *mut EMsgComposer) -> *mut EHTMLEditor;

    pub fn e_html_editor_get_ui_manager(editor: *mut EHTMLEditor) -> *mut GtkUIManager;

    pub fn e_html_editor_get_action_group(
        editor: *mut EHTMLEditor,
        name: *const c_char,
    ) -> *mut GtkActionGroup;

    pub fn e_cal_shell_view_get_type() -> GType;

    pub fn gtk_ui_manager_remove_ui(manager: *mut GtkUIManager, merge_id: c_uint);

    pub fn gtk_ui_manager_ensure_update(manager: *mut GtkUIManager);

    pub fn gtk_ui_manager_add_ui_from_string(
        manager: *mut GtkUIManager,
        buffer: *const c_char,
        length: c_int,
        error: *mut *mut GError,
    ) -> c_uint;

    pub fn gtk_window_get_title(window: *mut c_void) -> *const c_char;
}
