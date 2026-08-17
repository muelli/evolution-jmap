// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The calls whose *shape* differs between the EDS releases this crate is
//! generated against, behind one name each.
//!
//! `eds-sys` is otherwise a pure bindgen surface: whatever the installed
//! headers declare is what it exports, and a caller writes the call the way
//! that EDS spells it. That stops working the moment two supported releases
//! spell the same operation differently — EDS 3.60 dropped
//! `e_vcard_to_string`'s format argument and gave
//! `e_contact_date_to_string` a version one — because then every call site
//! would need its own `#[cfg]`, and a build-script cfg does not reach the
//! crates that depend on this one anyway.
//!
//! So the difference is resolved here, once, and the rest of the tree calls
//! the wrapper. Each wrapper is chosen by a `#[cfg]` that `build.rs`
//! feature-detected from the generated bindings (see its `EDS_FEATURES`), not
//! from a version comparison — and the two arms are written so that a wrong
//! detection is a *compile* error on one leg rather than a silently different
//! vCard on it.
//!
//! What this module is deliberately not: a place to paper over an API EDS
//! genuinely removed. `CamelFolderSearch` and the summary-database row structs
//! are gone from 3.60 with no drop-in replacement, and inventing one here
//! would hide a port that has to be designed. Those stay `#[cfg]`-gated at
//! their (test) call sites and recorded in `docs/eds-version-matrix.md`.

use crate::{EContactDate, EVCard, gchar};

/// The vCard 3.0 text of `evc`, as a newly allocated string the caller frees
/// with `g_free`.
///
/// vCard 3.0 rather than whatever this EDS defaults to, because 3.0 is the
/// version the whole mapping in `jmap-vcard` is written against — its parser
/// documents itself as reading what `EVCard` emits — and on 3.60 the
/// one-argument `e_vcard_to_string` emits the card's *own* version instead.
/// Asking for 3.0 explicitly is what keeps the two legs producing the same
/// bytes.
///
/// # Safety
///
/// `evc` must be a valid, non-NULL pointer to a live `EVCard` (or to an object
/// that derives from one, such as an `EContact`).
pub unsafe fn e_vcard_to_string_vcard_30(evc: *mut EVCard) -> *mut gchar {
    #[cfg(eds_vcard_version_enum)]
    // SAFETY: the caller guarantees `evc`; `E_VCARD_VERSION_30` is this EDS's
    // own name for vCard 3.0.
    unsafe {
        crate::e_vcard_convert_to_string(evc, crate::E_VCARD_VERSION_30)
    }

    #[cfg(not(eds_vcard_version_enum))]
    // SAFETY: as above, with the pre-3.60 spelling of the same request.
    unsafe {
        crate::e_vcard_to_string(evc, crate::EVC_FORMAT_VCARD_30)
    }
}

/// `dt` written the way a vCard 3.0 `BDAY`/`ANNIVERSARY` line states it, as a
/// newly allocated string the caller frees with `g_free`.
///
/// Same reasoning as [`e_vcard_to_string_vcard_30`]: on 3.60 the call takes
/// the version to write for, and leaving it to a default would let the two
/// legs disagree about the format of a date.
///
/// # Safety
///
/// `dt` must be a valid, non-NULL pointer to an `EContactDate`.
pub unsafe fn e_contact_date_to_string_vcard_30(dt: *mut EContactDate) -> *mut gchar {
    #[cfg(eds_vcard_version_enum)]
    // SAFETY: the caller guarantees `dt`.
    unsafe {
        crate::e_contact_date_to_string(dt, crate::E_VCARD_VERSION_30)
    }

    #[cfg(not(eds_vcard_version_enum))]
    // SAFETY: as above; pre-3.60 there is no version to state.
    unsafe {
        crate::e_contact_date_to_string(dt)
    }
}
