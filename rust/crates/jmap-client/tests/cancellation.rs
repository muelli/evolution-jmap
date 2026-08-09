// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What stops a call that is already under way, and how the caller says so.
//!
//! A [`Client`] is built once and used for every operation an account performs,
//! but cancellation is not a property of an account — it is a property of the
//! *operation*, and it arrives from a caller who has no client to hand. Camel
//! and EDS both spell it the same way: a `GCancellable` passed into one sync
//! vfunc, meaning "stop the thing this call is doing". A flag handed to
//! [`ClientBuilder::cancel_flag`] cannot express that, because by the time the
//! second operation runs the first one's cancellable is gone.
//!
//! So a client observes two things, and these tests pin which one wins:
//! the flag installed for the length of one operation on the thread running it
//! ([`CancelScope`]), and — when no operation installed one — the flag the
//! client was built with.
//!
//! [`ClientBuilder::cancel_flag`]: jmap_client::ClientBuilder::cancel_flag

use std::sync::mpsc;

use jmap_client::transport::{CancelFlag, CancelScope, observed};
use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use serde_json::json;

fn cancelled() -> CancelFlag {
    let flag = CancelFlag::new();
    flag.cancel();
    flag
}

/// The point of the whole mechanism: a caller that holds no client at all can
/// stop the call this thread is about to make.
#[test]
fn a_cancelled_scope_stops_the_call_this_thread_was_about_to_make() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");

    let scope = CancelScope::install(&cancelled());
    let outcome = client.echo(json!({"stop": true}));
    drop(scope);

    assert!(
        matches!(outcome, Err(Error::Cancelled)),
        "a call under a cancelled scope answered {outcome:?}"
    );
}

/// A scope is the operation's, not the account's: the next operation on the
/// same client is unaffected by the one the user stopped.
#[test]
fn the_scope_lasts_exactly_as_long_as_the_operation_holding_it() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");

    {
        let _scope = CancelScope::install(&cancelled());
        assert!(matches!(
            client.echo(json!({"n": 1})),
            Err(Error::Cancelled)
        ));
    }

    assert_eq!(
        client
            .echo(json!({"n": 1}))
            .expect("the next operation runs"),
        json!({"n": 1})
    );
}

/// A live flag is not a cancelled one — installing a scope is not itself a
/// refusal.
#[test]
fn a_scope_that_was_never_cancelled_stops_nothing() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");

    let _scope = CancelScope::install(&CancelFlag::new());
    assert_eq!(
        client.echo(json!({"on": true})).expect("not cancelled"),
        json!({"on": true})
    );
}

/// The precedence, and the reason for it: a client-wide flag that has latched
/// — cancelled once, and with no way to unset it — would otherwise veto every
/// operation the account ever performs again, including the ones the user is
/// waiting on now. The operation's own cancellable is the more specific
/// statement and wins.
#[test]
fn an_operation_that_was_not_cancelled_runs_under_a_client_flag_that_latched() {
    let server = MockServer::builder().start();
    let account = CancelFlag::new();
    let client = Client::builder()
        .cancel_flag(account.clone())
        .connect(server.origin(), Credentials::none())
        .expect("connected");

    account.cancel();
    assert!(
        matches!(client.echo(json!({"n": 1})), Err(Error::Cancelled)),
        "with no scope, the flag the client was built with is what answers"
    );

    let _scope = CancelScope::install(&CancelFlag::new());
    assert_eq!(
        client
            .echo(json!({"n": 1}))
            .expect("the operation was not cancelled"),
        json!({"n": 1})
    );
}

/// The client-wide flag is still observed where nothing more specific was
/// said — the address book and calendar backends hand one to their client and
/// install no scope yet.
#[test]
fn a_client_with_no_scope_observes_the_flag_it_was_built_with() {
    let server = MockServer::builder().start();
    let account = CancelFlag::new();
    let client = Client::builder()
        .cancel_flag(account.clone())
        .connect(server.origin(), Credentials::none())
        .expect("connected");

    assert_eq!(
        client.echo(json!({"n": 1})).expect("not cancelled yet"),
        json!({"n": 1})
    );
    account.cancel();
    assert!(matches!(
        client.echo(json!({"n": 1})),
        Err(Error::Cancelled)
    ));
}

/// Vfuncs nest — a folder operation calls into its store — so an inner scope
/// must give the outer one back rather than leave the thread observing
/// nothing.
#[test]
fn an_inner_scope_gives_the_outer_one_back() {
    let outer = CancelFlag::new();
    let _outer_scope = CancelScope::install(&outer);
    {
        let _inner_scope = CancelScope::install(&CancelFlag::new());
        outer.cancel();
        assert!(
            !observed().expect("a scope is installed").is_cancelled(),
            "the inner operation observed the outer operation's cancellation"
        );
    }
    assert!(
        observed().expect("the outer scope is back").is_cancelled(),
        "the outer operation's own cancellation was lost"
    );
}

/// Camel drives one store from several threads at once. A scope is what the
/// thread running the operation observes, and says nothing about any other.
#[test]
fn a_scope_belongs_to_the_thread_that_installed_it() {
    let _scope = CancelScope::install(&cancelled());

    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        sender
            .send(observed().is_some())
            .expect("the answer was sent");
    })
    .join()
    .expect("the other thread finished");

    assert!(
        !receiver.recv().expect("an answer from the other thread"),
        "another thread observed this thread's scope"
    );
}
