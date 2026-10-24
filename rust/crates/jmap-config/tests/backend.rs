// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The `EMailConfigServiceBackend` subclass: the type Evolution registers as an
// extension of its *Receiving Email* page. Before any widget exists, it tells two
// things: the provider it is the backend *for*, and the initial JMAP account.
//
// All calls go through the class struct, because Evolution uses only that. Both
// fields already have a value. On the abstract parent, `backend_name` is NULL,
// so no page matches the backend to a provider. The inherited `new_collection`
// returns NULL, so the account is not a collection. Neither failure gives an
// error. The results are a JMAP entry that is not in the provider list, and an
// account committed as a bare mail source with no address book and no calendar.
//
// This file has no live instance. To construct one runs Evolution's
// `constructed`, which needs an `EMailConfigServicePage` and thus a GTK display.
// This VM has no display. `JmapConfigServiceBackend::detached` replaces the
// instance for the one vfunc that never reads its backend. The `dup_resource_id`
// test of `jmap-backend-collection` does the same.

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_COLLECTION, E_SOURCE_EXTENSION_SECURITY,
    ESource, ESourceBackend, ESourceSecurity, e_source_backend_get_backend_name,
    e_source_collection_get_identity, e_source_get_extension, e_source_has_extension, e_source_new,
    e_source_security_get_secure,
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
use jmap_backend_core::oauth2::OAUTH2_AUTH_METHOD;
use jmap_backend_core::source::{ConnectTarget, SourceError};
use jmap_backend_core::subclass::register_static;
use jmap_collection_sync::Parts;
use jmap_collection_sync::child_source::Connection;
use jmap_config::account::{Account, BACKEND_NAME, apply, read};
use jmap_config::backend::{
    JmapConfigServiceBackend, JmapConfigServiceBackendClass, commit, is_complete, seed_existing,
    setup,
};
use jmap_config::complete::{Incomplete, check};
use jmap_config::mail::{MAIL_BACKEND_NAME, apply_server};
use jmap_config::oauth2;

// Initialise the parent classes of the Evolution types before the tests start threads.
jmap_backend_core::initialise_parents_before_main!(
    jmap_config::backend::JmapConfigServiceBackend,
    jmap_config::config_lookup::JmapConfigLookup,
    jmap_config::source_config::JmapBookConfig,
    jmap_config::source_config::JmapCalConfig,
);

/// The class that Evolution dispatches through. The test keeps a reference, so
/// the slots and the name stay valid.
struct Class(*mut JmapConfigServiceBackendClass);

impl Class {
    fn get() -> Self {
        let gtype = register_static::<JmapConfigServiceBackend>();
        assert_ne!(gtype, 0, "the setup backend type did not register");
        // SAFETY: the type is registered. Thus a reference to its class runs
        // class_init and returns a class struct with the layout of this crate.
        Self(unsafe { g_type_class_ref(gtype) }.cast())
    }

    /// The `EMailConfigServiceBackendClass` part, which holds the name and all
    /// vfuncs.
    fn vfuncs(&self) -> &EMailConfigServiceBackendClass {
        // SAFETY: the class is referenced and starts with the parent class.
        unsafe { &(*self.0).parent_class }
    }

    /// Calls `new_collection` as Evolution's `constructed` does, and takes
    /// ownership of the result.
    fn new_collection(&self) -> Collection {
        let mut backend = JmapConfigServiceBackend::detached();
        let new_collection = self
            .vfuncs()
            .new_collection
            .expect("class_init installed no new_collection");
        // SAFETY: the slot is filled, and this vfunc never reads the detached
        // instance (refer to the module comment).
        let source = unsafe {
            new_collection(ptr::from_mut(&mut *backend).cast::<EMailConfigServiceBackend>())
        };
        assert!(!source.is_null(), "new_collection answered NULL");
        Collection(source)
    }
}

impl Drop for Class {
    fn drop(&mut self) {
        // SAFETY: this releases the reference from `get` exactly one time.
        unsafe { g_type_class_unref(self.0.cast()) };
    }
}

/// The parent class, which holds the slots that an override must replace.
///
/// `g_type_class_peek`, not `_ref`: the reference to the subclass already
/// initialised it, and it stays alive until the process stops.
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

/// The collection source from `new_collection`, read with the reader of the
/// registry.
struct Collection(*mut ESource);

impl Collection {
    fn has_extension(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a NUL-terminated name.
        unsafe { e_source_has_extension(self.0, name.as_ptr()) != GFALSE }
    }

    /// `[Collection] BackendName`, the key that the registry uses to find the
    /// collection factory.
    fn backend_name(&self) -> Option<String> {
        // SAFETY: a live source with the extension, and a getter that returns a
        // string that the source owns.
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

    fn server(&self) -> Result<ConnectTarget, SourceError> {
        // SAFETY: a live source.
        unsafe { server_of(self.0) }.map(|server| server.target)
    }

    /// Writes `account` onto the collection, as the widgets do while the user
    /// types. `check_complete` then examines this collection.
    fn edited(self, account: &Account) -> Self {
        // SAFETY: a live source.
        unsafe { apply(self.0, account) };
        self
    }

    /// Writes a server that the user types by hand over the default. This is the
    /// one field that a JMAP setup can need to correct. The domain of the address
    /// gives only the *well-known* URL, and the server can be at a different
    /// location.
    fn with_server(self, host: &str) -> Self {
        // SAFETY: a live source, read and then written.
        let mut account = unsafe { read(self.0) };
        account.connection.host = host.to_owned();
        // SAFETY: as above.
        unsafe { apply(self.0, &account) };
        self
    }

    /// Does to this collection what `setup_defaults` does with the address from
    /// the identity page. This is the part of the vfunc that does not need the page.
    fn setup(&self, address: &str) -> bool {
        // SAFETY: a live source.
        unsafe { setup(self.0, address) }
    }

    /// The `check_complete` answer for the account that the source now holds.
    fn complete(&self) -> bool {
        // SAFETY: a live source.
        unsafe { is_complete(self.0) }
    }

    /// The reason for that answer, which the vfunc cannot return. It reads and
    /// checks the account, as `is_complete` does.
    fn refusal(&self) -> Result<(), Incomplete> {
        // SAFETY: a live source.
        check(&unsafe { read(self.0) })
    }
}

/// A finished account, as in `tests/complete.rs`. The manual test recipe
/// describes it, and *Next* must become sensitive when the entries hold it.
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
        oauth2_registered: false,
    }
}

impl Drop for Collection {
    fn drop(&mut self) {
        // SAFETY: `new_collection` is `(transfer full)`, so this owns the one
        // reference that Evolution otherwise owns.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// The scratch mail source that a backend holds next to its collection. It is
/// the return value of `e_mail_config_service_backend_get_source`, and the one
/// source that `commit_changes` can write.
///
/// The blank state is important because Evolution creates the source blank.
/// `e_mail_config_assistant` makes one `ESource` for each provider and writes
/// only `[Mail Account] BackendName` (or `[Mail Transport] BackendName`) into
/// it. Other code must write all other values. For JMAP, only this crate can do
/// this: the server is on the account, not on a page with a mail server entry.
struct MailSource(*mut ESource);

impl MailSource {
    fn blank() -> Self {
        let mut error = ptr::null_mut();
        // SAFETY: no related object, no file, and a `GError` out-parameter are
        // the documented arguments. `new_collection` makes the same call.
        let source = unsafe { e_source_new(ptr::null_mut(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new failed");
        Self(source)
    }

    /// A mail source that already has a real server, as the editor's scratch
    /// copy of an existing standalone account does (`docs/examples/jmap-mock-
    /// standalone-mail.source`): `[Authentication]`/`[Security]` straight on
    /// the mail source, no `[Collection]` anywhere.
    fn configured(connection: &Connection) -> Self {
        let source = Self::blank();
        // SAFETY: a live source, just created above.
        unsafe { apply_server(source.0, connection) };
        source
    }

    /// Asks for the extension, but does not read through it, as in
    /// `tests/mail.rs`. `e_source_get_extension` *creates* a missing extension.
    /// Thus a read through it changes "the commit wrote nothing" into "the commit
    /// wrote nothing in a group that it added".
    fn has_extension(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a NUL-terminated header constant.
        unsafe { e_source_has_extension(self.0, name.as_ptr()) != GFALSE }
    }

    /// The server that this mail source now connects to, read with the reader
    /// for the account. `[Authentication]` and `[Security]` are the same pair of
    /// groups on all kinds of source.
    fn server(&self) -> Result<ConnectTarget, SourceError> {
        // SAFETY: a live source.
        unsafe { server_of(self.0) }.map(|server| server.target)
    }

    fn user(&self) -> Option<String> {
        // SAFETY: a live source.
        unsafe { user_of(self.0) }
    }
}

impl Drop for MailSource {
    fn drop(&mut self) {
        // SAFETY: this holds the only reference.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// If this slot is empty, Look Up in the account assistant can find a complete
/// JMAP account, but the wizard offers only imapx/smtp. The `auto_configure` of
/// the base class always answers FALSE. `e_mail_config_service_page_auto_configure`
/// selects the winning backend only from the answers of that vfunc.
///
/// This was observed live against Fastmail on 2026-08-23: SRV, RFC 8414
/// discovery and RFC 7591 registration all succeeded, but the wizard did not
/// offer the result. A call to the slot needs a real `EConfigLookup` and a
/// collection source bound to a page, so only a functional test can do it. A
/// unit test CAN make sure that the slot is installed. That is the failure that
/// this test found.
#[test]
fn class_init_displaces_the_inherited_auto_configure() {
    let class = Class::get();
    // The slot is never NULL: GObject copies the parent vtable, and the default
    // of the parent always answers FALSE. class_init must REPLACE it. The
    // assertion has the same shape as in `new_collection_displaces_the_inherited_one`.
    let ours = class
        .vfuncs()
        .auto_configure
        .expect("class_init installed no auto_configure");
    let inherited = parent_class()
        .auto_configure
        .expect("Evolution installs its own auto_configure");
    assert!(
        !std::ptr::fn_addr_eq(ours, inherited),
        "auto_configure is still Evolution's, which answers FALSE \
         unconditionally — the assistant's Look Up can never offer JMAP \
         through it"
    );
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
    // The *Receiving Email* page of Evolution compares the `backend_name` of each
    // extension with the protocol of a Camel provider (`strcmp`). A wrong name
    // gives no error report. The result is that JMAP is not in the provider
    // list, and the user cannot select this account type.
    let class = Class::get();
    let name = class.vfuncs().backend_name;
    assert!(!name.is_null(), "class_init left backend_name NULL");
    // SAFETY: class_init put a NUL-terminated static string there.
    assert_eq!(unsafe { CStr::from_ptr(name) }, MAIL_BACKEND_NAME);
    // The committed account also has this string as `[Collection] BackendName`.
    // Thus the provider on the page and the factory in the registry use one
    // name, not two names that agree by chance.
    assert_eq!(MAIL_BACKEND_NAME.to_str(), Ok(BACKEND_NAME));

    // The value that an unwritten slot has.
    assert!(
        parent_class().backend_name.is_null(),
        "Evolution's abstract class now names a backend of its own"
    );
}

#[test]
fn new_collection_displaces_the_inherited_one() {
    // The Evolution implementation answers NULL ("this backend has no
    // collection"). That is correct for POP3, but not for a groupware account.
    // Without a collection source, the committed account is a mail account with
    // no address books and no calendars, and no message tells the user.
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

    // `[Collection]` makes the file an account, not a lone mail source. Its
    // `BackendName` is the key that the registry uses to find the collection
    // factory.
    assert!(collection.has_extension(E_SOURCE_EXTENSION_COLLECTION));
    assert_eq!(collection.backend_name().as_deref(), Some(BACKEND_NAME));
}

#[test]
fn the_collection_offered_is_the_account_the_dialog_starts_from() {
    let collection = Class::get().new_collection();

    // The three boxes start ticked and the connection starts secure, as
    // `defaults::from_identity` writes them. If all parts were off, the user
    // must repair the account before it is the account they want.
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

    // Evolution builds this source in `constructed`, before the user types
    // anything. Thus there is no address to get a server from. A guess here
    // makes a well-known probe to a domain that nobody named. The reader of the
    // registry also finds no connectable server, and `complete::check` agrees.
    // `None`, not `Some("")`: the EDS setters strip their input and store
    // nothing for a string that is then empty. Thus the empty identity reads
    // back as absent. That is the important result, because the registry reads
    // the same from a keyfile with no `Identity=` line.
    assert_eq!(collection.identity(), None);
    assert_eq!(collection.user(), None);
    assert_eq!(collection.server(), Err(SourceError::MissingHost));
}

#[test]
fn setup_defaults_displaces_the_inherited_one() {
    // The Evolution implementation is an empty function. This was read from the
    // installed library: the slot holds a two-instruction stub that returns
    // immediately. That is correct for a provider that knows nothing about an
    // account before the user types a server. If inherited, no error occurs.
    // Instead, the server settings page opens blank, although the assistant
    // knows the address. This crate's `check_complete` then greys out *Next*,
    // and the screen does not show which field is missing.
    let class = Class::get();
    let ours = class
        .vfuncs()
        .setup_defaults
        .expect("class_init installed no setup_defaults");
    let inherited = parent_class()
        .setup_defaults
        .expect("Evolution installs its own setup_defaults");
    assert!(
        !std::ptr::fn_addr_eq(ours, inherited),
        "setup_defaults is still Evolution's, which fills nothing in"
    );
}

#[test]
fn the_defaults_are_the_account_the_address_names() {
    // This is all that a JMAP setup knows before it connects. The address is
    // the identity of the account, and RFC 8620 autodiscovery asks at its
    // domain. The address is also the offered login name. The test reads back
    // with the reader of the registry. The default is useful only if the
    // collection backend gets the origin that the address names.
    let collection = Class::get().new_collection();
    assert!(collection.setup("vera@example.com"));

    assert_eq!(collection.identity().as_deref(), Some("vera@example.com"));
    assert_eq!(collection.user().as_deref(), Some("vera@example.com"));
    assert_eq!(
        collection.server(),
        Ok(ConnectTarget::Domain("example.com".into()))
    );
    // The setup can commit this account, which is the purpose of the default.
    // In the usual case, the page opens with *Next* already sensitive, not
    // greyed out after the user gave the address.
    assert!(collection.complete());
}

#[test]
fn the_defaults_leave_the_answers_the_address_does_not_give() {
    // `new_collection` already wrote the three parts and the TLS switch, and
    // the address tells nothing about them. Thus `setup` changes only the
    // fields from the address, and does not replace the account. Assume a user
    // unticks *Calendars*, then goes back to correct the address. The box must
    // stay unticked.
    let account = Account {
        // The address is still the initial one, so the defaults apply. The user
        // changed only a box that is not related to the address.
        identity: String::new(),
        parts: Parts {
            mail: true,
            contacts: true,
            calendars: false,
        },
        ..finished()
    };
    let collection = Class::get().new_collection().edited(&account);
    assert!(collection.setup("vera@example.com"));

    assert_eq!(
        collection.parts(),
        Parts {
            mail: true,
            contacts: true,
            calendars: false,
        }
    );
    assert!(collection.secure());
}

#[test]
fn an_address_that_is_not_one_yet_offers_no_server() {
    // The user can leave the identity page with any text in it. The result is
    // the account from `from_identity` (the address as typed, no server), and a
    // refusal that names the address, because the user must correct that field.
    let collection = Class::get().new_collection();
    assert!(collection.setup("vera"));

    assert_eq!(collection.identity().as_deref(), Some("vera"));
    assert_eq!(collection.server(), Err(SourceError::MissingHost));
    assert!(!collection.complete());
    assert_eq!(
        collection.refusal(),
        Err(Incomplete::InvalidIdentity("vera".to_owned()))
    );
}

#[test]
fn a_second_visit_to_the_page_keeps_the_server_the_user_typed() {
    // Evolution prepares the receiving page each time the assistant shows it.
    // Thus this vfunc runs again each time the user goes back and forward. The
    // defaults come from the *address*. If the address did not change, the
    // defaults are already written. Thus a server that the user corrected by
    // hand stays, and does not revert after a look at the previous page.
    let collection = Class::get()
        .new_collection()
        .with_server("jmap.example.com");
    assert!(collection.setup("vera@example.com"));
    assert_eq!(
        collection.server(),
        Ok(ConnectTarget::Domain("example.com".into())),
        "the first pass has no earlier answer to keep: the address is new"
    );

    let collection = collection.with_server("jmap.example.com");
    assert!(
        !collection.setup("vera@example.com"),
        "the same address a second time is nothing further to say"
    );
    assert_eq!(
        collection.server(),
        Ok(ConnectTarget::Domain("jmap.example.com".into()))
    );
}

#[test]
fn a_corrected_address_is_derived_from_again() {
    // The opposite case. The server that the user typed was for the old
    // address. After the address changes, the server of the account is not at
    // the old domain. If the old server stays, the setup goes to a domain that
    // the user no longer names.
    let collection = Class::get().new_collection();
    assert!(collection.setup("vera@example.com"));
    let collection = collection.with_server("jmap.example.com");

    assert!(collection.setup("vera@example.net"));
    assert_eq!(collection.identity().as_deref(), Some("vera@example.net"));
    assert_eq!(collection.user().as_deref(), Some("vera@example.net"));
    assert_eq!(
        collection.server(),
        Ok(ConnectTarget::Domain("example.net".into()))
    );
}

#[test]
fn an_existing_standalone_account_keeps_its_server_on_a_no_op_save() {
    // Item 144: the editor's backend for a standalone account (no collection in
    // the registry) always gets a fresh `collection` from `new_collection`, with
    // no server. Only `source`, the scratch copy of the real mail account, has
    // the real `[Authentication]`/`[Security]`. `check_complete`'s narrow must
    // seed `collection` from `source`, not from the address, or OK with no
    // change overwrites the real server with an autodiscovery guess.
    let collection = Class::get().new_collection();
    let source = MailSource::configured(&Connection {
        host: "127.0.0.1".to_owned(),
        port: Some(8090),
        user: None,
        auth_method: None,
        secure: false,
    });

    // SAFETY: both are live sources from this test.
    assert!(unsafe { seed_existing(collection.0, source.0, "vera@example.com") });

    assert_eq!(collection.identity().as_deref(), Some("vera@example.com"));
    assert_eq!(
        collection.server(),
        Ok(ConnectTarget::Origin("http://127.0.0.1:8090".into())),
        "the real server from `source`, not example.com's autodiscovery guess"
    );

    // What `check_complete` actually calls: `seed_existing` first, `setup` only
    // if it did nothing. On a second narrow with the same address (a keystroke
    // elsewhere on the page, or OK itself), the real server must survive.
    if !unsafe { seed_existing(collection.0, source.0, "vera@example.com") } {
        unsafe { setup(collection.0, "vera@example.com") };
    }
    assert_eq!(
        collection.server(),
        Ok(ConnectTarget::Origin("http://127.0.0.1:8090".into()))
    );
}

#[test]
fn a_new_account_with_no_configured_source_still_gets_the_address_default() {
    // The opposite case, so the fix does not regress the assistant: a blank
    // scratch mail source (no `[Authentication]` yet) means there is nothing
    // real to copy, and `seed_existing` must defer to `setup`'s address-derived
    // default, exactly as before this fix.
    let collection = Class::get().new_collection();
    let source = MailSource::blank();

    // SAFETY: both are live sources from this test.
    assert!(!unsafe { seed_existing(collection.0, source.0, "vera@example.com") });
    assert!(unsafe { setup(collection.0, "vera@example.com") });
    assert_eq!(
        collection.server(),
        Ok(ConnectTarget::Domain("example.com".into()))
    );
}

#[test]
fn a_backend_with_no_collection_source_has_no_defaults_to_write() {
    // The NULL from a failed `new_collection`, through the same vfuncs. It is
    // silent: the failure was logged where it occurred, and this runs on each
    // visit to the page.
    // SAFETY: this function documents that it accepts NULL.
    assert!(!unsafe { setup(ptr::null_mut(), "vera@example.com") });
}

#[test]
fn seed_existing_accepts_a_missing_collection_or_source() {
    let collection = Class::get().new_collection();
    let source = MailSource::configured(&Connection {
        host: "127.0.0.1".to_owned(),
        port: Some(8090),
        user: None,
        auth_method: None,
        secure: false,
    });
    // SAFETY: this function documents that it accepts NULL for either source.
    assert!(!unsafe { seed_existing(ptr::null_mut(), source.0, "vera@example.com") });
    // SAFETY: as above.
    assert!(!unsafe { seed_existing(collection.0, ptr::null_mut(), "vera@example.com") });
}

#[test]
fn check_complete_displaces_the_inherited_one() {
    // The Evolution implementation answers TRUE: the account is finished when
    // the user selects a provider. That is correct for a POP3 account with no
    // fields, but not for an account that needs an address and a server. If
    // inherited, no visible feature is missing. Instead, *Next* is sensitive
    // for an empty account, and the account is committed with no host and then
    // fails in the registry.
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
    // The account from `new_collection`, with no identity and no server. The
    // assistant opens with it, and it must not be committed. The refusal names
    // the identity, not the server, because the user is on the identity page.
    let collection = Class::get().new_collection();
    assert!(!collection.complete());
    assert_eq!(collection.refusal(), Err(Incomplete::MissingIdentity));
}

#[test]
fn a_finished_account_is_one_the_setup_may_commit() {
    let collection = Class::get().new_collection().edited(&finished());
    assert_eq!(collection.refusal(), Ok(()));
    assert!(collection.complete());
    // The registry reader gets the server that the check accepted, not a
    // different reading of the source.
    assert_eq!(
        collection.server(),
        Ok(ConnectTarget::Origin(
            "https://jmap.example.com:8443".into()
        ))
    );
}

#[test]
fn oauth2_picked_by_hand_with_no_registered_client_is_refused() {
    // The Authentication combo (`AUTH_CHOICES` in `backend.rs`) writes only
    // `[Authentication] Method` = `OAUTH2_AUTH_METHOD`. Only "Look Up Account
    // Details" (`config_lookup.rs`) runs `discover_and_register`. An account
    // with OAuth 2.0 but no `[JMAP OAuth2]` client cannot get a token. Thus
    // *Next* must not accept it.
    let mut account = finished();
    account.connection.auth_method = Some(OAUTH2_AUTH_METHOD.to_str().unwrap().to_owned());
    let collection = Class::get().new_collection().edited(&account);
    assert!(!collection.complete());
    assert_eq!(collection.refusal(), Err(Incomplete::OAuth2NotRegistered));
}

#[test]
fn oauth2_with_a_registered_client_is_accepted() {
    // The opposite case. After `discover_and_register` (or a future manual
    // entry) writes a client id, the same account is complete. The check
    // requires a client, but not a specific origin for it.
    let mut account = finished();
    account.connection.auth_method = Some(OAUTH2_AUTH_METHOD.to_str().unwrap().to_owned());
    let collection = Class::get().new_collection().edited(&account);
    // SAFETY: `collection.0` is the live source `edited` just wrote to.
    unsafe {
        oauth2::apply(
            collection.0,
            &oauth2::Config {
                client_id: Some("registered-client".to_owned()),
                ..oauth2::Config::default()
            },
        );
    }
    assert_eq!(collection.refusal(), Ok(()));
    assert!(collection.complete());
}

#[test]
fn plaintext_to_a_server_that_is_not_this_machine_is_refused() {
    // The TLS rule of the project (M3), through the path of the vfunc, not only
    // through `check`. The entries write `[Security] Method=none` onto the
    // collection source, and `check_complete` examines that source.
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
fn commit_changes_displaces_the_inherited_one() {
    // The Evolution implementation does nothing. That is correct for all
    // providers where the user types the server into this page. Those entries
    // are bound through `CamelSettings` to the `[Authentication]` and
    // `[Security]` of the mail source, so the value is written before commit.
    // For JMAP, this is not true. The account holds the server one time, and the
    // mail source is a second file that must get it. If inherited, the inbox of
    // the committed account names a provider and no host.
    let class = Class::get();
    let ours = class
        .vfuncs()
        .commit_changes
        .expect("class_init installed no commit_changes");
    let inherited = parent_class()
        .commit_changes
        .expect("Evolution installs its own commit_changes");
    assert!(
        !std::ptr::fn_addr_eq(ours, inherited),
        "commit_changes is still Evolution's, which writes nothing"
    );
}

#[test]
fn a_commit_gives_the_mail_source_the_server_the_account_names() {
    let collection = Class::get().new_collection().edited(&finished());
    let source = MailSource::blank();

    // SAFETY: two live sources: the collection of the backend and its scratch
    // mail source. The vfunc gets these two.
    assert!(unsafe { commit(collection.0, source.0) });

    // Read with the reader of the registry, not field by field. The mail source
    // must give the same answer as the account, in the same words. If only the
    // host agreed and the port did not, the store connects to a different server.
    assert_eq!(
        source.server(),
        Ok(ConnectTarget::Origin(
            "https://jmap.example.com:8443".into()
        ))
    );
    assert_eq!(source.server(), collection.server());
    // The user is important for more than consistency. EDS compares the two
    // `[Authentication] Host` strings to decide if a child shares the password
    // of its collection. A mail source with its own user is a second libsecret
    // entry for one account.
    assert_eq!(source.user().as_deref(), Some("vera"));
}

#[test]
fn a_commit_writes_nothing_an_unfinished_account_could_not_say() {
    // The account from `new_collection`, with no identity and no server. This
    // case is not an error. Evolution makes one instance of this backend for
    // each page. The instance of the *Sending* page gets its own scratch
    // collection, and no widget writes to it. If its empty values went onto a
    // transport source, a missing host becomes a host of "", which reads back
    // as configured.
    let collection = Class::get().new_collection();
    let source = MailSource::blank();

    // SAFETY: two live sources, as above.
    assert!(!unsafe { commit(collection.0, source.0) });

    // Not "wrote an empty host", but "wrote nothing", groups included.
    assert!(!source.has_extension(E_SOURCE_EXTENSION_AUTHENTICATION));
    assert!(!source.has_extension(E_SOURCE_EXTENSION_SECURITY));
    assert_eq!(source.server(), Err(SourceError::MissingHost));
}

#[test]
fn plaintext_to_this_machine_reaches_the_mail_source_too() {
    // The committed account of the mock server. This is the one case where the
    // setup writes a mail source without TLS. It must be `http://`, not the
    // default of the security method.
    let mut account = finished();
    account.connection.host = "localhost".to_owned();
    account.connection.secure = false;
    let collection = Class::get().new_collection().edited(&account);
    let source = MailSource::blank();

    // SAFETY: two live sources, as above.
    assert!(unsafe { commit(collection.0, source.0) });
    assert_eq!(
        source.server(),
        Ok(ConnectTarget::Origin("http://localhost:8443".into()))
    );
}

#[test]
fn a_commit_without_both_of_its_sources_writes_nothing() {
    // NULL collection: `new_collection` failed, so there is no account to copy.
    // NULL source: Evolution did not give this backend a scratch source yet.
    // A backend is in this state before it is a candidate for a page.
    let collection = Class::get().new_collection().edited(&finished());
    let source = MailSource::blank();

    // SAFETY: NULL and a live source, one of the two documented cases.
    assert!(!unsafe { commit(ptr::null_mut(), source.0) });
    assert!(!source.has_extension(E_SOURCE_EXTENSION_AUTHENTICATION));
    // SAFETY: a live source and NULL, the other documented case.
    assert!(!unsafe { commit(collection.0, ptr::null_mut()) });
}

#[test]
fn a_backend_with_no_collection_source_commits_nothing() {
    // If `new_collection` failed, the vfunc must answer FALSE. Then *Next* is
    // greyed out, and no unreadable account is committed. It is silent, because
    // the failure was reported where it occurred. A critical for each keystroke
    // hides the first report in copies.
    // SAFETY: NULL is the one non-source value that this function documents.
    assert!(!unsafe { is_complete(ptr::null_mut()) });
}
