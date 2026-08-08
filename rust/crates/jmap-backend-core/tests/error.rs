// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The GError a backend returns is not just a log line: EDS routes on domain
// and code. G_IO_ERROR_CANCELLED suppresses the error dialog,
// E_CLIENT_ERROR_AUTHENTICATION_FAILED re-prompts for credentials, and
// E_CLIENT_ERROR_REPOSITORY_OFFLINE makes a meta backend fall back to its
// cache instead of reporting failure. Getting the mapping wrong is a UX bug
// that no amount of correct JMAP will fix.

use eds_sys::{
    E_CLIENT_ERROR_AUTHENTICATION_FAILED, E_CLIENT_ERROR_OTHER_ERROR,
    E_CLIENT_ERROR_PERMISSION_DENIED, E_CLIENT_ERROR_REPOSITORY_OFFLINE, e_client_error_quark,
};
use glib_sys::{GError, g_error_free};
use jmap_backend_core::error::set_gerror;
use jmap_client::Error;
use jmap_proto::error::{MethodError, RequestError};
use std::collections::BTreeMap;
use std::ffi::CStr;
use std::ptr;

/// Maps `err`, then returns the resulting (domain, code, message).
fn mapped(err: &Error) -> (u32, i32, String) {
    let mut out: *mut GError = ptr::null_mut();
    unsafe { set_gerror(&mut out, err) };
    assert!(!out.is_null(), "set_gerror produced no error");
    unsafe {
        let triple = (
            (*out).domain,
            (*out).code,
            CStr::from_ptr((*out).message)
                .to_string_lossy()
                .into_owned(),
        );
        g_error_free(out);
        triple
    }
}

#[test]
fn cancellation_maps_to_the_gio_domain_evolution_special_cases() {
    let (domain, code, _) = mapped(&Error::Cancelled);
    assert_eq!(domain, unsafe { gio_sys::g_io_error_quark() });
    assert_eq!(code, gio_sys::G_IO_ERROR_CANCELLED);
}

#[test]
fn an_unauthorised_response_asks_evolution_to_re_authenticate() {
    let (domain, code, _) = mapped(&Error::Http {
        status: 401,
        problem: None,
    });
    assert_eq!(domain, unsafe { e_client_error_quark() });
    assert_eq!(code, E_CLIENT_ERROR_AUTHENTICATION_FAILED as i32);
}

#[test]
fn a_forbidden_response_maps_to_permission_denied() {
    let (_, code, _) = mapped(&Error::Http {
        status: 403,
        problem: None,
    });
    assert_eq!(code, E_CLIENT_ERROR_PERMISSION_DENIED as i32);
}

/// A server we cannot reach is exactly the case the meta backend's offline
/// cache exists for, so it must not surface as a generic failure.
#[test]
fn a_transport_failure_maps_to_repository_offline() {
    let (domain, code, message) = mapped(&Error::Transport("dns lookup failed".into()));
    assert_eq!(domain, unsafe { e_client_error_quark() });
    assert_eq!(code, E_CLIENT_ERROR_REPOSITORY_OFFLINE as i32);
    assert!(message.contains("dns lookup failed"), "{message}");
}

#[test]
fn protocol_level_failures_carry_their_description_into_the_message() {
    let method = MethodError::new("unknownMethod").with_description("no such method");
    let (domain, code, message) = mapped(&Error::Method(method));
    assert_eq!(domain, unsafe { e_client_error_quark() });
    assert_eq!(code, E_CLIENT_ERROR_OTHER_ERROR as i32);
    assert!(message.contains("unknownMethod"), "{message}");

    let (_, code, message) = mapped(&Error::Protocol("missing response for call id".into()));
    assert_eq!(code, E_CLIENT_ERROR_OTHER_ERROR as i32);
    assert!(message.contains("missing response"), "{message}");
}

/// The problem details from the server are the only thing that explains a 500
/// to the user; dropping them would leave "HTTP 500" and nothing else.
#[test]
fn http_problem_details_reach_the_message() {
    let (_, code, message) = mapped(&Error::Http {
        status: 500,
        problem: Some(RequestError {
            error_type: "urn:ietf:params:jmap:error:serverFail".into(),
            status: Some(500),
            detail: Some("backend exploded".into()),
            extra: BTreeMap::new(),
        }),
    });
    assert_eq!(code, E_CLIENT_ERROR_OTHER_ERROR as i32);
    assert!(message.contains("backend exploded"), "{message}");
}

/// GLib's convention: a NULL out-parameter means the caller is not interested.
#[test]
fn a_null_out_parameter_is_a_no_op_rather_than_a_crash() {
    unsafe { set_gerror(ptr::null_mut(), &Error::Cancelled) };
}
