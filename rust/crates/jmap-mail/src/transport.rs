// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelJmapTransport`: the service that sends the mail of an account.
//!
//! Camel divides an account into two separate objects with no pointer between
//! them. The [`crate::store`] holds the mail (folders, summaries, message
//! bodies), and a `CamelTransport` sends the mail.
//!
//! Evolution constructs each object from its own `ESource`, with its own
//! settings object. The store connects when the user opens a folder, the
//! transport when the user clicks Send.
//!
//! In JMAP, both halves are one session against one server. But a transport
//! has no supported path to the store of its account.
//!
//! `camel_session_get_service` needs a uid that this service does not have,
//! and Evolution pairs the two services in `EMailAccountStore`, above Camel.
//!
//! Thus the transport opens its own connection: one more HTTP client to the
//! same server, made once per send session, not once per message.
//!
//! The type holds only one slot: the connection, from `connect_sync` to
//! `disconnect_sync`. It holds no folder listing, because a transport lists
//! nothing.
//!
//! A send needs two mailboxes: the mailbox that stages the message and the
//! mailbox that files it after the send. The transport reads them over the
//! connection at send time and does not keep them.
//!
//! ## What the transport shares with the store
//!
//! The settings class is [`crate::settings`], the same as for the store,
//! because it is the same account. An inherited `CamelSettings` has no host,
//! no port and no user for `e_source_camel_configure_service` to write.
//!
//! The `CamelService` vfuncs are the same four from [`crate::service`],
//! installed for this type. Connecting is the same operation on each service.
//! Only the object that receives the connection is different: a
//! [`Connected`], which this type implements below.
//!
//! The connection is not shared. Each service has its own client, and the
//! disconnect of one service does not remove the client of the other.
//!
//! ## What it does
//!
//! It implements `send_to_sync`, the reason this type exists, with
//! [`crate::send`]. This module handles the GObject and the connection;
//! [`crate::send`] handles the arguments and answers of Camel.
//! [`JmapTransport::send_message`] below joins the two: it does the
//! account-side operations of a send under one hold of the connection.
//!
//! [`MailSync::identity_for`]: jmap_mail_sync::MailSync::identity_for
//! [`MailSync::outgoing_mailboxes`]: jmap_mail_sync::MailSync::outgoing_mailboxes
//! [`MailSync::send_message`]: jmap_mail_sync::MailSync::send_message

use std::ffi::CStr;
use std::ptr;
use std::sync::RwLock;

use eds_sys::{
    CamelService, CamelServiceClass, CamelTransport, CamelTransportClass, camel_service_get_type,
    camel_service_ref_session, camel_service_ref_settings, camel_transport_get_type,
};
use glib_sys::GType;
use jmap_backend_core::instance::Slot;
#[cfg(feature = "testing")]
use jmap_backend_core::instance::zeroed_box;
use jmap_backend_core::marshal::{checked_borrow_ptr, dispatched_borrow};
use jmap_backend_core::owned::Owned;
use jmap_backend_core::retry::retry_once_after;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_client::Credentials;
use jmap_mail_sync::{MailSync, SyncError};

use jmap_mail_sync::Outgoing;
use jmap_proto::Id;
use jmap_proto::mail::Envelope;

use crate::connect::StoreError;
use crate::oauth2;
use crate::service::Connected;
use crate::settings::settings_type;
use crate::store::{read, write};

/// The result of a sent message.
pub struct Sent {
    /// The id of the message in the account now. The server makes this id for
    /// the import, and the sent copy uses it as its uid.
    pub uid: Id,
    /// Whether that copy is in the mailbox for sent mail: Camel's
    /// `out_sent_message_saved`. If `false`, the caller saves its own copy.
    ///
    /// It is [`OutgoingMailboxes::saves_sent_copy`] unchanged. It does not mean
    /// "the server filed the message in a mailbox": see there.
    ///
    /// [`OutgoingMailboxes::saves_sent_copy`]: jmap_mail_sync::OutgoingMailboxes::saves_sent_copy
    pub saved: bool,
}

/// The instance struct. `#[repr(C)]` with the parent instance struct first
/// lets a `*mut JmapTransport` be the `CamelTransport *` of each Camel function.
#[repr(C)]
pub struct JmapTransport {
    parent: CamelTransport,
    /// The connection, from `connect_sync` to `disconnect_sync`.
    ///
    /// It is an `RwLock` for the same reason as in the store: Camel sends from
    /// its own thread, and the outbox gives a transport messages in sequence.
    ///
    /// Only connect and disconnect replace the value and need exclusive access.
    /// A send takes the read lock, so it does not wait for the upload of another send.
    connection: Slot<RwLock<Option<MailSync>>>,
}

impl JmapTransport {
    /// Installs `sync` as the live connection and replaces the previous one.
    ///
    /// It replaces and does not refuse, for the reason in
    /// [`JmapStore::store_connection`]: Camel reconnects a service that it
    /// thinks is lost, and the replaced connection is that lost one.
    ///
    /// Nothing else is dropped with it. Unlike the store, this service holds
    /// no listing that only the server of the old connection explains.
    ///
    /// [`JmapStore::store_connection`]: crate::store::JmapStore::store_connection
    pub fn install_connection(&self, sync: MailSync) {
        if let Some(connection) = self.connection() {
            tracing::debug!("installing connection in transport");
            *write(connection) = Some(sync);
        }
    }

    /// Drops the connection and tells whether one was there.
    ///
    /// No connection is a normal result, not a failure: Camel disconnects each
    /// service at shutdown, and a transport that never sent never connected.
    pub fn drop_connection(&self) -> bool {
        match self.connection() {
            Some(connection) => {
                let dropped = write(connection).take().is_some();
                tracing::debug!(dropped, "dropping connection from transport");
                dropped
            }
            None => false,
        }
    }

    /// Sends one message over the connection of this transport.
    ///
    /// This is the account-side half of `send_to_sync`. It joins four
    /// operations on [`MailSync`] and does nothing else.
    ///
    /// The operations get the identity of the envelope sender, then the
    /// staging and destination mailboxes, then import and submit the message.
    ///
    /// The RFC 5322 bytes and the envelope arrive final: [`crate::send`] builds
    /// them from the arguments of Camel before it gets a connection.
    ///
    /// **One hold of the read lock across all four.** With one hold per
    /// operation, a `disconnect_sync` can occur between the identity request
    /// and the submission.
    ///
    /// Then the message is imported over one connection and submitted over
    /// another. Or it is not submitted, and a draft stays for a failed send.
    ///
    /// It is a *read* lock, the reason for the `RwLock` in the slot: when the
    /// outbox sends messages in sequence, a send does not wait for another upload.
    ///
    /// **The identity first, then the mailboxes.** Both are round trips before
    /// the upload, so a failure of either costs nothing.
    ///
    /// The order only selects the message the user gets when both fail. "This
    /// account cannot send as that address" names a choice the user can change
    /// in the composer.
    pub fn send_message(&self, source: Vec<u8>, envelope: Envelope) -> Result<Sent, StoreError> {
        let connection = self.connection().ok_or(StoreError::Disconnected)?;
        let connection = read(connection);
        let sync = connection.as_ref().ok_or(StoreError::Disconnected)?;

        tracing::debug!(size = source.len(), "sending message");

        let identity = match retry_once_after(
            || sync.identity_for(&envelope.mail_from.email),
            SyncError::is_unauthorized,
            || self.refresh_credentials(sync),
        ) {
            Ok(identity) => identity,
            Err(failure) => {
                tracing::debug!(?failure, "looking up sender identity failed");
                return Err(failure.into());
            }
        };
        let mailboxes = match retry_once_after(
            || sync.outgoing_mailboxes(),
            SyncError::is_unauthorized,
            || self.refresh_credentials(sync),
        ) {
            Ok(mailboxes) => mailboxes,
            Err(failure) => {
                tracing::debug!(?failure, "looking up outgoing mailboxes failed");
                return Err(failure.into());
            }
        };
        let outgoing = Outgoing {
            source,
            identity,
            envelope: Some(envelope),
            staging: mailboxes.staging.clone(),
            destination: mailboxes.destination.clone(),
        };

        // Retry staging and submitting separately, not as one attempt.
        // `sync.stage_outgoing_message` commits a visible draft on success.
        //
        // After a 401 on the submit alone, a retry of both requests imports
        // the message again. The retried submission then leaves a duplicate
        // draft. See the doc of `MailSync::stage_outgoing_message`.
        //
        // Both take `outgoing` by reference. Thus, when `retry_once_after` runs
        // an `attempt` twice, it stages or submits the same message again.
        let uid = match retry_once_after(
            || sync.stage_outgoing_message(&outgoing),
            SyncError::is_unauthorized,
            || self.refresh_credentials(sync),
        ) {
            Ok(uid) => uid,
            Err(failure) => {
                tracing::debug!(?failure, "staging outgoing message failed");
                return Err(failure.into());
            }
        };
        match retry_once_after(
            || sync.submit_staged_message(&uid, &outgoing),
            SyncError::is_unauthorized,
            || self.refresh_credentials(sync),
        ) {
            Ok(()) => {}
            Err(failure) => {
                tracing::debug!(?failure, "submitting staged message failed");
                return Err(failure.into());
            }
        }

        let sent = Sent {
            uid,
            saved: mailboxes.saves_sent_copy,
        };
        tracing::debug!(uid = sent.uid.as_str(), saved = sent.saved, "sent message");
        Ok(sent)
    }

    /// Whether a send finds a connection.
    pub fn is_connected(&self) -> bool {
        self.connection()
            .is_some_and(|connection| read(connection).is_some())
    }

    /// This transport as the `CamelService` that Camel gave to Evolution, or
    /// `None` on a [`JmapTransport::detached`] test instance.
    ///
    /// The transport counterpart of [`JmapStore::service`], with one step less:
    /// `Self` starts with `CamelTransport`, which derives from `CamelService`.
    ///
    /// Thus `self` and this pointer are the same object, typed as its C
    /// ancestor. The type check makes this sound on a detached instance too.
    ///
    /// [`JmapStore::service`]: crate::store::JmapStore::service
    fn service(&self) -> Option<*mut CamelService> {
        let ptr = ptr::from_ref(self).cast::<CamelService>().cast_mut();
        // SAFETY: `ptr` is `self` read as its leading `GTypeInstance` field.
        // `checked_borrow_ptr` checks it against the target type before any
        // further read, as the doc comment above says.
        unsafe { checked_borrow_ptr(ptr, camel_service_get_type()) }
    }

    /// Gets a new OAuth 2.0 access token for the account, installs it on the
    /// live connection, and tells whether a retry of the operation is useful.
    ///
    /// The transport twin of [`JmapStore::refresh_credentials`], with the same
    /// two "nothing to refresh" cases: no OAuth 2.0, or no `CamelService` to ask.
    ///
    /// [`JmapStore::refresh_credentials`]: crate::store::JmapStore::refresh_credentials
    fn refresh_credentials(&self, sync: &MailSync) -> bool {
        let Some(service) = self.service() else {
            return false;
        };
        // SAFETY: the check in `Self::service` makes `service` a live
        // `CamelService`. The settings reference is released when it drops.
        let settings = unsafe { Owned::from_raw(camel_service_ref_settings(service)) };
        // SAFETY: `settings` is NULL or the `CamelSettings` referenced above.
        let uses_oauth2 = settings
            .as_ref()
            .is_some_and(|settings| unsafe { oauth2::uses_oauth2(settings.as_ptr()) });
        if !uses_oauth2 {
            return false;
        }

        // SAFETY: the check above makes `service` a valid, registered
        // `CamelService`. The session reference is released when it drops.
        let Some(session) = (unsafe { Owned::from_raw(camel_service_ref_session(service)) }) else {
            return false;
        };
        // SAFETY: a valid session and a valid, registered service. No
        // cancellable: the user did not ask to stop this refresh, and
        // `access_token` accepts NULL for "not cancellable".
        match unsafe { oauth2::access_token(session.as_ptr(), service, ptr::null_mut()) } {
            Ok(token) => {
                tracing::debug!("refreshed the transport connection's OAuth 2.0 access token");
                sync.client().set_credentials(Credentials::bearer(token));
                true
            }
            Err(failure) => {
                tracing::debug!(
                    ?failure,
                    "refreshing the transport connection's access token failed"
                );
                false
            }
        }
    }

    /// For the tests only, as [`JmapStore::inspect_connection`].
    ///
    /// [`JmapStore::inspect_connection`]: crate::store::JmapStore::inspect_connection
    #[cfg(feature = "testing")]
    pub fn inspect_connection<R>(&self, f: impl FnOnce(&MailSync) -> R) -> Option<R> {
        let connection = self.connection()?;
        let guard = read(connection);
        guard.as_ref().map(f)
    }

    /// An instance outside the GObject type system: zeroed parent bytes and an
    /// initialised slot. This is the result of `instance_init`, without the GObject.
    ///
    /// For the tests, with the full caveat of [`JmapStore::detached`]: the
    /// parent bytes are a valid bit pattern, but they are not a GObject.
    ///
    /// Thus a Camel function that gets one of these has undefined behaviour.
    /// Touch only the slot of the result.
    ///
    /// [`JmapStore::detached`]: crate::store::JmapStore::detached
    #[cfg(feature = "testing")]
    pub fn detached() -> Box<Self> {
        // SAFETY: each field of the parent is a pointer or an integer, for
        // which all-zero is a valid value. An all-zero `Slot` is its
        // documented empty state.
        let transport: Box<Self> = unsafe { zeroed_box() };
        transport.connection.init(RwLock::new(None));
        transport
    }

    /// The Rust view of a `CamelTransport *` from Camel.
    ///
    /// # Safety
    ///
    /// `transport` must be NULL or point to an instance of this type. Camel
    /// dispatches the vfuncs of a class only on instances of that class.
    ///
    /// Thus a vfunc argument satisfies this. Other callers must check with
    /// `G_TYPE_CHECK_INSTANCE_TYPE` first.
    pub unsafe fn borrow<'a>(transport: *mut CamelTransport) -> Option<&'a Self> {
        // SAFETY: the doc comment above states the same contract.
        unsafe { dispatched_borrow(transport) }
    }

    /// The connection slot, or `None` before `instance_init` or after
    /// `finalize`.
    fn connection(&self) -> Option<&RwLock<Option<MailSync>>> {
        self.connection.get()
    }
}

// SAFETY: `JmapTransport` is the instance struct of the type `transport_type`
// registers. That type derives from `CamelTransport`, from `CamelService`.
unsafe impl Connected for JmapTransport {
    fn hold_connection(&self, sync: MailSync) {
        self.install_connection(sync);
    }

    fn release_connection(&self) {
        self.drop_connection();
    }

    fn holds_connection(&self) -> bool {
        self.is_connected()
    }
}

/// The class struct, with the same rule one level up. It adds no fields now.
/// It exists as our class for the `send_to_sync` override of a parent slot.
#[repr(C)]
pub struct JmapTransportClass {
    parent_class: CamelTransportClass,
}

// SAFETY: both structs are #[repr(C)] and start with the CamelTransport instance
// and class structs. The tests/layout.rs of eds-sys checks those layouts against
// `g_type_query`. CamelTransport derives from CamelService, from GObject.
unsafe impl ObjectSubclass for JmapTransport {
    /// `CamelJmapTransport`, as the Camel names `CamelSmtpTransport` and
    /// `CamelSendmailTransport`. A user sees the type name in a GObject warning
    /// about the wrong service type.
    const NAME: &'static CStr = c"CamelJmapTransport";
    type Instance = JmapTransport;
    type Class = JmapTransportClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { camel_transport_get_type() }
    }

    fn class_init_types() -> Vec<GType> {
        // Register the settings type before this type, not in `class_init`.
        // `class_init` runs under the GLib class-initialisation lock.
        // Taking the registration lock there inverts the two locks: the
        // deadlock that `tests/transport.rs` found.
        vec![settings_type()]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // The settings class of the account, for the reason in the module docs:
        // the inherited class is `CamelSettings`, which has no server.
        //
        // SAFETY: the class starts with CamelTransportClass, which starts with
        // CamelServiceClass: the contract above.
        let service = class.cast::<CamelServiceClass>();
        unsafe { (*service).settings_type = settings_type() };

        // Connect, authenticate, disconnect, and the name of this service: the
        // same four as in the store, parameterised by the dispatched service.
        //
        // SAFETY: as above, and `Self` is the type that is being registered.
        unsafe { crate::service::install_vfuncs::<Self>(service) };

        // The reason this type exists. `camel_transport_send_to_sync` does
        // `g_return_val_if_fail (class->send_to_sync != NULL, FALSE)`. Without
        // this slot, the account offers to send and then gives a GLib critical.
        //
        // SAFETY: the class starts with `CamelTransportClass`, which is the
        // contract at the top of this impl.
        unsafe { crate::send::install_vfuncs(class.cast::<CamelTransportClass>()) };
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // SAFETY: `instance` points to our zeroed instance struct, and a
        // zeroed `Slot` is an empty one.
        unsafe { (*instance).connection.init(RwLock::new(None)) };
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // SAFETY: the instance is being finalized, so nothing can still reach
        // it and no borrow from `get` is alive. Without this, the connection
        // and its socket outlive the account.
        unsafe { (*instance).connection.clear() };
    }
}

/// Registers the transport type, or returns it if it is already registered.
///
/// Static, as the store type, for the same reason: a Camel provider is not a
/// `GTypeModule`, Camel never closes the object it dlopened, and the provider
/// struct names these types permanently.
pub fn transport_type() -> GType {
    register_static::<JmapTransport>()
}
