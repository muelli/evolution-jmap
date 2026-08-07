//! Raw FFI declarations for GLib, GObject, GTK 3, and Evolution APIs.
//!
//! All structs are declared `#[repr(C)]` with layouts that match the C ABI on
//! 64-bit Linux (the only platform Evolution targets in practice).
//!
//! Linking is handled entirely by `build.rs` via `pkg-config`; no `#[link]`
//! attributes are needed here.

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::os::raw::{c_char, c_int, c_uint, c_ulong, c_void};

// ── Basic GLib types ──────────────────────────────────────────────────────────

/// `GType` — a pointer-sized integer that uniquely identifies a GObject type.
///
/// GLib defines `GType` as `gsize`, which is pointer-sized. This crate targets
/// 64-bit Linux only; a compile-time assertion below enforces that.
pub type GType = usize;

// Fail to compile on anything other than a 64-bit target.
const _: () = assert!(
    std::mem::size_of::<usize>() == 8,
    "GType is assumed to be 8 bytes (64-bit platforms only)"
);

/// `gboolean` — GLib boolean (C `int`; 0 = false, non-zero = true).
pub type GBoolean = c_int;

// ── Opaque handle types ───────────────────────────────────────────────────────
//
// These types are only ever used through pointers; we do not need (or want) to
// know their internal layout.

/// `GTypeModule` — the GObject that Evolution passes to `e_module_load`.
pub type GTypeModule = c_void;

/// `GObject` — base instance type for all GObject instances.
pub type GObject = c_void;

/// `GError` — a GLib error object.
pub type GError = c_void;

// Evolution opaque handles
pub type EExtensible  = c_void;
pub type EShellView   = c_void;
pub type EShellWindow = c_void;
pub type EMsgComposer = c_void;
pub type EHTMLEditor  = c_void;

// GTK 3 opaque handles
pub type GtkUIManager  = c_void;
pub type GtkActionGroup = c_void;
pub type GtkAction      = c_void;

// ── GTypeClass / GTypeInstance ────────────────────────────────────────────────

/// First field of every GObject *class* struct (`struct _GTypeClass`).
#[repr(C)]
pub struct GTypeClass {
    pub g_type: GType,
}

/// First field of every GObject *instance* struct (`struct _GTypeInstance`).
#[repr(C)]
pub struct GTypeInstance {
    /// Pointer to the class struct of this instance's type.
    pub g_class: *mut GTypeClass,
}

// ── GObjectClass layout (64-bit Linux, GLib ≥ 2.54) ─────────────────────────
//
// Must exactly match `struct _GObjectClass` from `<gobject/gobject.h>`.
// Total size: 136 bytes on LP64.

#[repr(C)]
pub struct GObjectClass {
    pub g_type_class: GTypeClass,           // 8
    pub construct_properties: *mut c_void,  // 8
    pub constructor:
        Option<unsafe extern "C" fn(GType, c_uint, *mut c_void) -> *mut GObject>, // 8
    pub set_property:
        Option<unsafe extern "C" fn(*mut GObject, c_uint, *const c_void, *const c_void)>, // 8
    pub get_property:
        Option<unsafe extern "C" fn(*mut GObject, c_uint, *mut c_void, *const c_void)>,   // 8
    pub dispose:   Option<unsafe extern "C" fn(*mut GObject)>, // 8
    pub finalize:  Option<unsafe extern "C" fn(*mut GObject)>, // 8
    pub dispatch_properties_changed:
        Option<unsafe extern "C" fn(*mut GObject, c_uint, *mut *mut c_void)>, // 8
    pub notify:    Option<unsafe extern "C" fn(*mut GObject, *mut c_void)>,  // 8
    pub constructed: Option<unsafe extern "C" fn(*mut GObject)>,             // 8
    pub flags:     usize,                   // 8
    pub n_construct_properties: usize,      // 8
    pub pspecs:    *mut c_void,             // 8
    pub n_pspecs:  usize,                   // 8
    pub pdummy:    [*mut c_void; 3],        // 24
    // total: 136
}

// ── EExtensionClass layout ────────────────────────────────────────────────────
//
// Matches `struct _EExtensionClass` from `<libebackend/e-extension.h>`.
//
// struct _EExtensionClass {
//     GObjectClass parent_class;   // 136
//     GType        extensible_type; //   8
//     gpointer     reserved[16];   // 128
// };

#[repr(C)]
pub struct EExtensionClass {
    pub parent_class:    GObjectClass,   // 136
    pub extensible_type: GType,          //   8
    pub _reserved:       [*mut c_void; 16], // 128
}

// Safety: this struct is only written once during class initialisation (single-
// threaded), and only read afterwards — safe to share across threads.
unsafe impl Sync for EExtensionClass {}
unsafe impl Send for EExtensionClass {}

// ── GTypeInfo ─────────────────────────────────────────────────────────────────
//
// Matches `struct _GTypeInfo` from `<gobject/gtype.h>`.

#[repr(C)]
pub struct GTypeInfo {
    pub class_size:     u16,
    pub base_init:      Option<unsafe extern "C" fn(*mut c_void)>,
    pub base_finalize:  Option<unsafe extern "C" fn(*mut c_void)>,
    pub class_init:     Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    pub class_finalize: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    pub class_data:     *const c_void,
    pub instance_size:  u16,
    pub n_preallocs:    u16,
    pub instance_init:  Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    pub value_table:    *const c_void,
}

// Safety: GTypeInfo only contains function pointers and a *const to static
// data; safe to share across threads.
unsafe impl Sync for GTypeInfo {}

// ── GTypeQuery ────────────────────────────────────────────────────────────────
//
// Matches `struct _GTypeQuery` from `<gobject/gtype.h>`.

#[repr(C)]
pub struct GTypeQuery {
    pub type_:         GType,
    pub type_name:     *const c_char,
    pub class_size:    c_uint,
    pub instance_size: c_uint,
}

// ── GtkActionEntry ────────────────────────────────────────────────────────────
//
// Matches `GtkActionEntry` from `<gtk/gtkactiongroup.h>`.

#[repr(C)]
pub struct GtkActionEntry {
    pub name:        *const c_char,
    pub stock_id:    *const c_char,
    pub label:       *const c_char,
    pub accelerator: *const c_char,
    pub tooltip:     *const c_char,
    pub callback:    Option<unsafe extern "C" fn(*mut GtkAction, *mut c_void)>,
}

// Safety: all fields are either null or point to 'static string literals /
// function items; safe to share across threads.
unsafe impl Sync for GtkActionEntry {}

// ── GLib / GObject function declarations ──────────────────────────────────────

extern "C" {
    // ── GType system ──────────────────────────────────────────────────────────

    /// Fill `*query` with the sizes and name of `type_`.
    pub fn g_type_query(type_: GType, query: *mut GTypeQuery);

    /// Return the C type name for `type_` (do not free the result).
    pub fn g_type_name(type_: GType) -> *const c_char;

    /// Return non-zero if `type_` is identical to or derives from `is_a_type`.
    pub fn g_type_is_a(type_: GType, is_a_type: GType) -> GBoolean;

    /// Return the parent class of `g_class`.
    pub fn g_type_class_peek_parent(g_class: *mut c_void) -> *mut c_void;

    /// Register a new GObject type into `module` and return its `GType`.
    pub fn g_type_module_register_type(
        module: *mut GTypeModule,
        parent_type: GType,
        type_name: *const c_char,
        info: *const GTypeInfo,
        flags: c_uint,
    ) -> GType;

    // ── GObject ───────────────────────────────────────────────────────────────

    /// Attach arbitrary data to a GObject, with an optional destroy notifier.
    pub fn g_object_set_data_full(
        object: *mut GObject,
        key: *const c_char,
        data: *mut c_void,
        destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    );

    /// Retrieve data previously attached with `g_object_set_data_full`.
    pub fn g_object_get_data(
        object: *mut GObject,
        key: *const c_char,
    ) -> *mut c_void;

    /// Connect a callback to a GObject signal.
    pub fn g_signal_connect_data(
        instance: *mut c_void,
        detailed_signal: *const c_char,
        c_handler: Option<unsafe extern "C" fn()>,
        data: *mut c_void,
        destroy_data: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
        connect_flags: c_uint,
    ) -> c_ulong;

    /// Free a `GError`.
    pub fn g_error_free(error: *mut GError);

    // ── GLib utilities ────────────────────────────────────────────────────────

    pub fn g_free(mem: *mut c_void);

    // ── Evolution: libebackend ────────────────────────────────────────────────

    /// GType of `EExtension` (the base for all extension objects).
    pub fn e_extension_get_type() -> GType;

    /// Return the `EExtensible` object this extension is attached to.
    pub fn e_extension_get_extensible(extension: *mut c_void) -> *mut EExtensible;

    // ── Evolution: evolution-shell-3.0 ────────────────────────────────────────

    /// GType of `EShellView`.
    pub fn e_shell_view_get_type() -> GType;

    /// Return non-zero if `view` is the currently active shell view.
    pub fn e_shell_view_is_active(view: *mut EShellView) -> GBoolean;

    /// Return the `EShellWindow` that owns `view`.
    pub fn e_shell_view_get_shell_window(view: *mut EShellView) -> *mut EShellWindow;

    /// Return the `GtkUIManager` for `window`.
    pub fn e_shell_window_get_ui_manager(window: *mut EShellWindow) -> *mut GtkUIManager;

    /// Return the named `GtkActionGroup` from `window`.
    pub fn e_shell_window_get_action_group(
        window: *mut EShellWindow,
        group_name: *const c_char,
    ) -> *mut GtkActionGroup;

    /// Like `gtk_ui_manager_get_action_groups` but looks up by name.
    pub fn e_lookup_action_group(
        ui_manager: *mut GtkUIManager,
        group_name: *const c_char,
    ) -> *mut GtkActionGroup;

    /// Add `GtkActionEntry` items to a group with localisation support.
    pub fn e_action_group_add_actions_localized(
        action_group: *mut GtkActionGroup,
        translation_domain: *const c_char,
        entries: *const GtkActionEntry,
        n_entries: c_uint,
        user_data: *mut c_void,
    );

    // ── Evolution: evolution-mail-3.0 ─────────────────────────────────────────

    /// GType of the mail shell view (`EMailShellView`).
    pub fn e_mail_shell_view_get_type() -> GType;

    /// GType of `EMsgComposer`.
    pub fn e_msg_composer_get_type() -> GType;

    /// Return the `EHTMLEditor` embedded in `composer`.
    pub fn e_msg_composer_get_editor(composer: *mut EMsgComposer) -> *mut EHTMLEditor;

    /// Return the `GtkUIManager` owned by `editor`.
    pub fn e_html_editor_get_ui_manager(editor: *mut EHTMLEditor) -> *mut GtkUIManager;

    /// Return the named action group from `editor`.
    pub fn e_html_editor_get_action_group(
        editor: *mut EHTMLEditor,
        name: *const c_char,
    ) -> *mut GtkActionGroup;

    // ── Evolution: evolution-calendar-3.0 ────────────────────────────────────

    /// GType of the calendar shell view (`ECalShellView`).
    pub fn e_cal_shell_view_get_type() -> GType;

    // ── GTK 3 ─────────────────────────────────────────────────────────────────

    /// Remove a UI merge (identified by `merge_id`) from `manager`.
    pub fn gtk_ui_manager_remove_ui(manager: *mut GtkUIManager, merge_id: c_uint);

    /// Flush any pending UI changes.
    pub fn gtk_ui_manager_ensure_update(manager: *mut GtkUIManager);

    /// Parse a UI description string and merge it into `manager`.
    /// Returns the merge ID (0 on error).
    pub fn gtk_ui_manager_add_ui_from_string(
        manager: *mut GtkUIManager,
        buffer: *const c_char,
        length: c_int,
        error: *mut *mut GError,
    ) -> c_uint;

    /// Return the title of `window` (do not free).
    pub fn gtk_window_get_title(window: *mut c_void) -> *const c_char;
}
