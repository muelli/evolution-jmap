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
//! - [`provider`] is the struct itself: the protocol, what Evolution is allowed
//!   to offer a JMAP account as, and the store slot pointing at that type.
//! - [`module`] is the exported symbol, guarded like every other C entry point
//!   in this repository.
//!
//! Like the two EDS backends, this crate needs the installed headers — Camel's,
//! via `eds-sys` — and so stays out of the workspace's `default-members`; CMake
//! runs its tests through the `rust-test-eds` target and installs the built
//! `cdylib`, together with `libcameljmap.urls`, into Camel's provider
//! directory.

pub mod connect;
pub mod folder;
pub mod folder_info;
pub mod folders;
pub mod module;
pub mod provider;
pub mod server;
pub mod service;
pub mod settings;
pub mod store;
