// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JMAP calendar backend bridging Evolution Data Server to `jmap-cal-sync`.
//!
//! Submodules:
//! - [`connect`]: opens [`CalSync`] connections from `ESource` configurations.
//! - [`marshal`]: conversions between Rust types and EDS C structures.
//! - [`ops`]: backend operations using `CalSync`.
//! - [`scheduling`]: meeting invitation routing controls.
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
