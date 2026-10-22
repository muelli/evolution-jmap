mod ffi;
mod msg_composer_extension;
mod shell_view_extension;

/// Register extension types for this module.
///
/// # Safety
/// `type_module` must be a valid pointer that Evolution passes.
#[no_mangle]
pub unsafe extern "C" fn e_module_load(type_module: *mut ffi::GTypeModule) {
    unsafe {
        shell_view_extension::register_type(type_module);
        msg_composer_extension::register_type(type_module);
    }
}

/// Provide the unload hook that Evolution requires.
///
/// # Safety
/// `_type_module` must be a valid pointer that Evolution passes.
#[no_mangle]
pub unsafe extern "C" fn e_module_unload(_type_module: *mut ffi::GTypeModule) {}
