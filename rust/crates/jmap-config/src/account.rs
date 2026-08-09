// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The account itself: the collection `ESource` a setup commits.
//!
//! The inverse of [the collection backend's reader][collection_source], which reads
//! the same three groups back — `[Collection]` for which parts the account
//! offers, `[Authentication]` for where its server is and whom it logs in as,
//! `[Security]` for whether that is over TLS. Everything M6 does with an
//! account, it does with what this wrote.
//!
//! ## Here `e_source_get_extension` is wanted for what it does
//!
//! The reader goes out of its way never to call it: it *creates* the extension
//! it cannot find, and the reader is handed the user's own account file. This
//! module is the one place where creating the groups is the entire point —
//! `[Collection]` is what makes the file an account rather than a lone address
//! book, and its absence is not a broken account but a file
//! `e_source_registry_server_ref_backend_factory` never offers to a collection
//! factory at all.
//!
//! ## Every field is written, including the absent ones
//!
//! [`apply`] writes each of the eight properties every time, using NULL for a
//! string the account does not have and 0 for a port it does not name. That is
//! the difference between this and
//! [the child writer][child_source], which is handed a *fresh* child and may
//! leave a property it has nothing to say about alone.
//!
//! A setup commits onto a source that already says something — an account
//! being edited says the old server, and `EMailConfigServiceBackend` commits
//! into a scratch source that was copied from it. So "the user cleared the user
//! name" has to reach the file as an empty `User=`, and skipping the write
//! because there is nothing to write would leave the old one there: an account
//! the user made anonymous that still asks libsecret for a password under a
//! name they deleted. [`apply`] is therefore idempotent in the strong sense —
//! after it, the source says this [`Account`] and nothing of whatever it said
//! before.
//!
//! The one field that cannot be cleared is `[Authentication] Method`, and not
//! for want of trying: `ESourceAuthentication:method` has no unset state. A
//! fresh extension already reads `"none"`, and both NULL and `""` set it back
//! to that string rather than to nothing — checked against the installed EDS
//! rather than assumed. So an [`Account`] whose
//! [`auth_method`](Connection::auth_method) is `None` reads back as
//! `Some("none")`, which is the right *meaning* — it is what EDS's credentials
//! provider looks the account's password impl up by, and "none" is its answer
//! for "ask for a password the ordinary way" — but it is not the identity, and
//! `tests/account.rs` pins it so rather than pretending otherwise.
//!
//! ## `[Security]` is written as the method, and read as the boolean
//!
//! `ESourceSecurity` holds a string — "tls" or "none" — and derives
//! `ESourceSecurity:secure` from it; every JMAP backend reads the boolean. The
//! write goes through `e_source_security_set_method()` for the same reason
//! [`child_source`] does it that way: the keyfile has
//! the string in it, so the string is the spelling that has to be right, and a
//! test that reads the boolean back is what catches it when it is not.
//!
//! ## What this does not write
//!
//! - **`[Data Source] DisplayName`** — the name the user gives the account, and
//!   Evolution's own assistant page's to set. Writing it from here would rename
//!   the account on every commit, back to whatever the server settings page had
//!   composed.
//! - **`Enabled`** — a new `ESource` is enabled, and an existing one's flag is
//!   the user's answer to "show this account", not the setup's.
//! - **the mail sources** — see [`crate`]. A JMAP account's `[Mail Account]`,
//!   `[Mail Identity]` and `[Mail Transport]` are three further sources, not
//!   three more groups in this one.
//!
//! The three links above go to `jmap-backend-collection`, which is a
//! dev-dependency here — the tests link it, the library must not — so they are
//! written as paths into the generated documentation rather than as intra-doc
//! links, the same way `jmap-backend-collection` links to `jmap-backend-core`'s
//! `SourceConfig`.
//!
//! [collection_source]: ../../jmap_backend_collection/collection_source/index.html
//! [child_source]: ../../jmap_backend_collection/child_source/index.html
//! [`child_source`]: ../../jmap_backend_collection/child_source/index.html
//! [factory]: ../../jmap_backend_collection/factory/constant.FACTORY_NAME.html

use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_COLLECTION, E_SOURCE_EXTENSION_SECURITY,
    ESource, ESourceAuthentication, ESourceBackend, ESourceCollection, ESourceSecurity,
    e_source_authentication_get_type, e_source_authentication_set_host,
    e_source_authentication_set_method, e_source_authentication_set_port,
    e_source_authentication_set_user, e_source_backend_set_backend_name,
    e_source_collection_get_type, e_source_collection_set_calendar_enabled,
    e_source_collection_set_contacts_enabled, e_source_collection_set_identity,
    e_source_collection_set_mail_enabled, e_source_get_extension, e_source_security_get_type,
    e_source_security_set_method,
};
use glib_sys::{GFALSE, GTRUE, gboolean};
use jmap_backend_core::error::cstring_lossy;
use jmap_collection_sync::Parts;
use jmap_collection_sync::child_source::Connection;

/// The name the registry looks this account's collection factory up by.
///
/// `[Collection] BackendName` is not a description: the registry files each
/// collection factory under `"<factory_name>:Collection"` and looks up the key
/// built from this string, so a value that does not match the factory's name is
/// not an error anywhere — it is an account that sits in the sidebar with no
/// children and nothing in any log. `tests/account.rs` holds it against
/// [the collection factory's `FACTORY_NAME`](../../jmap_backend_collection/factory/constant.FACTORY_NAME.html),
/// which is the name the module actually registers.
pub const BACKEND_NAME: &str = "jmap";

/// An account, in the shape the setup has it before it is a source.
///
/// The fields are the reader's types rather than new ones: [`Connection`] is
/// what [the collection backend's `server_of`](../../jmap_backend_collection/collection_source/fn.server_of.html)
/// answers with
/// and what each child of the account is given, and [`Parts`] is what gates
/// every listing and every removal. One description of an account, written
/// here and read there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// `[Collection] Identity` — who the account is, which for JMAP is the
    /// email address the user typed on the identity page.
    ///
    /// Not the same field as `[Authentication] User`, and deliberately not
    /// derived from it: the address a user's mail comes from and the name they
    /// log in with are the same string often enough to be assumed equal and
    /// different often enough for the assumption to be wrong.
    pub identity: String,
    /// Where the server is, and whom the account authenticates as.
    pub connection: Connection,
    /// Which of mail, contacts and calendars the account offers — the three
    /// check boxes, and what the collection backend fans out to.
    pub parts: Parts,
}

/// Writes `account` onto `source`, which afterwards says exactly that account.
///
/// # Safety
///
/// `source` must be a valid `ESource` — the collection source the setup is
/// committing into. This call takes no reference to it and nothing here
/// outlives the call.
pub unsafe fn apply(source: *mut ESource, account: &Account) {
    // As everywhere an extension is looked up by name: `e_source_get_extension`
    // walks the registered children of `E_TYPE_SOURCE_EXTENSION`, so a type
    // nothing has referenced yet is one it cannot find — and here it would
    // create nothing and return NULL. Referencing the GType registers it.
    // SAFETY: no arguments, and the type system initialises itself.
    unsafe {
        e_source_collection_get_type();
        e_source_authentication_get_type();
        e_source_security_get_type();
    }

    // The strings outlive every call that borrows them, which is what keeps the
    // pointers below valid: a `cstring_lossy(…).as_ptr()` written inline would
    // be a pointer into a temporary dropped at the end of the statement.
    // Truncating at an interior NUL rather than refusing, as everywhere a typed
    // string crosses into C — what is kept is what the string would have meant
    // to every C caller downstream anyway, and refusing the write would mean an
    // account with no host, which fails every operation rather than one field.
    let identity = cstring_lossy(&account.identity);
    let host = cstring_lossy(&account.connection.host);
    let user = account.connection.user.as_deref().map(cstring_lossy);
    let auth_method = account.connection.auth_method.as_deref().map(cstring_lossy);
    let security_method = if account.connection.secure {
        c"tls"
    } else {
        c"none"
    };
    let backend_name = cstring_lossy(BACKEND_NAME);

    // SAFETY: a valid source by this function's contract, and header constants
    // naming extensions whose types are registered above; each extension is
    // created on demand and owned by the source, and every setter copies the
    // string it is given.
    unsafe {
        let collection: *mut ESourceCollection =
            e_source_get_extension(source, E_SOURCE_EXTENSION_COLLECTION.as_ptr()).cast();
        // `ESourceCollection` derives from `ESourceBackend`, which is where the
        // name the registry keys the factory off lives.
        e_source_backend_set_backend_name(
            collection.cast::<ESourceBackend>(),
            backend_name.as_ptr(),
        );
        e_source_collection_set_identity(collection, identity.as_ptr());
        e_source_collection_set_mail_enabled(collection, gboolean(account.parts.mail));
        e_source_collection_set_contacts_enabled(collection, gboolean(account.parts.contacts));
        e_source_collection_set_calendar_enabled(collection, gboolean(account.parts.calendars));

        let auth: *mut ESourceAuthentication =
            e_source_get_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast();
        e_source_authentication_set_host(auth, host.as_ptr());
        // NULL rather than a skipped write, for both of these: see the module
        // comment on committing onto an account that already says something.
        // EDS's string setters take NULL and mean it — the getter answers NULL
        // afterwards, which is the same "no user" the reader gets from an
        // account that never had one.
        e_source_authentication_set_user(auth, as_ptr(&user));
        e_source_authentication_set_method(auth, as_ptr(&auth_method));
        // Zero is how `[Authentication] Port` spells "not set", and it is what
        // the reader turns back into `None`.
        e_source_authentication_set_port(auth, account.connection.port.unwrap_or(0));

        let security: *mut ESourceSecurity =
            e_source_get_extension(source, E_SOURCE_EXTENSION_SECURITY.as_ptr()).cast();
        e_source_security_set_method(security, security_method.as_ptr());
    }
}

/// The string a `CString` holds, or NULL for the absence of one.
fn as_ptr(value: &Option<std::ffi::CString>) -> *const std::ffi::c_char {
    value.as_ref().map_or(ptr::null(), |value| value.as_ptr())
}

/// A Rust `bool` as the tri-state C one EDS's setters take.
fn gboolean(value: bool) -> gboolean {
    if value { GTRUE } else { GFALSE }
}
