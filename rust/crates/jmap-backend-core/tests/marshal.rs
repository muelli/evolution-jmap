// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The pieces of the vfunc boundary that are the same for every backend:
//! reading a string EDS owns, writing one back, filling a list out-parameter,
//! and finding the password EDS fetched from libsecret.
//!
//! Each is exercised the way EDS will use it — the out-parameters are freed
//! with the function EDS would call, so a pointer into a Rust `String` shows up
//! here as a failure rather than as a crash in a factory process.

use std::ffi::{CStr, CString};
use std::ptr;

use eds_sys::{
    CamelAddress, CamelInternetAddress, E_SOURCE_CREDENTIAL_PASSWORD,
    E_SOURCE_EXTENSION_AUTHENTICATION, ESourceAuthentication, camel_address_new,
    camel_internet_address_get_type, camel_internet_address_new, e_named_parameters_free,
    e_named_parameters_new, e_named_parameters_set, e_source_get_extension, e_source_has_extension,
    e_source_new,
};
use glib_sys::{GSList, g_free, g_slist_free_full, g_slist_length, g_slist_nth_data, gchar};
use gobject_sys::g_object_unref;
use jmap_backend_core::marshal;

/// A live `CamelInternetAddress`, upcast to `*mut CamelAddress` the way a
/// vfunc's declared argument type would arrive, for the borrow-helper tests
/// below. The caller frees it with `g_object_unref`.
fn internet_address() -> *mut CamelAddress {
    // SAFETY: `camel_internet_address_new` returns a live, owned instance.
    unsafe { camel_internet_address_new().cast() }
}

// ---------------------------------------------------------------------------
// read_string

/// The two spellings of "absent" have to mean the same thing. NULL is what the
/// EDS cache writes for a sync tag it does not have; `""` reaches the same
/// place through a hand-edited cache, and an empty state sent on to the server
/// is a state rather than the absence of one.
#[test]
fn a_null_and_an_empty_c_string_both_read_as_absent() {
    // SAFETY: a null pointer and 'static NUL-terminated literals.
    unsafe {
        assert_eq!(marshal::read_string(ptr::null()), None);
        assert_eq!(marshal::read_string(c"".as_ptr()), None);
        assert_eq!(
            marshal::read_string(c"state-7".as_ptr()).as_deref(),
            Some("state-7")
        );
    }
}

// ---------------------------------------------------------------------------
// set_out_string

#[test]
fn an_out_string_is_a_copy_the_caller_owns() {
    let mut out: *mut gchar = ptr::null_mut();
    unsafe {
        marshal::set_out_string(&mut out, "state-7");
        assert_eq!(CStr::from_ptr(out).to_str().unwrap(), "state-7");
        // Only valid because the marshalling allocated it rather than pointing
        // into the Rust string above.
        g_free(out.cast());
        // A NULL out-parameter is the GLib convention for "not interested".
        marshal::set_out_string(ptr::null_mut(), "state-8");
    }
}

// ---------------------------------------------------------------------------
// set_out_list

#[test]
fn an_out_list_is_written_when_the_caller_asked_for_one() {
    let mut out: *mut GSList = ptr::null_mut();
    unsafe {
        marshal::set_out_list(&mut out, || {
            glib_sys::g_slist_prepend(ptr::null_mut(), marshal::dup_string("K1").cast())
        });
        assert_eq!(g_slist_length(out), 1);
        let first = g_slist_nth_data(out, 0).cast::<gchar>();
        assert_eq!(CStr::from_ptr(first).to_str().unwrap(), "K1");
        g_slist_free_full(out, Some(g_free));
    }
}

/// A list built for a caller that did not want it would have to be freed with
/// the right per-node function, and there is nobody left to do that — so it is
/// not built at all.
#[test]
fn an_out_list_the_caller_did_not_ask_for_is_never_built() {
    let mut built = false;
    // SAFETY: NULL is the "not interested" out-parameter.
    unsafe {
        marshal::set_out_list(ptr::null_mut(), || {
            built = true;
            ptr::null_mut()
        });
    }
    assert!(!built, "the list was built for a NULL out-parameter");
}

// ---------------------------------------------------------------------------
// password

#[test]
fn the_password_is_read_out_of_the_named_parameters() {
    unsafe {
        let params = e_named_parameters_new();
        let value = CString::new("hunter2").unwrap();
        e_named_parameters_set(
            params,
            E_SOURCE_CREDENTIAL_PASSWORD.as_ptr(),
            value.as_ptr(),
        );
        assert_eq!(marshal::password(params).as_deref(), Some("hunter2"));
        e_named_parameters_free(params);
    }
}

/// Reporting a stored-but-empty password as absent would make `connect_sync`
/// ask for a prompt, and a user who enters nothing would be prompted again
/// forever. Sending it and being told it is wrong terminates. This is why
/// `password` does not simply defer to [`marshal::read_string`].
#[test]
fn an_empty_stored_password_is_present_not_absent() {
    unsafe {
        let params = e_named_parameters_new();
        let value = CString::new("").unwrap();
        e_named_parameters_set(
            params,
            E_SOURCE_CREDENTIAL_PASSWORD.as_ptr(),
            value.as_ptr(),
        );
        assert_eq!(marshal::password(params).as_deref(), Some(""));
        e_named_parameters_free(params);
    }
}

#[test]
fn credentials_without_a_password_report_none() {
    unsafe {
        let params = e_named_parameters_new();
        assert_eq!(marshal::password(params), None);
        e_named_parameters_free(params);
    }
}

/// EDS calls `connect_sync` with NULL credentials on the first attempt, before
/// it has asked libsecret for anything.
#[test]
fn null_credentials_report_none() {
    // SAFETY: NULL is what EDS passes before it has any credentials.
    unsafe {
        assert_eq!(marshal::password(ptr::null()), None);
    }
}

// ---------------------------------------------------------------------------
// checked_borrow_ptr_or

/// A NULL pointer is "nothing here", not a type mismatch — the same rule
/// [`checked_borrow`]/[`checked_borrow_ptr`] apply, kept even though this
/// variant's failure case carries a caller-supplied error instead of `None`.
#[test]
fn a_null_pointer_is_ok_none_not_the_supplied_error() {
    // SAFETY: NULL is a valid input to a checked-borrow helper.
    let result = unsafe {
        marshal::checked_borrow_ptr_or::<CamelAddress, CamelInternetAddress, &str>(
            ptr::null_mut(),
            camel_internet_address_get_type(),
            "boom",
        )
    };
    assert_eq!(result, Ok(None));
}

/// A pointer of the right type is a borrow, not the error.
#[test]
fn a_pointer_of_the_right_type_is_ok_some() {
    // SAFETY: a live `CamelInternetAddress`, which is-a `CamelAddress`.
    unsafe {
        let address: *mut CamelAddress = camel_internet_address_new().cast();
        let result = marshal::checked_borrow_ptr_or::<CamelAddress, CamelInternetAddress, &str>(
            address,
            camel_internet_address_get_type(),
            "boom",
        );
        assert_eq!(result, Ok(Some(address.cast())));
        g_object_unref(address.cast());
    }
}

/// A pointer of the wrong type is the caller's error, not `Ok(None)` — the
/// distinction `envelope.rs::internet` needs to turn a wrong-type argument
/// into a refusal rather than silently treating it as an absent address.
#[test]
fn a_pointer_of_the_wrong_type_is_the_supplied_error() {
    // SAFETY: a live `CamelAddress` that is not a `CamelInternetAddress`.
    unsafe {
        let plain = camel_address_new();
        let result = marshal::checked_borrow_ptr_or::<CamelAddress, CamelInternetAddress, &str>(
            plain,
            camel_internet_address_get_type(),
            "boom",
        );
        assert_eq!(result, Err("boom"));
        g_object_unref(plain.cast());
    }
}

// ---------------------------------------------------------------------------
// dispatched_borrow

#[test]
fn dispatched_borrow_reports_null_as_absent() {
    // SAFETY: NULL is a valid input to a borrow helper.
    let result = unsafe {
        marshal::dispatched_borrow::<CamelAddress, CamelInternetAddress>(ptr::null_mut())
    };
    assert!(result.is_none());
}

/// Unlike `checked_borrow`, this helper trusts vfunc dispatch instead of
/// checking a `GType` — a wrong-type pointer here would already be a GObject
/// bug, not something this call can catch. What it must still get right is
/// the cast itself: the returned reference has to be the same address as the
/// pointer that came in.
#[test]
fn dispatched_borrow_casts_a_live_pointer_to_the_same_address() {
    let address = internet_address();
    // SAFETY: `address` is a live instance, as `dispatched_borrow` requires.
    let borrowed =
        unsafe { marshal::dispatched_borrow::<CamelAddress, CamelInternetAddress>(address) };
    assert!(std::ptr::eq(
        borrowed.expect("a live pointer must borrow"),
        address.cast()
    ));
    // SAFETY: nothing above still borrows `address`.
    unsafe { g_object_unref(address.cast()) };
}

// ---------------------------------------------------------------------------
// checked_borrow

#[test]
fn checked_borrow_reports_null_as_absent() {
    // SAFETY: NULL is a valid input to a checked-borrow helper.
    let result = unsafe {
        marshal::checked_borrow::<CamelAddress, CamelInternetAddress>(
            ptr::null_mut(),
            camel_internet_address_get_type(),
        )
    };
    assert!(result.is_none());
}

#[test]
fn checked_borrow_accepts_a_pointer_of_the_declared_type() {
    let address = internet_address();
    // SAFETY: `address` is a live `CamelInternetAddress`.
    let borrowed = unsafe {
        marshal::checked_borrow::<CamelAddress, CamelInternetAddress>(
            address,
            camel_internet_address_get_type(),
        )
    };
    assert!(std::ptr::eq(
        borrowed.expect("a same-type pointer must borrow"),
        address.cast()
    ));
    // SAFETY: nothing above still borrows `address`.
    unsafe { g_object_unref(address.cast()) };
}

#[test]
fn checked_borrow_refuses_a_pointer_of_the_wrong_type() {
    // SAFETY: a live `CamelAddress` that is not a `CamelInternetAddress`.
    unsafe {
        let plain = camel_address_new();
        let result = marshal::checked_borrow::<CamelAddress, CamelInternetAddress>(
            plain,
            camel_internet_address_get_type(),
        );
        assert!(result.is_none());
        g_object_unref(plain.cast());
    }
}

// ---------------------------------------------------------------------------
// checked_borrow_ptr

#[test]
fn checked_borrow_ptr_reports_null_as_absent() {
    // SAFETY: NULL is a valid input to a checked-borrow helper.
    let result = unsafe {
        marshal::checked_borrow_ptr::<CamelAddress, CamelInternetAddress>(
            ptr::null_mut(),
            camel_internet_address_get_type(),
        )
    };
    assert!(result.is_none());
}

#[test]
fn checked_borrow_ptr_accepts_a_pointer_of_the_declared_type() {
    let address = internet_address();
    // SAFETY: `address` is a live `CamelInternetAddress`.
    let result = unsafe {
        marshal::checked_borrow_ptr::<CamelAddress, CamelInternetAddress>(
            address,
            camel_internet_address_get_type(),
        )
    };
    assert_eq!(result, Some(address.cast()));
    // SAFETY: nothing above still borrows `address`.
    unsafe { g_object_unref(address.cast()) };
}

#[test]
fn checked_borrow_ptr_refuses_a_pointer_of_the_wrong_type() {
    // SAFETY: a live `CamelAddress` that is not a `CamelInternetAddress`.
    unsafe {
        let plain = camel_address_new();
        let result = marshal::checked_borrow_ptr::<CamelAddress, CamelInternetAddress>(
            plain,
            camel_internet_address_get_type(),
        );
        assert!(result.is_none());
        g_object_unref(plain.cast());
    }
}

// ---------------------------------------------------------------------------
// extension_if_present

/// `e_source_get_extension` creates the extension it cannot find, which is
/// wrong for a read-only lookup — this is the property `extension_if_present`
/// exists to add. A freshly created source has no `Authentication` extension
/// until something asks to write one.
#[test]
fn extension_if_present_does_not_create_the_extension_it_cannot_find() {
    let mut error = ptr::null_mut();
    // SAFETY: the documented arguments — no D-Bus object, the default main
    // context, and a `GError` out-parameter.
    let source = unsafe { e_source_new(ptr::null_mut(), ptr::null_mut(), &mut error) };
    assert!(!source.is_null());

    // SAFETY: `source` is live and outlives the call.
    let found = unsafe {
        marshal::extension_if_present::<ESourceAuthentication>(
            source,
            E_SOURCE_EXTENSION_AUTHENTICATION,
        )
    };
    assert!(found.is_none());
    // SAFETY: only reading whether the lookup above created the extension.
    assert_eq!(
        unsafe { e_source_has_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()) },
        glib_sys::GFALSE,
        "extension_if_present created the extension it was only asked to look up"
    );
    // SAFETY: nothing above still borrows `source`.
    unsafe { g_object_unref(source.cast()) };
}

#[test]
fn extension_if_present_returns_the_extension_once_it_exists() {
    let mut error = ptr::null_mut();
    // SAFETY: as above.
    let source = unsafe { e_source_new(ptr::null_mut(), ptr::null_mut(), &mut error) };
    assert!(!source.is_null());

    // SAFETY: `source` is live; this is the ordinary, extension-creating call.
    let created = unsafe {
        e_source_get_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr())
            .cast::<ESourceAuthentication>()
    };
    assert!(!created.is_null());

    // SAFETY: `source` is live and outlives the call.
    let found = unsafe {
        marshal::extension_if_present::<ESourceAuthentication>(
            source,
            E_SOURCE_EXTENSION_AUTHENTICATION,
        )
    };
    assert_eq!(found, Some(created));
    // SAFETY: nothing above still borrows `source`.
    unsafe { g_object_unref(source.cast()) };
}
