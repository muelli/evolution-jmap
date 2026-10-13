// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The JMAP address book backend: an `EBookMetaBackend` subclass bridging EDS
//! to `jmap-book-sync`.
//!
//! Submodules:
//! - [`connect`]: opens [`BookSync`] connections from `ESource` configurations.
//! - [`marshal`]: conversions between Rust types and EDS C structures.
//! - [`ops`]: core backend operation bodies operating over `BookSync`.
//! - [`backend`]: `EBookMetaBackend` GObject subclass and vfunc dispatch.
//! - [`module`]: dynamic module loading routines called by EDS.
//! - [`factory`]: `EBookBackendFactory` subclass mapping `BackendName=jmap`.
//!
//! [`BookSync`]: jmap_book_sync::BookSync

pub mod backend;
pub mod connect;
pub mod factory;
pub mod marshal;
pub mod module;
pub mod ops;
