// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The registry loads an account from its `.source` file before anything in the
// process has registered the `[JMAP Rebase]` extension type. The mail child
// must still get the account's rebase opt-in. A separate test binary, because
// another test that registers the type first would hide the failure.

use std::ffi::{CString, c_char};
use std::ptr;

use eds_sys::{
    CamelSettings, E_SOURCE_EXTENSION_MAIL_ACCOUNT, ESource, ESourceBackend, ESourceCamel,
    e_server_side_source_new, e_source_authentication_get_type, e_source_backend_set_backend_name,
    e_source_camel_generate_subtype, e_source_camel_get_extension_name,
    e_source_camel_get_settings, e_source_get_extension, e_source_mail_account_get_type,
    e_source_new_with_uid, e_source_registry_server_new, e_source_security_get_type,
    g_file_new_for_path,
};
use gobject_sys::g_object_unref;
use jmap_backend_collection::child_added::follow_collection;
use jmap_backend_collection::prepare_mail::MAIL_BACKEND_NAME;
use jmap_mail::server::ServerConfig;
use jmap_mail::settings::settings_type;

mod common;
use common::with_timeout;

const ACCOUNT: &str = "[Data Source]\nDisplayName=Rebase account\nEnabled=true\nParent=\n\n\
[Authentication]\nHost=127.0.0.1\nPort=8080\nUser=vera\n\n\
[Security]\nMethod=none\n\n\
[Collection]\nBackendName=jmap\nMailEnabled=true\n\n\
[JMAP Rebase]\nRebaseUrls=true\n";

/// The account as the registry loads it: an `EServerSideSource` read from a file.
fn account_from_file() -> *mut ESource {
    let path = std::env::temp_dir().join(format!("jmap-rebase-{}.source", std::process::id()));
    std::fs::write(&path, ACCOUNT).expect("write the account file");
    let path = CString::new(path.into_os_string().into_encoded_bytes()).expect("no NUL");
    let mut error = ptr::null_mut();
    // SAFETY: no-argument constructor, a NUL-terminated path the callee copies,
    // and a GError out-parameter. The server stays alive for the test process.
    let source = unsafe {
        let server = e_source_registry_server_new().cast();
        let file = g_file_new_for_path(path.as_ptr());
        let source = e_server_side_source_new(server, file, &mut error);
        g_object_unref(file.cast());
        source
    };
    assert!(!source.is_null(), "e_server_side_source_new failed");
    source
}

/// The mail store child as the setup module leaves it: only a service name.
fn mail_store() -> *mut ESource {
    // SAFETY: no arguments; the uid is a literal; the source creates and owns
    // the extension and the setter copies the name.
    unsafe {
        e_source_mail_account_get_type();
        let mut error = ptr::null_mut();
        let source =
            e_source_new_with_uid(c"jmap-mail-store".as_ptr(), ptr::null_mut(), &mut error);
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        let backend: *mut ESourceBackend =
            e_source_get_extension(source, E_SOURCE_EXTENSION_MAIL_ACCOUNT.as_ptr()).cast();
        e_source_backend_set_backend_name(backend, MAIL_BACKEND_NAME.as_ptr());
        source
    }
}

fn camel_settings(source: *mut ESource) -> *mut CamelSettings {
    // SAFETY: a live source; the subtype name is interned and never freed.
    unsafe {
        let name: *const c_char = e_source_camel_get_extension_name(MAIL_BACKEND_NAME.as_ptr());
        let extension: *mut ESourceCamel = e_source_get_extension(source, name).cast();
        assert!(!extension.is_null(), "no jmap Camel settings extension");
        e_source_camel_get_settings(extension)
    }
}

#[test]
fn a_mail_store_gets_the_rebase_opt_in_of_an_account_loaded_from_its_file() {
    with_timeout(|| {
        // SAFETY: no arguments, and a GType derived from CamelSettings. This is
        // what the registry does when it loads the Camel providers; it does not
        // register the `[JMAP Rebase]` type.
        unsafe {
            e_source_authentication_get_type();
            e_source_security_get_type();
            e_source_camel_generate_subtype(MAIL_BACKEND_NAME.as_ptr(), settings_type());
        }
        let account = account_from_file();
        let store = mail_store();

        // SAFETY: two live sources.
        unsafe { follow_collection(account, store) };

        // SAFETY: the settings object of a live source, only read from.
        let server = unsafe { ServerConfig::from_settings(camel_settings(store)) };
        assert!(
            server.as_ref().is_ok_and(|s| s.rebase_urls),
            "the mail store lost the account's [JMAP Rebase] opt-in: {server:?}"
        );
        // SAFETY: the test holds one reference to each.
        unsafe {
            g_object_unref(store.cast());
            g_object_unref(account.cast());
        }
    });
}
