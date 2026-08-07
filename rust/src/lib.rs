mod ffi;
mod msg_composer_extension;
mod shell_view_extension;

/// Called by Evolution when the module is loaded.
///
/// Registers every GObject extension type the plugin provides.
///
/// # Safety
///
/// `type_module` must be the valid `GTypeModule *` that Evolution passes to
/// this symbol; it must remain alive for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn e_module_load(type_module: *mut ffi::GTypeModule) {
    shell_view_extension::register_type(type_module);
    msg_composer_extension::register_type(type_module);
}

/// Called by Evolution just before the module shared library is unloaded.
///
/// # Safety
///
/// `type_module` must be the valid `GTypeModule *` provided by Evolution.
#[no_mangle]
pub unsafe extern "C" fn e_module_unload(_type_module: *mut ffi::GTypeModule) {}
