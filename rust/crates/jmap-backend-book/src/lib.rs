// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP address book backend bridging EDS to [`BookSync`].
//!
//! Submodules:
//! - [`connect`]: opens [`BookSync`] connections from `ESource` configurations.
//! - [`marshal`]: converts between Rust types and EDS C structures.
//! - [`ops`]: backend operations operating over [`BookSync`].
//! - [`backend`]: `EBookMetaBackend` GObject subclass and vfunc dispatch.
//! - [`module`]: module loading routines for EDS.
//! - [`factory`]: `EBookBackendFactory` subclass mapping `BackendName=jmap`.
//!
//! [`BookSync`]: jmap_book_sync::BookSync

pub mod backend;
pub mod connect;
pub mod factory;
pub mod marshal;
pub mod module;
pub mod ops;

// Test binaries initialise the parent classes before their tests start threads.
#[cfg(feature = "testing")]
jmap_backend_core::initialise_parents_before_main!(
    backend::JmapBookBackend,
    factory::JmapBookFactory,
);
