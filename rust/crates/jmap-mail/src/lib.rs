// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The JMAP Camel provider (M5): the mail side of the account.
//!
//! Camel loads a mail provider on a path that has nothing in common with the
//! one EDS uses for the address book and calendar backends. There is no
//! `EModule` and no `GTypeModule`; Camel reads a `.urls` file beside each
//! shared object in its provider directory to learn which protocols that object
//! claims, and only dlopens the object — and calls
//! [`module::camel_provider_module_init`] — when one of those protocols is
//! actually asked for. What the entry point registers is a [`CamelProvider`], a
//! plain C struct handed over by pointer and kept forever, whose job is to name
//! the `GType`s Camel should instantiate.
//!
//! [`CamelProvider`]: eds_sys::CamelProvider
//!
//! This crate is that path, bottom up:
//!
//! - [`settings`] is `CamelJmapSettings`, the object a JMAP account's server
//!   is configured on. Camel keeps host, port, user and security method on the
//!   `CamelNetworkSettings` interface, which none of its stock settings classes
//!   implements, so every provider declares a settings class of its own.
//! - [`server`] reads that object back the other way round: the origin and
//!   user name a client is built from, with the host validation and the TLS
//!   rule shared with the EDS backends rather than written a second time.
//! - [`connect`] turns that origin into a live account: the client, the JMAP
//!   account id its mail lives in, and the two answers a failure has to give in
//!   Camel's vocabulary rather than EDS's.
//! - [`store`] is `CamelJmapStore`, the `CamelOfflineStore` subclass a JMAP
//!   account's folders hang off. It names the settings class above, holds the
//!   connection between `connect_sync` and `disconnect_sync`, and keeps the
//!   folder listing read over it — what `get_folder_info_sync` answers with,
//!   and what Camel's `REFRESH` flag decides whether to go and check.
//! - [`service`] is the `CamelService` half of that store as vfuncs:
//!   `connect_sync`, which asks the session to authenticate rather than opening
//!   anything itself, `authenticate_sync`, which is where the connection is
//!   actually made, and `disconnect_sync`.
//! - [`folders`] is the `CamelStore` half: `get_folder_info_sync`, and the
//!   reading of the two arguments that decide which part of the store's tree
//!   one call is answered with — the `top` it is rooted at and the depth
//!   `CAMEL_STORE_FOLDER_INFO_RECURSIVE` cuts it to.
//! - [`folder`] is `CamelJmapFolder`, the object one of those folders *is*
//!   rather than is described by: a `CamelOfflineFolder` subclass carrying the
//!   JMAP mailbox id that nothing in Camel's own model has a field for, and
//!   which a Camel path — invented here out of a mailbox name — cannot be
//!   turned back into.
//! - [`summary`] is where those rows live: the `CamelFolderSummary` every
//!   folder is given at construction, what a listing of the mailbox does to the
//!   rows already in it, and the flag that lets Camel ask a folder how much it
//!   holds. A subclass for one field's sake — the row type Camel instantiates
//!   when it reads the folder back off disk.
//! - [`changes`] is the other half of that answer: the uids one listing moved,
//!   in the `CamelFolderChangeInfo` Camel's `changed` signal carries — which is
//!   how a message list that is already on screen learns about new mail.
//! - [`refresh`] is `refresh_info_sync`, the folder vfunc that joins the three
//!   above to a server: the mailbox listed over the store's connection, the
//!   rows reconciled against what the folder holds, and the diff emitted.
//! - [`message`] is what a row is not: `get_message_sync`, the vfunc that
//!   downloads one message's RFC 5322 bytes over the store's connection and lets
//!   Camel's own parser turn them into the `CamelMimeMessage` the preview pane
//!   renders.
//! - [`cache`] is why it only downloads them once: a `CamelDataCache` under the
//!   account's cache directory, a file per message keyed by its JMAP id, which
//!   is what makes a second click free and a message already read openable with
//!   the account offline.
//! - [`message_info`] is one row of that folder's contents: the
//!   `CamelMessageInfo` a `jmap-mail-sync` summary row becomes, and with it the
//!   three columns that are a computation rather than a copy — the flags word,
//!   the formatted address headers, and the 64-bit digests Camel threads on —
//!   and a fourth that is neither, and is why the row is a subclass: the
//!   keywords the last listing found, which a flag change is the difference
//!   from and which nothing else on the row still holds once the user has
//!   marked it.
//! - [`synchronize`] is the only one of them that writes: `synchronize_sync`,
//!   the vfunc that walks the rows Camel marked as having to reach the server
//!   and turns each into the `Email/set` that closes the difference between the
//!   keywords the last listing found and the ones the row claims now.
//! - [`transfer`] is the other thing the user does that reaches the server:
//!   `transfer_messages_to_sync`, the vfunc behind dragging a message into
//!   another folder. Its patch is one `Email/set` over `mailboxIds` — a JMAP
//!   mailbox is a member of a set rather than a place, so a copy adds one and a
//!   move adds one and takes another away — and the work around it is the rows
//!   a move leaves the source folder holding.
//! - [`mime`] is the step every message this account uploads goes through: the
//!   `CamelMimeMessage` as the RFC 5322 octets a blob is made of, written by
//!   Camel's own emitter and line-ended the way a message on the wire has to
//!   be. Its own module because two unrelated objects need it — a folder taking
//!   a message in and a transport sending one out — and a second emitter is
//!   where the two would start to disagree.
//! - [`append`] is the arrival [`transfer`] cannot serve: `append_message_sync`,
//!   the vfunc that takes a `CamelMimeMessage` the account has never seen — one
//!   dragged out of another account, one the composer just built — writes it out
//!   through [`mime`] and puts it on the server as an `Email/import` over an
//!   uploaded blob.
//! - [`manage`] is the pair that changes which folders there are:
//!   `create_folder_sync` and `delete_folder_sync`, the vfuncs behind the
//!   folder the user adds to an account and the one they take away — one
//!   `Mailbox/set` each, and the edit to the store's held listing that keeps
//!   the folder Camel was just handed openable.
//! - [`envelope`] is the first piece of the half that has no store in it at
//!   all: the SMTP envelope a `CamelTransport`'s `send_to_sync` is handed as
//!   two `CamelAddress` arguments, turned into the `envelope` property RFC 8621
//!   §7 puts on an `EmailSubmission`. Its own module because it is the one part
//!   of sending that is a pure reading of Camel's objects — and because it is
//!   where the rule lives that an address with nothing deliverable in it
//!   refuses the send rather than shortening it.
//! - [`transport`] is the object that half hangs off: `CamelJmapTransport`, the
//!   second service Camel gives an account and the one that reads no folder. It
//!   is a service of its own rather than a view of the store because Camel gives
//!   it no way to reach one — so it has a connection of its own to the same
//!   server, opened by the same [`service`] vfuncs, and the account-side half of
//!   a send held under one lock of it.
//! - [`send`] is the slot that object exists for: `send_to_sync`, the vfunc a
//!   message leaves the account through. It joins [`envelope`] and [`mime`] to
//!   the identity, the two mailboxes and the import-and-submit — refusing
//!   everything it can before the upload, so that a send the user is told did
//!   not happen has left nothing behind — and answers Camel's
//!   `out_sent_message_saved`, which decides whether Evolution keeps a second
//!   copy of every message this account sends.
//! - [`provider`] is the struct itself: the protocol, what Evolution is allowed
//!   to offer a JMAP account as, and the two slots naming the store and the
//!   transport.
//! - [`module`] is the exported symbol, guarded like every other C entry point
//!   in this repository.
//!
//! Like the two EDS backends, this crate needs the installed headers — Camel's,
//! via `eds-sys` — and so stays out of the workspace's `default-members`; CMake
//! runs its tests through the `rust-test-eds` target and installs the built
//! `cdylib`, together with `libcameljmap.urls`, into Camel's provider
//! directory.

pub mod append;
pub mod cache;
pub mod changes;
pub mod connect;
pub mod envelope;
pub mod expunge;
pub mod folder;
pub mod folder_info;
pub mod folders;
pub mod manage;
pub mod message;
pub mod message_info;
pub mod mime;
pub mod module;
pub mod provider;
pub mod refresh;
pub mod send;
pub mod server;
pub mod service;
pub mod settings;
pub mod store;
pub mod subscribe;
pub mod summary;
pub mod synchronize;
pub mod transfer;
pub mod transport;
