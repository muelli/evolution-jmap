// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared test utilities, fixture setup, and detached backend wrappers for
//! `jmap-backend-book` integration tests.

#![allow(dead_code)]

use eds_sys::{EBookMetaBackend, e_client_error_quark};
use glib_sys::GError;
use jmap_backend_book::backend::JmapBookBackend;
use jmap_backend_core::push::PushRefresh;
use jmap_backend_core::source::{ConnectTarget, SourceConfig};
use jmap_book_sync::BookSync;
use jmap_client::eventsource::expand_url;
use jmap_client::{Client, Credentials};
use jmap_mock::{MockServer, MockServerBuilder};
use jmap_proto::Id;
use jmap_proto::contacts::ContactCard;

/// Detached backend instance for test isolation.
pub struct Detached(pub Box<JmapBookBackend>);

impl Detached {
    pub fn new() -> Self {
        Self(JmapBookBackend::detached())
    }

    pub fn with_sync(sync: BookSync) -> Self {
        let backend = JmapBookBackend::detached();
        backend.store_connection(sync);
        Self(backend)
    }

    pub fn as_ptr(&mut self) -> *mut EBookMetaBackend {
        std::ptr::from_mut(&mut *self.0).cast()
    }
}

impl std::ops::Deref for Detached {
    type Target = JmapBookBackend;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Detached {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Mock server and `BookSync` test fixture.
pub struct Fixture {
    pub server: MockServer,
    pub account_id: Id,
    pub ours: Id,
    pub theirs: Id,
}

impl Fixture {
    pub fn start() -> Self {
        Self::start_with(MockServer::builder())
    }

    /// As [`Fixture::start`], on a server that `builder` configures.
    pub fn start_with(builder: MockServerBuilder) -> Self {
        let server = builder.start();
        let account_id = server.account_id();
        let (theirs, ours) = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            // Seed non-default book first so incorrect default resolution fails visibly.
            (
                account.seed_address_book("Shared", false),
                account.seed_address_book("Personal", true),
            )
        };
        Self {
            server,
            account_id,
            ours,
            theirs,
        }
    }

    /// Starts a mock server without a default address book.
    pub fn start_without_default() -> (MockServer, SourceConfig) {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            account.seed_address_book("Shared", false);
        }
        let config = SourceConfig {
            target: ConnectTarget::Origin(server.origin().to_owned()),
            user: None,
            resource_id: None,
            rebase_urls: false,
        };
        (server, config)
    }

    /// Starts a mock server with a primary account and a second account.
    /// Returns `(fixture, second_account_id, second_book_id)`.
    pub fn start_two_accounts() -> (Self, Id, Id) {
        let fixture = Self::start();
        let second_account_id = Id::from("second-account");
        let second_book_id = {
            let state = fixture.server.state();
            let mut state = state.lock().unwrap();
            let account = state.add_account(second_account_id.clone(), "Second Account");
            account.seed_address_book("SecondShared", true)
        };
        (fixture, second_account_id, second_book_id)
    }

    pub fn client(&self) -> Client {
        Client::connect(self.server.origin(), Credentials::none()).unwrap()
    }

    pub fn client_with_token(&self, token: &str) -> Client {
        Client::connect(self.server.origin(), Credentials::bearer(token)).unwrap()
    }

    /// Returns a [`BookSync`] over the default address book.
    pub fn sync(&self) -> BookSync {
        BookSync::new(self.client(), self.account_id.clone(), self.ours.clone())
    }

    pub fn config(&self) -> SourceConfig {
        SourceConfig {
            target: ConnectTarget::Origin(self.server.origin().to_owned()),
            user: None,
            resource_id: None,
            rebase_urls: false,
        }
    }

    /// Creates a card directly, bypassing the code under test.
    pub fn seed(&self, book: &Id, full_name: &str, email: &str) -> Id {
        self.client()
            .contact_create(
                &self.account_id,
                &ContactCard::simple(book.clone(), full_name, email),
            )
            .unwrap()
            .id
            .expect("server assigned id")
    }

    /// Creates a card in the default address book.
    pub fn seed_default(&self, full_name: &str, email: &str) -> Id {
        self.seed(&self.ours, full_name, email)
    }

    /// The uids the book holds, as the server sees them.
    pub fn uids(&self) -> Vec<String> {
        let (_, contacts) = self.sync().list_existing().unwrap();
        let mut uids: Vec<String> = contacts.into_iter().map(|info| info.uid).collect();
        uids.sort();
        uids
    }

    pub fn push_subscription(&self) -> PushRefresh {
        let url = expand_url(
            &format!("{}/eventsource", self.server.origin()),
            &["ContactCard"],
            false,
            0,
        );
        PushRefresh::start(
            url,
            Vec::new(),
            self.account_id.clone(),
            vec!["ContactCard".to_owned()],
            |_types: &[String]| {},
        )
    }

    pub fn detached(&self, token: &str) -> Detached {
        Detached::with_sync(BookSync::new(
            self.client_with_token(token),
            self.account_id.clone(),
            self.ours.clone(),
        ))
    }

    /// Returns the number of contacts currently held on the server.
    pub fn contacts_on_the_server(&self) -> usize {
        self.sync().list_existing().expect("listed").1.len()
    }
}

/// Asserts that a failed call set `E_CLIENT_ERROR_REPOSITORY_OFFLINE` and frees the error.
pub unsafe fn assert_offline(error: &mut *mut GError) {
    unsafe {
        assert!(!error.is_null(), "the call failed without setting an error");
        assert!(!(*error).is_null(), "the error is null");
        assert_eq!((**error).domain, e_client_error_quark(), "error domain");
        assert_eq!(
            (**error).code,
            eds_sys::E_CLIENT_ERROR_REPOSITORY_OFFLINE as i32,
            "error code"
        );
        glib_sys::g_error_free(*error);
        *error = std::ptr::null_mut();
    }
}
