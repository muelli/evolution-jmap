mod ffi;
mod msg_composer_extension;
mod shell_view_extension;

/// Register this module's extension types.
///
/// # Safety
/// `type_module` must be a valid pointer from Evolution.
#[no_mangle]
pub unsafe extern "C" fn e_module_load(type_module: *mut ffi::GTypeModule) {
    unsafe {
        shell_view_extension::register_type(type_module);
        msg_composer_extension::register_type(type_module);
    }
}

/// Unload hook required by Evolution.
///
/// # Safety
/// `_type_module` must be a valid pointer from Evolution.
#[no_mangle]
pub unsafe extern "C" fn e_module_unload(_type_module: *mut ffi::GTypeModule) {}
