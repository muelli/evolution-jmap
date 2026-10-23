// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests for the mail part of a JMAP account that a collection backend owns: the
// `prepare_mail` vfunc, called through the EDS function
// `e_collection_backend_factory_prepare_mail`.
//
// This backend does not create mail children. EDS
// `collection_backend_load_resources()` deletes the cache file of each child for
// which `dup_resource_id` returns NULL. All reference implementations
// (`module-google-backend.c`, `module-yahoo-backend.c`, and `e-ews-backend.c` of
// evolution-ews) return NULL for the mail extensions. Thus the mail account,
// identity and transport sources are not cached children of the collection.
// They are usual registry sources with the account as `Parent`, and the setup UI
// creates them. `prepare_mail` is the only hook where a vendor backend sets
// their service. `tests/fan_out.rs` and `tests/populate.rs` test the children
// that this backend creates. This file tests the three sources that it only
// fills in.
//
// The test calls the vfunc through the public EDS wrapper, not directly. Only
// this path goes through the class struct. The wrapper reads `prepare_mail` at
// the offset that the parent class expects: the slot after the two fields that
// `class_init` also writes. `tests/factory.rs` checks that the slot holds a
// pointer. This file checks that EDS dispatches through that slot.

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_MAIL_ACCOUNT, E_SOURCE_EXTENSION_MAIL_IDENTITY,
    E_SOURCE_EXTENSION_MAIL_SUBMISSION, E_SOURCE_EXTENSION_MAIL_TRANSPORT,
    ECollectionBackendFactoryClass, ESource, ESourceBackend, ESourceMailAccount,
    ESourceMailSubmission, e_collection_backend_factory_prepare_mail,
    e_source_backend_get_backend_name, e_source_get_extension, e_source_get_uid,
    e_source_has_extension, e_source_mail_account_get_identity_uid,
    e_source_mail_submission_get_transport_uid, e_source_new,
};
use glib_sys::GFALSE;
use gobject_sys::{g_object_unref, g_type_class_ref, g_type_class_unref, g_type_create_instance};
use jmap_backend_collection::backend::JmapCollectionBackend;
use jmap_backend_collection::factory::JmapCollectionFactory;
use jmap_backend_collection::prepare_mail::MAIL_BACKEND_NAME;
use jmap_backend_collection::resource_id::resource_id_of;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::subclass::register_static;

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One of the three `ESource`s that the setup UI gives `prepare_mail`: a source
/// with a uid and no extensions.
///
/// EDS uses `e_source_new (NULL, NULL, &error)` for a source without a keyfile.
/// It creates the uid. This type creates no extension, because the tests check
/// the extensions. `e_source_get_extension` for an unregistered
/// `ESourceExtension` subclass returns NULL and does not fail (`e-source.c`,
/// `source_find_extension_classes`).
struct Blank(*mut ESource);

impl Blank {
    fn new() -> Self {
        let mut error = ptr::null_mut();
        // SAFETY: the documented arguments are no related object, no file, and
        // a GError out-parameter.
        let source = unsafe { e_source_new(ptr::null_mut(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new failed");
        Self(source)
    }

    fn uid(&self) -> Option<String> {
        // SAFETY: a live source. The source owns the uid that it returns.
        unsafe { read_string(e_source_get_uid(self.0)) }
    }

    fn has_extension(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a NUL-terminated header constant.
        unsafe { e_source_has_extension(self.0, name.as_ptr()) != GFALSE }
    }

    /// The `ESourceBackend:backend-name` of the extension `name`. It first
    /// asserts that the extension is present, because `e_source_get_extension`
    /// creates a missing extension.
    fn backend_name(&self, name: &CStr) -> Option<String> {
        assert!(
            self.has_extension(name),
            "the source has no {name:?} to read a backend name from"
        );
        // SAFETY: the extension is present, so this returns the extension that
        // the source owns. Its name is NULL or a NUL-terminated string with the
        // same lifetime.
        unsafe {
            let backend: *mut ESourceBackend = e_source_get_extension(self.0, name.as_ptr()).cast();
            read_string(e_source_backend_get_backend_name(backend))
        }
    }
}

impl Drop for Blank {
    fn drop(&mut self) {
        // SAFETY: this releases the reference from `e_source_new` once.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// The three sources after this factory prepared them.
struct Prepared {
    account: Blank,
    identity: Blank,
    transport: Blank,
}

impl Prepared {
    fn new() -> Self {
        // Static registration, not the module entry point. `tests/factory.rs`
        // tests `e_module_load`, and there is no module here. `register_static`
        // returns an already-registered type, so all tests here get one type.
        let gtype = register_static::<JmapCollectionFactory>();
        assert_ne!(gtype, 0, "the factory type is not registered");

        // `g_type_create_instance`, not `g_object_new`, as `tests/factory.rs`
        // explains. `EExtension:extensible` is CONSTRUCT_ONLY, so `g_object_new`
        // without properties causes an unrelated critical from `E_IS_EXTENSIBLE`.
        // SAFETY: a registered, instantiatable type.
        let factory = unsafe { g_type_create_instance(gtype) };
        assert!(!factory.is_null(), "g_type_create_instance returned NULL");

        let prepared = Self {
            account: Blank::new(),
            identity: Blank::new(),
            transport: Blank::new(),
        };

        // SAFETY: a live factory of a type derived from ECollectionBackendFactory,
        // and three live sources. The call takes no references.
        unsafe {
            e_collection_backend_factory_prepare_mail(
                factory.cast(),
                prepared.account.0,
                prepared.identity.0,
                prepared.transport.0,
            );
        }

        // SAFETY: this releases the reference from instance creation once.
        unsafe { g_object_unref(factory.cast()) };

        prepared
    }
}

/// Evolution receives mail through the mail account. Its backend name is the
/// Camel protocol of `libcameljmap.so`, not the factory name of this crate.
///
/// The inherited `prepare_mail` writes no backend name. A mail account source
/// without one causes no error. Evolution shows the account but cannot open it,
/// because it calls `camel_provider_get` for the empty protocol.
#[test]
fn the_mail_account_is_served_by_the_jmap_camel_provider() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let prepared = Prepared::new();

        assert!(
            prepared
                .account
                .has_extension(E_SOURCE_EXTENSION_MAIL_ACCOUNT),
            "the account source is not a mail account"
        );
        assert_eq!(
            prepared
                .account
                .backend_name(E_SOURCE_EXTENSION_MAIL_ACCOUNT)
                .as_deref(),
            MAIL_BACKEND_NAME.to_str().ok(),
        );
    });
}

/// The transport uses the same name, because JMAP sends over the session that
/// it reads through. The `jmap-mail` provider registers one protocol with a
/// store type and a transport type. Thus there is no second name, unlike `smtp`
/// for IMAP accounts.
#[test]
fn the_transport_is_the_same_provider_and_not_a_second_one() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let prepared = Prepared::new();

        assert!(
            prepared
                .transport
                .has_extension(E_SOURCE_EXTENSION_MAIL_TRANSPORT),
            "the transport source is not a mail transport"
        );
        assert_eq!(
            prepared
                .transport
                .backend_name(E_SOURCE_EXTENSION_MAIL_TRANSPORT)
                .as_deref(),
            MAIL_BACKEND_NAME.to_str().ok(),
        );
    });
}

/// The backend name must be the protocol that Camel sends to `libcameljmap.so`.
/// The file `libcameljmap.urls` gives Camel this protocol.
///
/// The test reads the file from the source tree, and has no crate dependency.
/// `jmap-mail` links Camel and this crate does not. A build dependency only for
/// this check would be too much. `tests/provider.rs` of `jmap-mail` does the
/// same check from the other side. The CTest `install-camel-provider` checks
/// that the file gets to the directory that Camel scans.
#[test]
fn the_name_written_is_the_protocol_camel_dlopens_the_provider_for() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let urls = include_str!("../../jmap-mail/libcameljmap.urls");
        let protocols: Vec<&str> = urls.lines().collect();

        assert_eq!(
            protocols,
            [MAIL_BACKEND_NAME.to_str().expect("ASCII")],
            "the mail sources would name a protocol libcameljmap.urls does not \
             claim, so Camel would never dlopen the provider for them"
        );
    });
}

/// If a subclass does not chain up, the three sources are not linked into one
/// account. No error occurs: each source is still valid, but no source refers
/// to a different one.
///
/// The parent class writes both links: `identity-uid` on the account and
/// `transport-uid` in `[Mail Submission]` of the identity. The test compares
/// both with the uids of the sources that they must name.
#[test]
fn chaining_up_left_the_three_sources_pointing_at_each_other() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let prepared = Prepared::new();

        // SAFETY: the extension is present (`has_extension` asserts it first), so
        // this returns the extension that the source owns. Its uid is NULL or a
        // NUL-terminated string that the extension owns.
        let identity_uid = unsafe {
            assert!(
                prepared
                    .account
                    .has_extension(E_SOURCE_EXTENSION_MAIL_ACCOUNT),
                "the account source is not a mail account"
            );
            let account: *mut ESourceMailAccount = e_source_get_extension(
                prepared.account.0,
                E_SOURCE_EXTENSION_MAIL_ACCOUNT.as_ptr(),
            )
            .cast();
            read_string(e_source_mail_account_get_identity_uid(account))
        };
        assert_eq!(
            identity_uid,
            prepared.identity.uid(),
            "the mail account does not name the identity it was prepared with"
        );

        assert!(
            prepared
                .identity
                .has_extension(E_SOURCE_EXTENSION_MAIL_IDENTITY),
            "the identity source is not a mail identity, so nothing recognises it \
             as one"
        );

        // SAFETY: as above, for the `[Mail Submission]` extension.
        let transport_uid = unsafe {
            assert!(
                prepared
                    .identity
                    .has_extension(E_SOURCE_EXTENSION_MAIL_SUBMISSION),
                "the identity source has no submission settings"
            );
            let submission: *mut ESourceMailSubmission = e_source_get_extension(
                prepared.identity.0,
                E_SOURCE_EXTENSION_MAIL_SUBMISSION.as_ptr(),
            )
            .cast();
            read_string(e_source_mail_submission_get_transport_uid(submission))
        };
        assert_eq!(
            transport_uid,
            prepared.transport.uid(),
            "the identity does not name the transport it was prepared with, so \
             sending would pick some other account's"
        );
    });
}

/// The identity is a person, not a service. A backend name would make it a
/// service.
///
/// `collection_backend_child_is_mail()` treats a source with one of the three
/// mail extensions as mail. An identity with `[Mail Account]` would be a second,
/// empty receiving account. Evolution would show it, with no server.
#[test]
fn the_identity_is_not_turned_into_a_service_of_its_own() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let prepared = Prepared::new();

        for extension in [
            E_SOURCE_EXTENSION_MAIL_ACCOUNT,
            E_SOURCE_EXTENSION_MAIL_TRANSPORT,
        ] {
            assert!(
                !prepared.identity.has_extension(extension),
                "the identity was given {extension:?}, which makes it a mail \
                 service in its own right"
            );
        }
    });
}

/// This collection never claims one of the three sources as a child. This rule
/// keeps `prepare_mail` and `dup_resource_id` consistent.
///
/// EDS calls `dup_resource_id` for each `.source` file in the cache directory
/// of the backend. NULL means "not mine", and for a file in that directory it
/// means "delete this". Both results are wrong for a mail source that looks like
/// a child. These three sources are in the source directory of the registry,
/// with the account as parent, and the setup UI manages them.
#[test]
fn no_prepared_mail_source_is_read_as_a_child_of_this_collection() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let prepared = Prepared::new();

        for (what, source) in [
            ("the mail account", &prepared.account),
            ("the identity", &prepared.identity),
            ("the transport", &prepared.transport),
        ] {
            // SAFETY: a live source.
            assert_eq!(
                unsafe { resource_id_of(source.0) },
                None,
                "{what} was read as a child this backend created"
            );
        }
    });
}

/// `Prepared::new()` is the only place in the tests of this crate that gets the
/// factory through `register_static` alone. No `module::load` calls
/// `remember_backend_type` first. The doc comment of `factory::backend_type`
/// names this test case.
///
/// Thus only this test can find a failure of `class_init_types` to register the
/// backend type before `class_init`. `class_init` asks for the type again under
/// the GLib class-initialisation lock. There, a miss logs a critical and leaves
/// `backend_type` zero, with no retry.
#[test]
fn the_backend_type_is_valid_when_the_factory_is_registered_alone() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let gtype = register_static::<JmapCollectionFactory>();
        assert_ne!(gtype, 0, "the factory type is not registered");

        // SAFETY: the type is registered. A reference to its class runs
        // class_init, which writes `backend_type`. The class starts with
        // ECollectionBackendFactoryClass.
        let class = unsafe { g_type_class_ref(gtype) }.cast::<ECollectionBackendFactoryClass>();
        assert!(!class.is_null(), "the factory class failed to initialise");
        // SAFETY: a live, referenced class.
        let backend_type = unsafe { (*class).backend_type };

        // This call is outside class_init, so it registers the backend if
        // necessary. The result is the correct value, which `class` must hold.
        let expected = register_static::<JmapCollectionBackend>();
        assert_ne!(expected, 0, "the backend type does not register at all");
        assert_eq!(
            backend_type, expected,
            "class_init wrote no valid backend type, so new_backend would \
             build nothing for an account found this way"
        );

        // SAFETY: this releases the reference from above once.
        unsafe { g_type_class_unref(class.cast()) };
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_prepare_mail_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}
