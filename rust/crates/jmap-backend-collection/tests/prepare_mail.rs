// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The mail half of a JMAP account, as far as a collection backend owns it: the
// `prepare_mail` vfunc, driven through EDS's own
// `e_collection_backend_factory_prepare_mail`.
//
// Which is *not* the same thing as this backend creating mail children, and the
// distinction is the whole reason this file is small. EDS's own
// `collection_backend_load_resources()` deletes the cache file of any child
// whose `dup_resource_id` answers NULL, and every reference implementation —
// `module-google-backend.c`, `module-yahoo-backend.c`, evolution-ews's
// `e-ews-backend.c` — answers NULL for exactly the mail extensions. So the mail
// account, identity and transport sources are not the collection's cached
// children at all: they are ordinary registry sources whose `Parent` is the
// account, created by the setup UI, and `prepare_mail` is the one hook a vendor
// backend gets to say what service they are. `tests/fan_out.rs` and
// `tests/populate.rs` cover the children this backend *does* create; this
// covers the three it only fills in.
//
// The vfunc is reached through EDS's public wrapper rather than by calling our
// function, because that is the only way the test crosses the class struct: the
// wrapper reads `prepare_mail` at the offset the *parent* believes it is at,
// one slot past the two fields `class_init` also writes. `tests/factory.rs`
// checks that slot as a pointer, which says the write landed somewhere; this
// says it landed in the slot EDS dispatches through.

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_MAIL_ACCOUNT, E_SOURCE_EXTENSION_MAIL_IDENTITY,
    E_SOURCE_EXTENSION_MAIL_SUBMISSION, E_SOURCE_EXTENSION_MAIL_TRANSPORT, ESource, ESourceBackend,
    ESourceMailAccount, ESourceMailSubmission, e_collection_backend_factory_prepare_mail,
    e_source_backend_get_backend_name, e_source_get_extension, e_source_get_uid,
    e_source_has_extension, e_source_mail_account_get_identity_uid,
    e_source_mail_submission_get_transport_uid, e_source_new,
};
use glib_sys::GFALSE;
use gobject_sys::{g_object_unref, g_type_create_instance};
use jmap_backend_collection::factory::JmapCollectionFactory;
use jmap_backend_collection::prepare_mail::MAIL_BACKEND_NAME;
use jmap_backend_collection::resource_id::resource_id_of;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::subclass::register_static;

/// One of the three `ESource`s the setup UI would hand `prepare_mail`: a source
/// with a uid and nothing on it.
///
/// `e_source_new (NULL, NULL, &error)` is what EDS itself uses for a source with
/// no keyfile behind it yet; it mints the uid. Nothing here creates an
/// extension — that is precisely what is being tested, since an
/// `e_source_get_extension` for an unregistered `ESourceExtension` subclass
/// returns NULL rather than failing (`e-source.c`, `source_find_extension_classes`).
struct Blank(*mut ESource);

impl Blank {
    fn new() -> Self {
        let mut error = ptr::null_mut();
        // SAFETY: no related object, no file, and a GError out-parameter are
        // the documented arguments.
        let source = unsafe { e_source_new(ptr::null_mut(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new failed");
        Self(source)
    }

    fn uid(&self) -> Option<String> {
        // SAFETY: a live source; the uid it returns is owned by it.
        unsafe { read_string(e_source_get_uid(self.0)) }
    }

    fn has_extension(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a NUL-terminated header constant.
        unsafe { e_source_has_extension(self.0, name.as_ptr()) != GFALSE }
    }

    /// The `ESourceBackend:backend-name` under `name`, which is only asked for
    /// once the extension is known to be there — `e_source_get_extension`
    /// creates what it cannot find, and a test that created the extension it
    /// then asserts the presence of would assert nothing.
    fn backend_name(&self, name: &CStr) -> Option<String> {
        assert!(
            self.has_extension(name),
            "the source has no {name:?} to read a backend name from"
        );
        // SAFETY: the extension is present, so this returns the source's own,
        // owned by it; the name it holds is NULL or a NUL-terminated string
        // with the same lifetime.
        unsafe {
            let backend: *mut ESourceBackend = e_source_get_extension(self.0, name.as_ptr()).cast();
            read_string(e_source_backend_get_backend_name(backend))
        }
    }
}

impl Drop for Blank {
    fn drop(&mut self) {
        // SAFETY: the reference `e_source_new` transferred, given back once.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// The three sources after this factory has prepared them.
struct Prepared {
    account: Blank,
    identity: Blank,
    transport: Blank,
}

impl Prepared {
    fn new() -> Self {
        // Registered statically rather than through the module entry point:
        // `tests/factory.rs` owns the question of what `e_module_load` does, and
        // there is no module here for the type to belong to. `register_static`
        // hands an already-registered type straight back, so every test in this
        // binary gets the same one.
        let gtype = register_static::<JmapCollectionFactory>();
        assert_ne!(gtype, 0, "the factory type is not registered");

        // `g_type_create_instance` rather than `g_object_new`, for the reason
        // `tests/factory.rs` sets out: `EExtension:extensible` is
        // CONSTRUCT_ONLY, so a property-less `g_object_new` earns a critical
        // from `E_IS_EXTENSIBLE` that has nothing to do with this test.
        // SAFETY: a registered, instantiatable type.
        let factory = unsafe { g_type_create_instance(gtype) };
        assert!(!factory.is_null(), "g_type_create_instance returned NULL");

        let prepared = Self {
            account: Blank::new(),
            identity: Blank::new(),
            transport: Blank::new(),
        };

        // SAFETY: a live factory of a type derived from ECollectionBackendFactory
        // and three live sources; the call takes no references.
        unsafe {
            e_collection_backend_factory_prepare_mail(
                factory.cast(),
                prepared.account.0,
                prepared.identity.0,
                prepared.transport.0,
            );
        }

        // SAFETY: the reference instance creation left behind, given back once.
        unsafe { g_object_unref(factory.cast()) };

        prepared
    }
}

/// The mail account is what Evolution *receives* through, and the backend name
/// is the Camel protocol it is received through — `libcameljmap.so`'s, not this
/// crate's factory name.
///
/// The inherited `prepare_mail` writes no backend name at all, and a mail
/// account source without one is not an error either: it is an account
/// Evolution shows and cannot open, because `camel_provider_get` is asked for
/// the empty protocol.
#[test]
fn the_mail_account_is_served_by_the_jmap_camel_provider() {
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
}

/// And so is the transport, under the *same* name, because JMAP submits over
/// the session it reads through: `jmap-mail`'s provider registers one protocol
/// with both a store and a transport type in it, which is why there is no
/// second name here the way IMAP accounts have `smtp` beside them.
#[test]
fn the_transport_is_the_same_provider_and_not_a_second_one() {
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
}

/// The name written above has to be the protocol Camel routes to
/// `libcameljmap.so`, and that file is what tells Camel so.
///
/// Read out of the source tree rather than depended on as a crate: `jmap-mail`
/// links Camel and this crate does not, and the two strings meeting in a build
/// dependency would be a heavier answer than the one thing that has to be true.
/// It is the same check `jmap-mail`'s own `tests/provider.rs` makes from the
/// other side, and CTest's `install-camel-provider` checks the same file
/// reaches the directory Camel scans.
#[test]
fn the_name_written_is_the_protocol_camel_dlopens_the_provider_for() {
    let urls = include_str!("../../jmap-mail/libcameljmap.urls");
    let protocols: Vec<&str> = urls.lines().collect();

    assert_eq!(
        protocols,
        [MAIL_BACKEND_NAME.to_str().expect("ASCII")],
        "the mail sources would name a protocol libcameljmap.urls does not \
         claim, so Camel would never dlopen the provider for them"
    );
}

/// A subclass that forgets to chain up loses the wiring that makes the three
/// sources one account, and loses it silently: each source is still a valid
/// source of its own kind, and nothing points at anything.
///
/// Both links are the parent's work — `identity-uid` on the account and
/// `transport-uid` on the identity's `[Mail Submission]` — and both are uids
/// this test reads back off the very sources they should name.
#[test]
fn chaining_up_left_the_three_sources_pointing_at_each_other() {
    let prepared = Prepared::new();

    // SAFETY: the extension is present (asserted through `has_extension`
    // first), so this returns the source's own, and the uid it holds is NULL or
    // a NUL-terminated string owned by it.
    let identity_uid = unsafe {
        assert!(
            prepared
                .account
                .has_extension(E_SOURCE_EXTENSION_MAIL_ACCOUNT),
            "the account source is not a mail account"
        );
        let account: *mut ESourceMailAccount =
            e_source_get_extension(prepared.account.0, E_SOURCE_EXTENSION_MAIL_ACCOUNT.as_ptr())
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

    // SAFETY: as above, with `[Mail Submission]` the extension.
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
}

/// The identity is a person, not a service, and giving it a backend name would
/// make it one.
///
/// `collection_backend_child_is_mail()` treats a source carrying any of the
/// three mail extensions as mail, so an identity that also carried
/// `[Mail Account]` would be a second, empty receiving account of this user's —
/// visible in Evolution, pointed at nothing.
#[test]
fn the_identity_is_not_turned_into_a_service_of_its_own() {
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
}

/// And none of the three is ever claimed as a child of this collection, which
/// is the rule that keeps `prepare_mail` and `dup_resource_id` from
/// contradicting each other.
///
/// `dup_resource_id` is asked about every `.source` file in the backend's cache
/// directory and its NULL means "not mine" — but for a file that *is* in that
/// directory it means "delete this". A mail source that this vfunc had made
/// look like a child of ours would be one or the other, and both are wrong:
/// these three live in the registry's own source directory, parented to the
/// account, and are the setup UI's to manage.
#[test]
fn no_prepared_mail_source_is_read_as_a_child_of_this_collection() {
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
}
