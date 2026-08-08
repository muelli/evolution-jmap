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
//! - [`store`] is `CamelJmapStore`, the `CamelOfflineStore` subclass a JMAP
//!   account's folders will hang off. Empty so far — registering the type and
//!   picking its parent is the increment; `Mailbox/get` is the next one.
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

pub mod module;
pub mod provider;
pub mod store;
