// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a populate keeps for the *next* edit of the account: one `"changed"`
//! connection, and the state that decides if that edit needs a new login.
//!
//! EDS does less here than it seems to do. `ECollectionBackend` schedules a
//! populate again for four events only (`e-collection-backend.c`, 3.52.3):
//!
//! - the three `ESourceCollection` toggles
//!   `calendar-enabled`/`contacts-enabled`/`mail-enabled`
//!   (`collection_backend_notify_collection_cb`);
//! - a transition from offline to online, at most once per 24 hours
//!   (`collection_backend_online_changed_cb`);
//! - construction;
//! - a manual `e_source_registry_refresh_backend()`.
//!
//! A general edit of the account schedules nothing. Thus a corrected host or
//! port during a session leaves the account in the state of its last failed
//! login. This continues until the owner disables and enables it, or asks for
//! a refresh.
//!
//! `e-ews-backend.c` solves this partly. `ews_backend_populate` connects the
//! `"changed"` signal of the account source to `ews_backend_source_changed_cb`,
//! which runs populate again. This module does the same, with two decisions of
//! its own.
//!
//! ## Why no handler id, and no `dispose` override
//!
//! EWS keeps the handler id in `priv->source_changed_id`. It disconnects it in
//! `ews_backend_dispose`, and reads the source from the backend to do so.
//! [`crate::backend`] uses `g_signal_connect_object` on the backend instead,
//! and so stores nothing. That call adds a GObject `g_object_watch_closure`,
//! which gives two guarantees.
//!
//! - It wraps the marshal in `g_object_ref`/`g_object_unref` of the watched
//!   object, so the backend cannot be freed *during* a handler call.
//! - It removes the closure in `g_object_real_dispose`, so the connection is
//!   gone before the backend finalizes.
//!
//! A hand-written disconnect can fail: if it finds a different source, or no
//! source, a live handler keeps a freed backend. `insert_widgets` in
//! `jmap_config` gives the same reasoning.
//!
//! ## Why an edit after a working login does nothing
//!
//! A `"changed"` fires for every setting of the account, the display name
//! included. A repopulate ends in `e_backend_schedule_credentials_required`,
//! and thus in a new network fan-out. A repopulate for every edit would log in
//! again for a rename. It would also double the response to the three
//! enabled-toggles, because EDS schedules a populate for those itself.
//!
//! [`AccountWatch`] gives the same answer as `need_update_folders` in EWS. A
//! populate marks the account unproven, and a successful fan-out marks it
//! proven. A `"changed"` repopulates only while the account is unproven, and
//! only when the login settings differ from those the populate read. Thus the
//! edit that repairs a broken account gets its retry, and later edits cost
//! nothing.
//!
//! The cost is the opposite case. If a *working* account gets a broken host,
//! nothing notices until something else connects it again. EWS accepts the
//! same cost.
//!
//! ## Why the own write of a populate is not an edit
//!
//! One `"changed"` of every account comes from this backend. A populate sets
//! `allow-sources-rename` of `ESourceCollection`. Its pspec has
//! `E_SOURCE_PARAM_SETTING` and its default is FALSE. A settings property
//! reaches `e_source_changed` through the `notify` override of
//! `ESourceExtension` (`e-source-extension.c`, 3.52.3).
//!
//! The emission is an idle, so it arrives after the populate returns. That is
//! after the populate connected this handler, and before a fan-out has proved
//! anything. A repopulate in response would ask every account for a second
//! login at its first populate. `e_backend_schedule_authenticate` also cancels
//! the first login to start the second. evolution-ews writes the flag in
//! `constructed`, before its handler exists. This backend has no `constructed`
//! override.
//!
//! Thus `"changed"` alone is not the test. [`login_fingerprint`] is the test:
//! the login settings of the account as one number, recorded by the populate
//! that read them. An edit that does not change it changes nothing for a
//! second login, independent of who made the edit. The three enabled-toggles
//! are deliberately not in it, because EDS schedules a populate for those
//! itself.
//!
//! `e_source_changed` also combines a burst of edits into one idle emission.
//! Thus *Apply* in an account editor gives one `"changed"`, not one per field.
//!
//! Only one fan-out can run at a time, whatever starts it. `EBackend` holds
//! `priv->authenticate_lock` around all of the `authenticate_sync` vfunc ("To
//! not run multiple authenticate requests simultaneously", `e-backend.c`
//! 3.52.3). This crate starts a fan-out from no other place.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use eds_sys::ESource;
use jmap_backend_core::api_token::source_uses_api_token;
use jmap_backend_core::oauth2::source_uses_oauth2;

use crate::collection_source::{server_of, user_of};

/// The account settings that a login uses, as one number.
///
/// This is all that [`login_of`](crate::authenticate::login_of) reads from the
/// account, except its parts: the server, the user, and the authentication
/// method. If two reads agree, the account logs in identically, so the handler
/// of this module has nothing to retry.
///
/// # Safety
///
/// `source` must be a valid `ESource`: the account from which EDS constructed
/// the backend. It is only read from, and nothing outlives the call.
pub unsafe fn login_fingerprint(source: *mut ESource) -> u64 {
    // SAFETY: a valid source by the contract of this function, only read from.
    let (server, user, uses_oauth2, uses_api_token) = unsafe {
        (
            server_of(source),
            user_of(source),
            source_uses_oauth2(source),
            source_uses_api_token(source),
        )
    };

    let mut hasher = DefaultHasher::new();
    // Through `Debug`, not `Hash`: a new field of `Server` then becomes part of
    // the fingerprint automatically.
    format!("{server:?}").hash(&mut hasher);
    user.hash(&mut hasher);
    uses_oauth2.hash(&mut hasher);
    uses_api_token.hash(&mut hasher);
    hasher.finish()
}

/// The state that one collection backend keeps between a populate and the
/// later edits of the account.
///
/// It is inline in [`JmapCollectionBackend`](crate::backend::JmapCollectionBackend),
/// so all-zero must be its initial state. GObject gives `instance_init` a
/// zeroed instance struct. Zero here means "nothing connected, nothing proven",
/// which is true for an account before its first populate. `finalize` has
/// nothing to release, because no field owns anything.
///
/// The fields are atomics, not `Cell`s, because the vfuncs of a backend run on
/// different threads. `populate` runs on the main loop of
/// `evolution-source-registry`. [`authenticate_sync`](crate::backend) runs on
/// the thread on which EDS resolved the credentials.
#[derive(Debug, Default)]
#[repr(C)]
pub struct AccountWatch {
    /// Whether the `"changed"` handler of the account is already connected.
    connected: AtomicBool,
    /// Whether a fan-out has succeeded since the last populate began.
    proven: AtomicBool,
    /// The [`login_fingerprint`] that the last populate read from the account.
    /// Zero means "no populate has recorded one", so an edit causes a retry.
    /// That is the safe direction. It cannot occur, because only a populate
    /// connects the handler.
    login: AtomicU64,
}

impl AccountWatch {
    /// Not connected, not proven: the zeroed state, written explicitly.
    pub const fn new() -> Self {
        Self {
            connected: AtomicBool::new(false),
            proven: AtomicBool::new(false),
            login: AtomicU64::new(0),
        }
    }

    /// Whether this caller is the one that should connect the handler.
    ///
    /// True for the first caller only. Without this, each reconnect adds a
    /// populate and a handler, and the repopulates double with every pass.
    pub fn claim_connection(&self) -> bool {
        self.connected
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// A populate has started on the account for which [`login_fingerprint`]
    /// gave `login`. Its settings are unproven again until a fan-out succeeds
    /// with them.
    pub fn populating(&self, login: u64) {
        self.login.store(login, Ordering::SeqCst);
        self.proven.store(false, Ordering::SeqCst);
    }

    /// A fan-out succeeded: this code reached the server of the account, and
    /// the server accepted its credentials.
    pub fn proven(&self) {
        self.proven.store(true, Ordering::SeqCst);
    }

    /// Whether an edit that gives the account the [`login_fingerprint`]
    /// `login` needs a new populate.
    pub fn wants_repopulate(&self, login: u64) -> bool {
        !self.proven.load(Ordering::SeqCst) && self.login.load(Ordering::SeqCst) != login
    }
}

#[cfg(test)]
mod tests {
    use super::AccountWatch;

    /// The fingerprints of the login settings of two accounts.
    const ONE_SERVER: u64 = 0x1111_1111_1111_1111;
    const ANOTHER_SERVER: u64 = 0x2222_2222_2222_2222;

    #[test]
    fn only_the_first_caller_connects() {
        let watch = AccountWatch::new();
        assert!(watch.claim_connection());
        assert!(!watch.claim_connection());
        assert!(!watch.claim_connection());
    }

    #[test]
    fn an_account_no_populate_has_read_yet_is_worth_a_try() {
        let watch = AccountWatch::new();
        assert!(
            watch.wants_repopulate(ONE_SERVER),
            "the zeroed state has recorded no login, so an edit is worth one"
        );
    }

    #[test]
    fn a_setting_the_populate_wrote_itself_is_not_an_edit() {
        let watch = AccountWatch::new();
        watch.populating(ONE_SERVER);

        assert!(
            !watch.wants_repopulate(ONE_SERVER),
            "a populate writes settings of its own, and hearing one back as \
             an edit asks the account to log in a second time"
        );
    }

    #[test]
    fn an_edit_that_names_a_different_server_is_worth_another_try() {
        let watch = AccountWatch::new();
        watch.populating(ONE_SERVER);

        assert!(
            watch.wants_repopulate(ANOTHER_SERVER),
            "a corrected host has no other chance of being tried"
        );
    }

    #[test]
    fn an_edit_after_a_working_login_asks_for_nothing() {
        let watch = AccountWatch::new();
        watch.populating(ONE_SERVER);
        watch.proven();

        assert!(
            !watch.wants_repopulate(ANOTHER_SERVER),
            "breaking a working account is noticed when something reconnects \
             it, not by a fresh fan-out per edit"
        );
    }

    #[test]
    fn a_reconnect_makes_the_account_unproven_again() {
        let watch = AccountWatch::new();
        watch.proven();
        watch.populating(ONE_SERVER);

        assert!(
            watch.wants_repopulate(ANOTHER_SERVER),
            "the fan-out that proved the account was for the previous populate"
        );
    }

    #[test]
    fn the_connection_survives_a_repopulate() {
        let watch = AccountWatch::new();
        assert!(watch.claim_connection());
        watch.proven();
        watch.populating(ONE_SERVER);

        assert!(
            !watch.claim_connection(),
            "a populate re-run by the handler must not connect a second one"
        );
    }
}
