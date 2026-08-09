// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The account a setup starts from: what the server settings page already says
// when the user first reaches it, given only the address they typed on the page
// before.
//
// The last two tests are the joins, and they are why this is worth having as
// its own function rather than as widget defaults. A first guess that the
// setup's own `check` would refuse is a *Next* button greyed out on a page the
// user has not touched yet; a first guess whose server the registry reads back
// as some other server is a well-known probe aimed somewhere the address never
// named. Both are tested here against the same two readers the rest of the
// crate is tested against.

use std::ffi::CString;
use std::ptr;

use eds_sys::{ESource, e_source_new_with_uid};
use gobject_sys::g_object_unref;
use jmap_backend_collection::collection_source::server_of;
use jmap_backend_core::source::SourceError;
use jmap_collection_sync::Parts;
use jmap_config::account::{Account, apply};
use jmap_config::complete::{Incomplete, check};
use jmap_config::defaults::from_identity;

/// An `ESource` in the state a setup commits into, as in `tests/account.rs`.
struct TestSource(*mut ESource);

impl TestSource {
    fn written(account: &Account) -> Self {
        let uid = CString::new("jmap-account").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        // SAFETY: a live source.
        unsafe { apply(source, account) };
        Self(source)
    }

    /// The server as the collection backend reads it back.
    fn server(&self) -> Result<String, SourceError> {
        // SAFETY: a live source.
        unsafe { server_of(self.0) }.map(|server| server.origin)
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: this owns the reference `e_source_new_with_uid` returned.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

#[test]
fn the_server_offered_is_the_address_domain() {
    // RFC 8620 §2.2: the address's domain is where a client that knows nothing
    // else looks, so it is the one guess that is not a guess.
    assert_eq!(
        from_identity("vera@example.com").connection.host,
        "example.com"
    );
}

#[test]
fn the_domain_is_what_follows_the_last_at_sign() {
    // Not the first: an address has exactly one, and if a string has two the
    // part that could be a domain is the last one. `check` refuses the identity
    // either way, so this decides nothing about what gets committed — it
    // decides what the entry says while the user is still fixing the address.
    assert_eq!(
        from_identity("a@b@example.com").connection.host,
        "example.com"
    );
}

#[test]
fn the_login_name_offered_is_the_whole_address() {
    // The one place this crate does derive the login from the address, and only
    // as an offer: `apply` still writes whatever the entry says at commit time.
    let account = from_identity("vera@example.com");
    assert_eq!(account.identity, "vera@example.com");
    assert_eq!(account.connection.user.as_deref(), Some("vera@example.com"));
}

#[test]
fn what_the_user_typed_is_offered_back_unchanged() {
    // Verbatim, as everywhere in this crate: a default that lower-cased the
    // address would be a setup silently disagreeing with the user about their
    // own address, and DNS does not care about the host either way.
    let account = from_identity("Vera@Example.COM");
    assert_eq!(account.identity, "Vera@Example.COM");
    assert_eq!(account.connection.host, "Example.COM");
    assert_eq!(account.connection.user.as_deref(), Some("Vera@Example.COM"));
}

#[test]
fn the_default_connection_is_a_secure_one_on_the_schemes_own_port() {
    let connection = from_identity("vera@example.com").connection;
    // TLS is the project's rule (M3), so it is also the state the dialog opens
    // in: a default of plaintext would be a password typed into an account
    // nothing had yet refused.
    assert!(connection.secure);
    // Nobody has named a port, and 443 is not this project's to name either —
    // `origin` leaves an unnamed port out so the scheme's default applies.
    assert_eq!(connection.port, None);
    // `ESourceAuthentication:method` reads back as "none", which is EDS's own
    // "ask for the password the ordinary way". A setup that guessed a method
    // here would be guessing at the server's auth before it has met it.
    assert_eq!(connection.auth_method, None);
}

#[test]
fn a_new_account_offers_all_three_parts() {
    // A JMAP account is one account for mail, contacts and calendars, and which
    // of them the server actually has is the collection backend's discovery to
    // make. The three boxes start ticked, and turning one off is an answer the
    // user gives.
    assert_eq!(
        from_identity("vera@example.com").parts,
        Parts {
            mail: true,
            contacts: true,
            calendars: true,
        }
    );
}

#[test]
fn an_identity_that_is_not_an_address_offers_no_server() {
    // The empty host is the unanswered question, not a bad answer: `check`
    // reports the address the user is still typing, and the server entry sits
    // blank rather than filled with half of it.
    for identity in ["", "vera", "vera@", "@example.com"] {
        let account = from_identity(identity);
        assert_eq!(
            account.connection.host, "",
            "{identity:?} was made into a server"
        );
        assert!(
            matches!(
                check(&account),
                Err(Incomplete::MissingIdentity | Incomplete::InvalidIdentity(_))
            ),
            "{identity:?} was refused for something other than the address"
        );
    }
}

#[test]
fn an_empty_address_is_not_offered_as_a_login_name() {
    // The same translation `complete` makes for the host, in the other
    // direction: an entry the user has not reached yet is nothing, and `None`
    // is what nothing is here — an empty `user` would be a login name of "",
    // which is not what an untouched dialog says.
    assert_eq!(from_identity("").connection.user, None);
    // Anything else is offered as typed, address or not: it is the entry's
    // contents, and the user is still in it.
    assert_eq!(
        from_identity("vera").connection.user.as_deref(),
        Some("vera")
    );
}

#[test]
fn the_account_a_setup_starts_from_is_one_it_would_commit() {
    // The first join: nothing on the server settings page needs touching for an
    // address whose domain is its JMAP server, which is the case this default
    // exists for. If it were not so, the assistant would open on a page whose
    // *Next* is greyed out with nothing on it to fix.
    assert_eq!(check(&from_identity("vera@example.com")), Ok(()));
}

#[test]
fn the_default_and_the_registry_agree_about_the_server() {
    // The second join, the one `tests/complete.rs` makes for the check: the
    // origin the registry hands a client after this account is committed is the
    // address's own domain over TLS — the URL RFC 8620's autodiscovery would
    // have fetched the session document from.
    let source = TestSource::written(&from_identity("vera@example.com"));
    assert_eq!(source.server().as_deref(), Ok("https://example.com"));
}
