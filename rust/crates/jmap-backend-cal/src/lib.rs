// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The JMAP calendar backend: an `ECalMetaBackend` subclass bridging EDS
//! to `jmap-cal-sync`.
//!
//! Submodules:
//! - [`connect`]: opens [`CalSync`] connections from `ESource` configurations.
//! - [`marshal`]: conversions between Rust types and EDS C structures.
//! - [`ops`]: core backend operation bodies operating over `CalSync`.
//! - [`scheduling`]: who sends meeting invitations, the server or Evolution.
//! - [`backend`]: `ECalMetaBackend` GObject subclass and vfunc dispatch.
//! - [`module`]: dynamic module loading routines called by EDS.
//! - [`factory`]: `ECalBackendFactory` subclass mapping `BackendName=jmap`.
//!
//! [`CalSync`]: jmap_cal_sync::CalSync

pub mod backend;
pub mod connect;
pub mod factory;
pub mod marshal;
pub mod module;
pub mod ops;
pub mod scheduling;
