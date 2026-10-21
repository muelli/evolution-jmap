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

// Test binaries initialise the parent classes before their tests start threads.
#[cfg(feature = "testing")]
jmap_backend_core::initialise_parents_before_main!(
    backend::JmapCalBackend,
    factory::JmapCalFactory,
);

// Fills libical's zone table before `main`, as `module::load` does in EDS.
#[cfg(feature = "testing")]
const _: () = {
    extern "C" fn load_builtin_zones() {
        marshal::load_builtin_zones();
    }

    #[used]
    #[unsafe(link_section = ".init_array")]
    static LOAD_BUILTIN_ZONES: extern "C" fn() = load_builtin_zones;
};
