// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelJmapTransport`: the service an account's mail leaves through.
//!
//! Camel splits an account in two. The [`crate::store`] is the mail the account
//! *holds* — folders, summaries, message bodies — and a `CamelTransport` is the
//! mail it *sends*, and the two are separate objects with no pointer between
//! them. Evolution constructs each from its own `ESource`, gives each its own
//! settings object, and connects each on its own schedule: the store when the
//! user opens a folder, the transport when they press Send.
//!
//! That split is a fact about Camel rather than about JMAP, where both halves
//! are the same session against the same server. It is still the shape this
//! type has to take, because there is no supported way for a transport to reach
//! the store of the account it belongs to: `camel_session_get_service` needs a
//! uid this service does not carry, and Evolution's pairing of the two lives in
//! `EMailAccountStore`, above Camel. So the transport opens a connection of its
//! own — one more HTTP client against a server the account is already talking
//! to, made once per send session rather than per message.
//!
//! What it therefore carries is one slot and no more: the connection, between
//! `connect_sync` and `disconnect_sync`. There is no folder listing, because a
//! transport lists nothing; the mailboxes sending needs — the one a message is
//! staged in and the one it is filed into afterwards — are read over the
//! connection at send time, and are not state this object keeps.
//!
//! ## What is shared with the store, and what is not
//!
//! The settings class is [`crate::settings`], the same one: it is the same
//! account, and a transport that inherited `CamelSettings` would have no host,
//! no port and no user for `e_source_camel_configure_service` to write. The
//! `CamelService` vfuncs are [`crate::service`]'s, the same four, installed here
//! for this type — connecting is the same operation on either service, and the
//! only thing that differs is which object the connection it produced is put
//! on. That is [`Connected`], which this type implements below.
//!
//! Not shared: the connection itself. Two services, two clients, and neither
//! disconnect takes the other's away.
//!
//! ## What is not here yet
//!
//! `send_to_sync`, which is the reason the type exists. Every piece it joins is
//! now written and tested — [`crate::envelope`] for the addresses Camel hands
//! it, [`crate::mime`] for the message it is handed as the bytes that go up,
//! [`MailSync::identity_for`] for the identity to submit through,
//! [`MailSync::outgoing_mailboxes`] for where the message waits and is filed,
//! and [`MailSync::send_message`] for the import-and-submit itself — but
//! nothing calls them yet, and until something does the provider's transport
//! slot stays `G_TYPE_INVALID`. A registered transport whose `send_to_sync` is
//! NULL would be an account that offers to send and fails with a GLib critical.
//!
//! [`MailSync::identity_for`]: jmap_mail_sync::MailSync::identity_for
//! [`MailSync::outgoing_mailboxes`]: jmap_mail_sync::MailSync::outgoing_mailboxes
//! [`MailSync::send_message`]: jmap_mail_sync::MailSync::send_message

use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::sync::RwLock;

use eds_sys::{CamelServiceClass, CamelTransport, CamelTransportClass, camel_transport_get_type};
use glib_sys::GType;
use jmap_backend_core::instance::Slot;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_mail_sync::MailSync;

use crate::service::Connected;
use crate::settings::settings_type;
use crate::store::{read, write};

/// The instance struct. `#[repr(C)]` leading with the parent's instance struct
/// is what makes a `*mut JmapTransport` usable as the `CamelTransport *` every
/// Camel function takes.
#[repr(C)]
pub struct JmapTransport {
    parent: CamelTransport,
    /// The connection, from `connect_sync` to `disconnect_sync`.
    ///
    /// An `RwLock` for the store's reason rather than out of symmetry: Camel
    /// sends from a thread of its own, and Evolution's outbox will hand a
    /// transport several messages one after another while the user carries on
    /// composing. Only connect and disconnect, which replace the value, need
    /// exclusive access; a send is a *use* of the connection and takes the read
    /// lock, so a second send does not queue behind the first's upload.
    connection: Slot<RwLock<Option<MailSync>>>,
}

impl JmapTransport {
    /// Installs `sync` as the live connection, replacing whatever was there.
    ///
    /// Replacing rather than refusing, for [`JmapStore::store_connection`]'s
    /// reason: Camel reconnects a service it believes has gone away, and the
    /// connection being replaced is the one it believes that about. Nothing is
    /// dropped alongside it — unlike the store, this service holds no listing
    /// that the old connection's server is the only explanation for.
    ///
    /// [`JmapStore::store_connection`]: crate::store::JmapStore::store_connection
    pub fn install_connection(&self, sync: MailSync) {
        if let Some(connection) = self.connection() {
            *write(connection) = Some(sync);
        }
    }

    /// Drops the connection, reporting whether there was one.
    ///
    /// "There was none" is a normal outcome rather than a failure: Camel asks
    /// every service to disconnect on shutdown, and an account whose user never
    /// sent anything has a transport that never connected.
    pub fn drop_connection(&self) -> bool {
        match self.connection() {
            Some(connection) => write(connection).take().is_some(),
            None => false,
        }
    }

    /// Whether a send would find a connection.
    pub fn is_connected(&self) -> bool {
        self.connection()
            .is_some_and(|connection| read(connection).is_some())
    }

    /// An instance outside the GObject type system: zeroed parent bytes and an
    /// initialised slot, which is what `instance_init` leaves behind minus the
    /// GObject.
    ///
    /// For the tests, and with [`JmapStore::detached`]'s caveat in full: the
    /// parent bytes are a valid bit pattern but they are not a GObject, so
    /// passing one of these to any Camel function is undefined behaviour.
    /// Nothing but the slot may be touched through the result.
    ///
    /// [`JmapStore::detached`]: crate::store::JmapStore::detached
    pub fn detached() -> Box<Self> {
        // SAFETY: every field of the parent is a pointer or an integer, for
        // which all-zero is a valid value, and an all-zero `Slot` is its
        // documented empty state.
        let transport: Box<Self> = Box::new(unsafe { MaybeUninit::zeroed().assume_init() });
        transport.connection.init(RwLock::new(None));
        transport
    }

    /// The Rust view of a `CamelTransport *` Camel handed over.
    ///
    /// # Safety
    ///
    /// `transport` must be NULL or point at an instance of this type. Camel
    /// only dispatches a class's vfuncs on instances of that class, so a
    /// vfunc's argument satisfies this; anything else has to check with
    /// `G_TYPE_CHECK_INSTANCE_TYPE` first.
    pub unsafe fn borrow<'a>(transport: *mut CamelTransport) -> Option<&'a Self> {
        unsafe { transport.cast::<Self>().as_ref() }
    }

    /// The connection slot, or `None` on an instance whose `instance_init` has
    /// not run or whose `finalize` already has.
    fn connection(&self) -> Option<&RwLock<Option<MailSync>>> {
        self.connection.get()
    }
}

// SAFETY: `JmapTransport` is the instance struct of the type `transport_type`
// registers, and that type derives from `CamelTransport` — from `CamelService`.
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

/// The class struct, same rule one level up. It adds nothing of its own today;
/// `send_to_sync` is an override of a slot the parent already declares, so what
/// this exists for is to be a class of ours for that override to be installed
/// on.
#[repr(C)]
pub struct JmapTransportClass {
    parent_class: CamelTransportClass,
}

// SAFETY: both structs are #[repr(C)] and lead with the CamelTransport instance
// and class structs, whose layouts eds-sys's tests/layout.rs checks against
// `g_type_query`; CamelTransport derives from CamelService, from GObject.
unsafe impl ObjectSubclass for JmapTransport {
    /// `CamelJmapTransport`, matching Camel's own `CamelSmtpTransport` and
    /// `CamelSendmailTransport`: the type name is what a user sees in a GObject
    /// warning about the wrong service type.
    const NAME: &'static CStr = c"CamelJmapTransport";
    type Instance = JmapTransport;
    type Class = JmapTransportClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { camel_transport_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // The account's own settings class, for the reason the module docs
        // give: inherited it would be `CamelSettings`, which carries no server.
        //
        // SAFETY: the class leads with CamelTransportClass, which leads with
        // CamelServiceClass — the contract above.
        let service = class.cast::<CamelServiceClass>();
        unsafe { (*service).settings_type = settings_type() };

        // Connect, authenticate, disconnect, and the name Camel calls this
        // service by — the same four the store installs, parameterised by which
        // service they were dispatched on.
        //
        // SAFETY: as above, and `Self` is the type being registered.
        unsafe { crate::service::install_vfuncs::<Self>(service) };
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // SAFETY: `instance` points at a zeroed instance struct of ours, and a
        // zeroed `Slot` is an empty one.
        unsafe { (*instance).connection.init(RwLock::new(None)) };
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // SAFETY: the instance is being finalized, so nothing can still reach
        // it and no borrow handed out by `get` is alive. Without this the
        // connection — and its socket — outlives the account.
        unsafe { (*instance).connection.clear() };
    }
}

/// Registers the transport type, or returns it if it is already registered.
///
/// Statically, like the store's and for the same reason: a Camel provider is
/// not a `GTypeModule`, Camel never closes the object it dlopened, and the
/// provider struct names these types forever.
pub fn transport_type() -> GType {
    register_static::<JmapTransport>()
}
