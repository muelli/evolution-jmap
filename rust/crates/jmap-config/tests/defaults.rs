// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The account that a setup starts from: what the server settings page shows
// when the user first opens it, from only the address on the previous page.
//
// The last two tests are the joins. They are the reason to have a function
// for this, not widget defaults. If the `check` of the setup refuses the first
// offer, *Next* is disabled on a page that the user did not change yet. If the
// registry reads back a different server, the well-known probe goes to a
// location that the address did not name. These tests use the same two
// readers as the other tests of the crate.

use std::ffi::CString;
use std::ptr;

use eds_sys::{ESource, e_source_new_with_uid};
use gobject_sys::g_object_unref;
use jmap_backend_collection::collection_source::server_of;
use jmap_backend_core::source::{ConnectTarget, SourceError};
use jmap_collection_sync::Parts;
use jmap_config::account::{Account, apply};
use jmap_config::complete::{Incomplete, check};
use jmap_config::defaults::from_identity;

/// An `ESource` in the state that a setup commits, as in `tests/account.rs`.
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

    /// The server, as the collection backend reads it back.
    fn server(&self) -> Result<ConnectTarget, SourceError> {
        // SAFETY: a live source.
        unsafe { server_of(self.0) }.map(|server| server.target)
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: this owns the reference that `e_source_new_with_uid` returned.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

#[test]
fn the_server_offered_is_the_address_domain() {
    // RFC 8620 §2.2: a client that knows only the address looks at its domain,
    // so this default is not a guess.
    assert_eq!(
        from_identity("vera@example.com").connection.host,
        "example.com"
    );
}

#[test]
fn the_domain_is_what_follows_the_last_at_sign() {
    // Not the first `@`: an address has exactly one. In a string with two, only
    // the part after the last can be a domain. `check` refuses this identity
    // in all cases, so this does not change what is committed. It only decides
    // what the entry shows while the user corrects the address.
    assert_eq!(
        from_identity("a@b@example.com").connection.host,
        "example.com"
    );
}

#[test]
fn the_login_name_offered_is_the_whole_address() {
    // The only place where this crate derives the login from the address, as an
    // offer only: `apply` still writes the value of the entry at commit time.
    let account = from_identity("vera@example.com");
    assert_eq!(account.identity, "vera@example.com");
    assert_eq!(account.connection.user.as_deref(), Some("vera@example.com"));
}

#[test]
fn what_the_user_typed_is_offered_back_unchanged() {
    // Unchanged, as everywhere in this crate. A default in lower case would
    // silently disagree with the user about their own address. DNS ignores the
    // case of the host.
    let account = from_identity("Vera@Example.COM");
    assert_eq!(account.identity, "Vera@Example.COM");
    assert_eq!(account.connection.host, "Example.COM");
    assert_eq!(account.connection.user.as_deref(), Some("Vera@Example.COM"));
}

#[test]
fn the_default_connection_is_a_secure_one_on_the_schemes_own_port() {
    let connection = from_identity("vera@example.com").connection;
    // TLS is the project rule (M3), so it is also the initial state of the
    // dialog. With a plaintext default, the user types a password into an
    // account that nothing refused yet.
    assert!(connection.secure);
    // No person named a port, and this project must not name 443. `origin`
    // omits an unnamed port, so the default of the scheme applies.
    assert_eq!(connection.port, None);
    // `ESourceAuthentication:method` reads back as "none", which in EDS means
    // "ask for the password in the usual way". A different method here is a
    // guess about the server authentication before the first connection.
    assert_eq!(connection.auth_method, None);
}

#[test]
fn a_new_account_offers_all_three_parts() {
    // A JMAP account is one account for mail, contacts and calendars. The
    // discovery of the collection backend finds which of them the server has.
    // The three boxes start selected. Only the user clears one.
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
    // The empty host means "no answer yet", not a bad answer. `check` reports
    // the address that the user types, and the server entry stays empty, not
    // half filled.
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
    // The same translation that `complete` makes for the host, in the opposite
    // direction. An entry that the user did not fill is nothing, and here
    // nothing is `None`. An empty `user` is a login name of "", which an
    // unchanged dialog does not show.
    assert_eq!(from_identity("").connection.user, None);
    // All other values are offered as typed, also if not an address: it is the
    // content of the entry, and the user is still in it.
    assert_eq!(
        from_identity("vera").connection.user.as_deref(),
        Some("vera")
    );
}

#[test]
fn the_account_a_setup_starts_from_is_one_it_would_commit() {
    // The first join: if the domain of the address is its JMAP server, the user
    // changes nothing on the server settings page. This default is for that
    // case. Otherwise the assistant opens on a page with a disabled *Next* and
    // nothing to correct.
    assert_eq!(check(&from_identity("vera@example.com")), Ok(()));
}

#[test]
fn the_default_and_the_registry_agree_about_the_server() {
    // The second join, as `tests/complete.rs` does for the check. The registry
    // gives a client the domain of the address, secure and with no port: the
    // RFC 8620 §2.2 entry point. `_jmap._tcp` SRV autodiscovery can apply to it
    // before the bare-domain fallback.
    let source = TestSource::written(&from_identity("vera@example.com"));
    assert_eq!(
        source.server(),
        Ok(ConnectTarget::Domain("example.com".into()))
    );
}
