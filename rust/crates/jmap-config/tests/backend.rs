// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The `EMailConfigServiceBackend` subclass: the type Evolution registers as an
// extension of its *Receiving Email* page, and the two things it says before
// any widget exists — which provider it is the backend *for*, and what a JMAP
// account starts life as.
//
// Everything goes through the class struct, because that is all Evolution ever
// touches. Both fields already hold something: `backend_name` is NULL on the
// abstract parent, which is a backend no page ever matches to a provider, and
// `new_collection` inherits an implementation that returns NULL, which is a
// backend whose account is not a collection at all. Neither failure produces an
// error — they produce a JMAP entry that is missing from the provider list, and
// an account that is committed as a bare mail source with no address book and
// no calendar behind it.
//
// What is not here is a live instance: constructing one runs Evolution's own
// `constructed`, which wants an `EMailConfigServicePage` to extend and so a GTK
// display this VM does not have. `JmapConfigServiceBackend::detached` stands in
// for the one vfunc that never reads the backend it is handed, exactly as
// `jmap-backend-collection`'s `dup_resource_id` test does.

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_COLLECTION, E_SOURCE_EXTENSION_SECURITY, ESource, ESourceBackend,
    ESourceSecurity, e_source_backend_get_backend_name, e_source_collection_get_identity,
    e_source_get_extension, e_source_has_extension, e_source_security_get_secure,
};
use evo_sys::{
    EMailConfigServiceBackend, EMailConfigServiceBackendClass,
    e_mail_config_service_backend_get_type,
};
use glib_sys::GFALSE;
use gobject_sys::{
    g_object_unref, g_type_class_peek, g_type_class_ref, g_type_class_unref, g_type_parent,
};
use jmap_backend_collection::collection_source::{parts_of, server_of, user_of};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::SourceError;
use jmap_backend_core::subclass::register_static;
use jmap_collection_sync::Parts;
use jmap_collection_sync::child_source::Connection;
use jmap_config::account::{Account, BACKEND_NAME, apply, read};
use jmap_config::backend::{JmapConfigServiceBackend, JmapConfigServiceBackendClass, is_complete};
use jmap_config::complete::{Incomplete, check};
use jmap_config::mail::MAIL_BACKEND_NAME;

/// The class Evolution would dispatch through, kept referenced for the test's
/// duration so the slots and the name stay valid.
struct Class(*mut JmapConfigServiceBackendClass);

impl Class {
    fn get() -> Self {
        let gtype = register_static::<JmapConfigServiceBackend>();
        assert_ne!(gtype, 0, "the setup backend type did not register");
        // SAFETY: the type is registered, so referencing its class runs
        // class_init and hands back a class struct of our own layout.
        Self(unsafe { g_type_class_ref(gtype) }.cast())
    }

    /// The `EMailConfigServiceBackendClass` half, which is where both the name
    /// and every vfunc live.
    fn vfuncs(&self) -> &EMailConfigServiceBackendClass {
        // SAFETY: the class is referenced and leads with the parent's.
        unsafe { &(*self.0).parent_class }
    }

    /// Calls `new_collection` the way Evolution's `constructed` does, and takes
    /// ownership of what it hands back.
    fn new_collection(&self) -> Collection {
        let mut backend = JmapConfigServiceBackend::detached();
        let new_collection = self
            .vfuncs()
            .new_collection
            .expect("class_init installed no new_collection");
        // SAFETY: the slot is filled and the detached instance is never read by
        // this vfunc — see the module comment.
        let source = unsafe {
            new_collection(ptr::from_mut(&mut *backend).cast::<EMailConfigServiceBackend>())
        };
        assert!(!source.is_null(), "new_collection answered NULL");
        Collection(source)
    }
}

impl Drop for Class {
    fn drop(&mut self) {
        // SAFETY: the reference taken in `get` is given back exactly once.
        unsafe { g_type_class_unref(self.0.cast()) };
    }
}

/// The parent's class, for the slots an override has to displace.
///
/// `g_type_class_peek` and not `_ref`: referencing our own subclass has already
/// initialised it, and it stays alive for as long as the process does.
fn parent_class() -> &'static EMailConfigServiceBackendClass {
    // SAFETY: `Class::get` referenced a subclass of it, so the parent class is
    // initialised and alive.
    unsafe {
        g_type_class_peek(e_mail_config_service_backend_get_type())
            .cast::<EMailConfigServiceBackendClass>()
            .as_ref()
    }
    .expect("Evolution's own class is not initialised")
}

/// The collection source a `new_collection` answered, read back with the
/// registry's own reader.
struct Collection(*mut ESource);

impl Collection {
    fn has_extension(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a NUL-terminated name.
        unsafe { e_source_has_extension(self.0, name.as_ptr()) != GFALSE }
    }

    /// `[Collection] BackendName`, which is what the registry keys the
    /// collection factory off.
    fn backend_name(&self) -> Option<String> {
        // SAFETY: a live source carrying the extension, and a getter that hands
        // back a string the source owns.
        unsafe {
            let collection = e_source_get_extension(self.0, E_SOURCE_EXTENSION_COLLECTION.as_ptr());
            read_string(e_source_backend_get_backend_name(
                collection.cast::<ESourceBackend>(),
            ))
        }
    }

    fn identity(&self) -> Option<String> {
        // SAFETY: as above.
        unsafe {
            let collection = e_source_get_extension(self.0, E_SOURCE_EXTENSION_COLLECTION.as_ptr());
            read_string(e_source_collection_get_identity(collection.cast()))
        }
    }

    fn secure(&self) -> bool {
        // SAFETY: as above.
        unsafe {
            let security = e_source_get_extension(self.0, E_SOURCE_EXTENSION_SECURITY.as_ptr());
            e_source_security_get_secure(security.cast::<ESourceSecurity>()) != GFALSE
        }
    }

    fn parts(&self) -> Parts {
        // SAFETY: a live source.
        unsafe { parts_of(self.0) }
    }

    fn user(&self) -> Option<String> {
        // SAFETY: a live source.
        unsafe { user_of(self.0) }
    }

    fn server(&self) -> Result<String, SourceError> {
        // SAFETY: a live source.
        unsafe { server_of(self.0) }.map(|server| server.origin)
    }

    /// Writes `account` onto the collection — what the widgets do to it while
    /// the user is typing, and what `check_complete` is then asked about.
    fn edited(self, account: &Account) -> Self {
        // SAFETY: a live source.
        unsafe { apply(self.0, account) };
        self
    }

    /// What `check_complete` answers about the account the source now says.
    fn complete(&self) -> bool {
        // SAFETY: a live source.
        unsafe { is_complete(self.0) }
    }

    /// The reason behind that answer, which the vfunc itself has nowhere to
    /// put: the account read back and checked, which is what `is_complete` is.
    fn refusal(&self) -> Result<(), Incomplete> {
        // SAFETY: a live source.
        check(&unsafe { read(self.0) })
    }
}

/// A finished account, as in `tests/complete.rs`: the one the manual test
/// recipe describes, and the state the entries are in when *Next* should go
/// sensitive.
fn finished() -> Account {
    Account {
        identity: "vera@example.com".to_owned(),
        connection: Connection {
            host: "jmap.example.com".to_owned(),
            port: Some(8443),
            user: Some("vera".to_owned()),
            auth_method: None,
            secure: true,
        },
        parts: Parts {
            mail: true,
            contacts: true,
            calendars: true,
        },
    }
}

impl Drop for Collection {
    fn drop(&mut self) {
        // SAFETY: `new_collection` is `(transfer full)`, so this owns the one
        // reference Evolution would have owned.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

#[test]
fn the_type_extends_the_class_evolution_instantiates_per_provider() {
    let gtype = register_static::<JmapConfigServiceBackend>();
    assert_eq!(
        unsafe { g_type_parent(gtype) },
        unsafe { e_mail_config_service_backend_get_type() },
        "the setup backend no longer derives from EMailConfigServiceBackend"
    );
}

#[test]
fn the_backend_is_named_after_the_camel_provider_it_configures() {
    // Evolution's *Receiving Email* page walks its extensions and matches each
    // one's `backend_name` against a Camel provider's protocol with `strcmp`.
    // A name that is not the provider's is not a mismatch anyone reports: it is
    // a JMAP entry that never appears in the provider list, and an account type
    // the user cannot choose.
    let class = Class::get();
    let name = class.vfuncs().backend_name;
    assert!(!name.is_null(), "class_init left backend_name NULL");
    // SAFETY: a NUL-terminated static string, which is what class_init put
    // there.
    assert_eq!(unsafe { CStr::from_ptr(name) }, MAIL_BACKEND_NAME);
    // The same string once more, from the other end: it is also what the
    // account this commits carries as `[Collection] BackendName`, so the
    // provider the page offers and the factory the registry looks up are one
    // name and not two that happen to agree today.
    assert_eq!(MAIL_BACKEND_NAME.to_str(), Ok(BACKEND_NAME));

    // And the state an unwritten slot would leave it in.
    assert!(
        parent_class().backend_name.is_null(),
        "Evolution's abstract class now names a backend of its own"
    );
}

#[test]
fn new_collection_displaces_the_inherited_one() {
    // Evolution's own answers NULL — "this backend has no collection" — which
    // is the right default for POP3 and the wrong one for a groupware account:
    // an account committed without a collection source is a mail account with
    // no address books and no calendars behind it, and nothing says so.
    let class = Class::get();
    let ours = class
        .vfuncs()
        .new_collection
        .expect("class_init installed no new_collection");
    let inherited = parent_class()
        .new_collection
        .expect("Evolution installs its own new_collection");
    assert!(
        !std::ptr::fn_addr_eq(ours, inherited),
        "new_collection is still Evolution's, which answers NULL"
    );
}

#[test]
fn the_collection_offered_is_an_account_the_registry_recognises() {
    let collection = Class::get().new_collection();

    // `[Collection]` is what makes the file an account rather than a lone mail
    // source, and its `BackendName` is the key the registry looks the collection
    // factory up by.
    assert!(collection.has_extension(E_SOURCE_EXTENSION_COLLECTION));
    assert_eq!(collection.backend_name().as_deref(), Some(BACKEND_NAME));
}

#[test]
fn the_collection_offered_is_the_account_the_dialog_starts_from() {
    let collection = Class::get().new_collection();

    // The three boxes start ticked and the connection starts secure, which is
    // `defaults::from_identity` said onto a source: a scratch collection that
    // read back as an account with every part switched off would be one the
    // user has to repair before it is what they asked for.
    assert_eq!(
        collection.parts(),
        Parts {
            mail: true,
            contacts: true,
            calendars: true,
        }
    );
    assert!(collection.secure());
}

#[test]
fn the_collection_offered_names_nobody_and_nowhere_yet() {
    let collection = Class::get().new_collection();

    // Evolution builds this one in `constructed`, before the user has typed
    // anything at all, so there is no address to derive a server from — and a
    // guess made here would be a well-known probe aimed at a domain nobody
    // named. The registry's reader agrees it is not connectable yet, which is
    // the same verdict `complete::check` reaches from the other side.
    // `None` and not `Some("")`: EDS's own setters strip what they are given
    // and store nothing for a string that is empty afterwards, so the empty
    // identity this writes reads back as the absent one — which is the reading
    // that matters, since it is also what the registry would find in a keyfile
    // with no `Identity=` line at all.
    assert_eq!(collection.identity(), None);
    assert_eq!(collection.user(), None);
    assert_eq!(collection.server(), Err(SourceError::MissingHost));
}

#[test]
fn check_complete_displaces_the_inherited_one() {
    // Evolution's own answers TRUE: a backend whose account is finished as soon
    // as a provider is picked, which is right for a POP3 account with nothing to
    // fill in and wrong for one that needs an address and a server. Left
    // inherited it is not a missing feature that shows up anywhere — it is an
    // assistant whose *Next* is sensitive over an empty account, and an account
    // committed with no host that then fails in the registry.
    let class = Class::get();
    let ours = class
        .vfuncs()
        .check_complete
        .expect("class_init installed no check_complete");
    let inherited = parent_class()
        .check_complete
        .expect("Evolution installs its own check_complete");
    assert!(
        !std::ptr::fn_addr_eq(ours, inherited),
        "check_complete is still Evolution's, which accepts anything"
    );
}

#[test]
fn the_account_the_dialog_starts_from_is_not_one_it_may_commit() {
    // `new_collection`'s own account, which names nobody and nowhere: the
    // assistant opens on it, and it is exactly the account that must not be
    // committed. The refusal names the identity and not the server, because the
    // identity is the page the user is on.
    let collection = Class::get().new_collection();
    assert!(!collection.complete());
    assert_eq!(collection.refusal(), Err(Incomplete::MissingIdentity));
}

#[test]
fn a_finished_account_is_one_the_setup_may_commit() {
    let collection = Class::get().new_collection().edited(&finished());
    assert_eq!(collection.refusal(), Ok(()));
    assert!(collection.complete());
    // And it is the same account on the way out: the registry's reader gets the
    // server the check accepted, rather than a second reading of the source.
    assert_eq!(
        collection.server().as_deref(),
        Ok("https://jmap.example.com:8443")
    );
}

#[test]
fn plaintext_to_a_server_that_is_not_this_machine_is_refused() {
    // The project's TLS rule (M3), reached through the vfunc's own path rather
    // than through `check` alone: the entries write `[Security] Method=none`
    // onto the collection source, and what `check_complete` is asked about is
    // that source.
    let mut account = finished();
    account.connection.secure = false;
    let collection = Class::get().new_collection().edited(&account);
    assert!(!collection.complete());
    assert_eq!(
        collection.refusal(),
        Err(Incomplete::Server(SourceError::InsecureTransport(
            "jmap.example.com".to_owned()
        )))
    );
}

#[test]
fn plaintext_to_this_machine_is_still_the_mock_server() {
    let mut account = finished();
    account.connection.host = "localhost".to_owned();
    account.connection.secure = false;
    let collection = Class::get().new_collection().edited(&account);
    assert!(collection.complete());
}

#[test]
fn a_backend_with_no_collection_source_commits_nothing() {
    // What the vfunc has to answer when `new_collection` failed: FALSE, which
    // greys *Next* out rather than committing an account nothing can read back.
    // Silently, because the failure was reported where it happened — a critical
    // per keystroke would bury it in copies of itself.
    // SAFETY: NULL is the one non-source this is documented to take.
    assert!(!unsafe { is_complete(ptr::null_mut()) });
}
