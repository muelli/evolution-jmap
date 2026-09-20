// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a populate leaves behind so the *next* edit of the account can reach
//! it: one `"changed"` connection, and the one bit of state that decides
//! whether that edit is worth another login.
//!
//! EDS does less here than it looks like it does. `ECollectionBackend`
//! reschedules a populate for exactly four things (`e-collection-backend.c`,
//! 3.52.3): the three `ESourceCollection` toggles
//! `calendar-enabled`/`contacts-enabled`/`mail-enabled`
//! (`collection_backend_notify_collection_cb`), an offline-to-online
//! transition throttled to once per 24 hours
//! (`collection_backend_online_changed_cb`), construction, and a manual
//! `e_source_registry_refresh_backend()`. Nothing reschedules on a generic
//! edit of the account. So correcting a mistyped host or port mid-session
//! leaves the account in whatever state its last failed login left it in,
//! until its owner disables and re-enables it or asks for a refresh.
//!
//! `e-ews-backend.c` exists partly to cover that: `ews_backend_populate`
//! connects the account source's own `"changed"` to
//! `ews_backend_source_changed_cb`, which re-runs populate. This module is
//! that, and two decisions of its own.
//!
//! ## Why no handler id, and no `dispose` override
//!
//! EWS keeps the handler id in `priv->source_changed_id` and disconnects it in
//! `ews_backend_dispose`, reading the source back out of the backend to do so.
//! [`crate::backend`] uses `g_signal_connect_object` against the backend
//! instead, and so stores nothing. GObject's own `g_object_watch_closure`,
//! which is what that call adds, gives two guarantees a hand-written
//! disconnect has to be written correctly to match: it wraps the marshal in
//! `g_object_ref`/`g_object_unref` of the watched object, so the backend
//! cannot be freed *during* a handler call, and it drops the closure from
//! `g_object_real_dispose`, so the connection is gone by the time the backend
//! finalizes. What it removes is the failure mode the hand-written version
//! has: a disconnect that looks up the source again and finds a different one,
//! or none, leaves a live handler holding a freed backend. The same reasoning
//! is written out at `jmap_config`'s own `insert_widgets`.
//!
//! ## Why an edit after a working login does nothing
//!
//! A `"changed"` fires for every setting of the account, its display name
//! included, and a repopulate ends in `e_backend_schedule_credentials_required`
//! and so in a fresh network fan-out. Answering every edit with one would
//! re-login for a rename, and would answer the three enabled-toggles twice
//! over, since EDS reschedules a populate for those itself.
//!
//! [`AccountWatch`] is the same answer EWS's `need_update_folders` is: a
//! populate marks the account unproven, a successful fan-out marks it proven,
//! and a `"changed"` only repopulates while it is unproven -- and only when the
//! account no longer names the login the populate read. So the edit that fixes
//! a broken account gets its retry, and every later edit is free.
//!
//! The cost is the mirror case: breaking a *working* account's host is not
//! noticed until something else reconnects it. That is what EWS settles for
//! too, and it is not a regression either way — before this module there was
//! no handler at all, so no edit of any kind was noticed.
//!
//! ## Why a populate's own write is not an edit
//!
//! One `"changed"` of every account is this backend's own. A populate sets
//! `ESourceCollection`'s `allow-sources-rename`, whose pspec carries
//! `E_SOURCE_PARAM_SETTING` and whose default is FALSE, and a settings
//! property reaches `e_source_changed` through `ESourceExtension`'s `notify`
//! override (`e-source-extension.c`, 3.52.3). The emission is an idle, so it
//! lands after the populate that wrote it returned -- after that same populate
//! connected this handler, and long before a fan-out has proved anything.
//! Answering it with a repopulate would ask every account for a second login
//! the first time it is ever populated, and `e_backend_schedule_authenticate`
//! cancels the first login to start the second. evolution-ews does not meet
//! this because it writes the flag in `constructed`, before its own handler
//! exists; this backend has no `constructed` override.
//!
//! So `"changed"` on its own is not the question. [`login_fingerprint`] is:
//! the account settings a login is built from, as one number, recorded by the
//! populate that read them. An edit that leaves it alone changed nothing a
//! second login would do differently, whoever made the edit. The three
//! enabled-toggles are deliberately outside it, because EDS reschedules a
//! populate for those itself. `e_source_changed` also coalesces a burst of
//! edits into a single idle emission, so an account editor's *Apply* arrives
//! as one `"changed"` rather than one per field.
//!
//! Only one fan-out can be in flight whatever asks for it: `EBackend` takes
//! `priv->authenticate_lock` around the whole `authenticate_sync` vfunc ("To
//! not run multiple authenticate requests simultaneously", `e-backend.c`
//! 3.52.3), and this crate fans out from nowhere else.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use eds_sys::ESource;
use jmap_backend_core::api_token::source_uses_api_token;
use jmap_backend_core::oauth2::source_uses_oauth2;

use crate::collection_source::{server_of, user_of};

/// The account settings a login is built from, as one number.
///
/// Everything [`login_of`](crate::authenticate::login_of) reads out of the
/// account except its parts: where the server is, as whom, and by which
/// authentication method. Two reads that agree describe an account that would
/// log in exactly the same way, so an edit between them is not one this
/// module's handler has anything to retry for.
///
/// # Safety
///
/// `source` must be a valid `ESource` -- the account EDS constructed the
/// backend from. It is only read from, and nothing outlives the call.
pub unsafe fn login_fingerprint(source: *mut ESource) -> u64 {
    // SAFETY: a valid source by this function's contract, only read from.
    let (server, user, uses_oauth2, uses_api_token) = unsafe {
        (
            server_of(source),
            user_of(source),
            source_uses_oauth2(source),
            source_uses_api_token(source),
        )
    };

    let mut hasher = DefaultHasher::new();
    // Through `Debug` rather than `Hash`: a field added to `Server` later joins
    // the fingerprint without anyone having to remember this line.
    format!("{server:?}").hash(&mut hasher);
    user.hash(&mut hasher);
    uses_oauth2.hash(&mut hasher);
    uses_api_token.hash(&mut hasher);
    hasher.finish()
}

/// The state one collection backend keeps between a populate and the account
/// edits that follow it.
///
/// Lives inline in [`JmapCollectionBackend`](crate::backend::JmapCollectionBackend),
/// which is why all-zero has to be its starting state: GObject hands
/// `instance_init` a zeroed instance struct, and zero here reads as "nothing
/// connected, nothing proven", which is exactly true of an account no populate
/// has run for yet. Nothing to undo in `finalize` either, since neither field
/// owns anything.
///
/// Atomics rather than `Cell`s because a backend's vfuncs are not all called
/// from one thread: `populate` runs on `evolution-source-registry`'s main
/// loop, [`authenticate_sync`](crate::backend) on whichever thread EDS
/// resolved the credentials on.
#[derive(Debug, Default)]
#[repr(C)]
pub struct AccountWatch {
    /// Whether the account's `"changed"` handler is already connected.
    connected: AtomicBool,
    /// Whether a fan-out has succeeded since the last populate began.
    proven: AtomicBool,
    /// The [`login_fingerprint`] the last populate read off the account. Zero
    /// is the zeroed state, and reads as "no populate has recorded one", which
    /// makes an edit worth a try: the safe direction, and unreachable anyway,
    /// since the handler exists only once a populate has connected it.
    login: AtomicU64,
}

impl AccountWatch {
    /// Not connected, not proven: the zeroed state, spelled out.
    pub const fn new() -> Self {
        Self {
            connected: AtomicBool::new(false),
            proven: AtomicBool::new(false),
            login: AtomicU64::new(0),
        }
    }

    /// Whether this caller is the one that should connect the handler.
    ///
    /// True for the first caller and no other, which is what keeps a populate
    /// per reconnect from becoming a handler per reconnect -- and so a
    /// repopulate per handler, doubling with every pass.
    pub fn claim_connection(&self) -> bool {
        self.connected
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// A populate has begun on the account [`login_fingerprint`] answered
    /// `login` for, so its settings are unproven again until a fan-out gets
    /// through with them.
    pub fn populating(&self, login: u64) {
        self.login.store(login, Ordering::SeqCst);
        self.proven.store(false, Ordering::SeqCst);
    }

    /// A fan-out succeeded: the account names a server this code reached and
    /// credentials it accepted.
    pub fn proven(&self) {
        self.proven.store(true, Ordering::SeqCst);
    }

    /// Whether an edit that left the account with the [`login_fingerprint`]
    /// `login` is worth re-running populate for.
    pub fn wants_repopulate(&self, login: u64) -> bool {
        !self.proven.load(Ordering::SeqCst) && self.login.load(Ordering::SeqCst) != login
    }
}

#[cfg(test)]
mod tests {
    use super::AccountWatch;

    /// Two accounts' worth of login settings, as the fingerprints of them.
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
