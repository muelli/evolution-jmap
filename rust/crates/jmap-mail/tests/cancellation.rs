// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Stop button, and the vfuncs that had never looked at it.
//!
//! Every sync vfunc in this provider is handed a `GCancellable` — it is the
//! second-to-last argument of nearly all of them — and until now every one of
//! them named it `_cancellable` and ignored it. The connection was opened with
//! a [`CancelFlag`] taken from the *authentication's* cancellable, and that
//! bridge is disconnected the moment `authenticate_sync` returns; so from the
//! first folder listing onwards, a user pressing Stop on a refresh that is
//! fetching a thousand summaries was pressing a button wired to nothing.
//!
//! `jmap-backend-core`'s `observe` is what a vfunc now holds for the length of
//! its call, and these tests are the proof that holding it is enough — that a
//! cancellable this test cancels stops a call made several layers below, by a
//! [`Client`] built before the cancellable existed.
//!
//! ## Why these calls go through the class and not through Camel's wrappers
//!
//! `camel_folder_refresh_info_sync` and its siblings check the cancellable
//! themselves, before they ever dispatch: `camel_service_connect_sync` fails an
//! already-cancelled call outright. A test that went through the wrapper would
//! therefore pass whether or not this provider observed anything at all, which
//! is the definition of a test that proves nothing. Called through the class,
//! the only thing between the cancellable and the answer is the vfunc.
//!
//! ## One cancellable, cancelled before the call
//!
//! Rather than a thread racing to cancel a fetch in flight. What is being
//! tested is that the vfunc *observes* — and `g_cancellable_connect` fires its
//! callback immediately for a cancellable that is already cancelled, which is
//! exactly the case EDS and Camel produce when a user stops an operation that
//! was still queued. A race would test the same wiring less reliably.
//!
//! [`CancelFlag`]: jmap_client::CancelFlag
//! [`Client`]: jmap_client::Client

mod common;

use std::ffi::CString;
use std::ptr;

use common::Account;
use eds_sys::{
    CAMEL_STORE_FOLDER_INFO_REFRESH, CAMEL_STORE_FOLDER_NONE, CamelFolder, CamelFolderClass,
    CamelStoreClass, camel_store_get_folder_sync,
};
use gio_sys::{
    G_IO_ERROR_CANCELLED, GCancellable, g_cancellable_cancel, g_cancellable_new, g_io_error_quark,
};
use glib_sys::{GError, g_error_free};
use gobject_sys::{g_object_unref, g_type_class_ref, g_type_class_unref};
use jmap_client::transport::{CancelFlag, CancelScope};
use jmap_client::{Client, Credentials};
use jmap_mail::connect::StoreError;
use jmap_mail::folder::folder_type;
use jmap_mail::server::ServerConfig;
use jmap_mail::service::authenticate;
use jmap_mail::store::{JmapStore, store_type};
use jmap_mail_sync::MailSync;
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::mail::role;

/// An account with one message in its inbox, and that inbox opened the way
/// Camel opens it.
struct Mailbox {
    _server: MockServer,
    account: Account,
    folder: *mut CamelFolder,
}

fn with_mail() -> Mailbox {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&account_id).unwrap();
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_email(EmailSeed::new(
            inbox,
            ("Bob", "bob@example.com"),
            "First",
            "one",
            "2026-01-01T09:00:00Z",
        ));
    }

    let client = Client::connect(server.origin(), Credentials::none()).expect("connected");
    let account = Account::open();
    account.connect(MailSync::new(client, server.account_id()));

    let path = CString::new("Inbox").expect("a path with no NUL");
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live store of ours, a NUL-terminated path alive across the
    // call, and an out-parameter that is writable and currently NULL.
    let folder = unsafe {
        camel_store_get_folder_sync(
            account.store,
            path.as_ptr(),
            CAMEL_STORE_FOLDER_NONE,
            ptr::null_mut(),
            &mut error,
        )
    };
    assert!(
        !folder.is_null() && error.is_null(),
        "the inbox would not open"
    );

    Mailbox {
        _server: server,
        account,
        folder,
    }
}

impl Drop for Mailbox {
    fn drop(&mut self) {
        // SAFETY: the one reference `camel_store_get_folder_sync` handed over.
        unsafe { g_object_unref(self.folder.cast()) };
    }
}

/// A `GCancellable` the user has already stopped.
struct Stopped(*mut GCancellable);

impl Stopped {
    fn new() -> Self {
        // SAFETY: constructing a GCancellable and cancelling it; both take
        // ownership of nothing.
        unsafe {
            let cancellable = g_cancellable_new();
            g_cancellable_cancel(cancellable);
            Self(cancellable)
        }
    }
}

impl Drop for Stopped {
    fn drop(&mut self) {
        // SAFETY: the one reference `g_cancellable_new` handed over.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// What a vfunc that refused set, freed the way Camel frees it.
struct Refusal(*mut GError);

impl Refusal {
    /// The failure is `G_IO_ERROR_CANCELLED` — GLib's own domain, which is what
    /// every layer above Camel tests for before it decides an account is
    /// broken and puts an alert in front of the user.
    fn is_the_stop_the_user_pressed(&self, what: &str) {
        assert!(!self.0.is_null(), "{what} did not report the cancellation");
        // SAFETY: a live GError this struct owns.
        unsafe {
            assert_eq!(
                (*self.0).domain,
                g_io_error_quark(),
                "{what} reported the cancellation in the wrong domain: {}",
                std::ffi::CStr::from_ptr((*self.0).message).to_string_lossy()
            );
            assert_eq!((*self.0).code, G_IO_ERROR_CANCELLED, "{what}");
        }
    }
}

impl Drop for Refusal {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the vfunc handed ownership of it over.
            unsafe { g_error_free(self.0) };
        }
    }
}

#[test]
fn a_refresh_the_user_stopped_does_not_go_to_the_server() {
    let mail = with_mail();
    let stopped = Stopped::new();
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: referencing the class runs the class_init that installs the
    // vfunc; the folder is an instance of that class, the cancellable is live,
    // and `error` is writable and currently NULL.
    let ok = unsafe {
        let class = g_type_class_ref(folder_type()).cast::<CamelFolderClass>();
        let vfunc = (*class).refresh_info_sync.expect("a refresh vfunc");
        let ok = vfunc(mail.folder, stopped.0, &mut error);
        g_type_class_unref(class.cast());
        ok
    };

    assert_eq!(ok, glib_sys::GFALSE, "the refresh claimed to have happened");
    Refusal(error).is_the_stop_the_user_pressed("refresh_info_sync");
}

#[test]
fn a_message_the_user_stopped_opening_is_not_fetched() {
    let mail = with_mail();
    let stopped = Stopped::new();
    let uid = CString::new("M1").expect("a uid with no NUL");
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: as above; `uid` is NUL-terminated and alive across the call.
    let message = unsafe {
        let class = g_type_class_ref(folder_type()).cast::<CamelFolderClass>();
        let vfunc = (*class).get_message_sync.expect("a get-message vfunc");
        let message = vfunc(mail.folder, uid.as_ptr(), stopped.0, &mut error);
        g_type_class_unref(class.cast());
        message
    };

    assert!(message.is_null(), "a cancelled open produced a message");
    Refusal(error).is_the_stop_the_user_pressed("get_message_sync");
}

/// The store side of the same thing — and the listing in particular, because
/// `CAMEL_STORE_FOLDER_INFO_REFRESH` is the flag that makes it a network call
/// and Evolution sets it every time the user asks for the folder tree.
#[test]
fn a_folder_listing_the_user_stopped_does_not_go_to_the_server() {
    let mail = with_mail();
    let stopped = Stopped::new();
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: as above; a NULL `top` asks for the whole tree, which is what
    // Camel passes for a full listing.
    let tree = unsafe {
        let class = g_type_class_ref(store_type()).cast::<CamelStoreClass>();
        let vfunc = (*class).get_folder_info_sync.expect("a listing vfunc");
        let tree = vfunc(
            mail.account.store,
            ptr::null(),
            CAMEL_STORE_FOLDER_INFO_REFRESH,
            stopped.0,
            &mut error,
        );
        g_type_class_unref(class.cast());
        tree
    };

    assert!(tree.is_null(), "a cancelled listing produced a tree");
    Refusal(error).is_the_stop_the_user_pressed("get_folder_info_sync");
}

/// A write, not a read: what the user stops here is a change to the account,
/// and the folder must not appear on the server after they stopped it.
#[test]
fn a_folder_the_user_stopped_creating_is_not_created() {
    let mail = with_mail();
    let stopped = Stopped::new();
    let name = CString::new("Later").expect("a name with no NUL");
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: as above; a NULL parent is a folder at the top level.
    let created = unsafe {
        let class = g_type_class_ref(store_type()).cast::<CamelStoreClass>();
        let vfunc = (*class).create_folder_sync.expect("a create vfunc");
        let created = vfunc(
            mail.account.store,
            ptr::null(),
            name.as_ptr(),
            stopped.0,
            &mut error,
        );
        g_type_class_unref(class.cast());
        created
    };

    assert!(created.is_null(), "a cancelled create produced a folder");
    Refusal(error).is_the_stop_the_user_pressed("create_folder_sync");
}

/// The connect itself, which is the operation that blocks longest and the one
/// place cancellation already reached — through a flag handed to the client at
/// build time. It still has to work now that the flag is gone and the scope
/// the vfunc installs is what carries it.
#[test]
fn an_authentication_the_user_stopped_does_not_reach_the_server() {
    let server = MockServer::builder().start();
    let store = JmapStore::detached();
    let config = ServerConfig {
        origin: server.origin().to_owned(),
        user: None,
    };

    let outcome = {
        let _scope = CancelScope::install(&{
            let flag = CancelFlag::new();
            flag.cancel();
            flag
        });
        authenticate(&*store, &config, Credentials::none())
    };

    assert!(
        matches!(
            outcome,
            Err(StoreError::Client(jmap_client::Error::Cancelled))
        ),
        "a cancelled authentication answered {outcome:?}"
    );
}

/// And the other half of that: the connection the store keeps must not carry
/// the cancellation of the operation that opened it. A flag can be set and
/// never unset, so a client built around one that fired would refuse every
/// operation the account performed for the rest of the session — the account
/// would look permanently broken, and reconnecting would be the only cure.
#[test]
fn a_connection_does_not_inherit_the_cancellation_of_the_call_that_opened_it() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_mailbox("Inbox", Some(role::INBOX));
    }
    let store = JmapStore::detached();
    let config = ServerConfig {
        origin: server.origin().to_owned(),
        user: None,
    };

    let flag = CancelFlag::new();
    {
        let _scope = CancelScope::install(&flag);
        authenticate(&*store, &config, Credentials::none()).expect("the authentication succeeded");
        // The user stops the operation *after* it opened the connection — the
        // race the store lives with, since Camel cancels asynchronously and
        // the vfunc has already installed what it stored.
        flag.cancel();
    }

    let listed = store
        .folders(CAMEL_STORE_FOLDER_INFO_REFRESH)
        .expect("the next operation on the account works");
    assert_eq!(listed.len(), 1, "the account's inbox was not listed");
}
