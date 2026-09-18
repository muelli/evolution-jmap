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
//! and a `"changed"` only repopulates while it is unproven. So the edit that
//! fixes a broken account gets its retry, and every later edit is free.
//!
//! The cost is the mirror case: breaking a *working* account's host is not
//! noticed until something else reconnects it. That is what EWS settles for
//! too, and it is not a regression either way — before this module there was
//! no handler at all, so no edit of any kind was noticed.
//!
//! ## Why this cannot chase its own tail
//!
//! A repopulate that emitted `"changed"` on the account would run forever.
//! `ESource::changed` is emitted from `source_notify` (`e-source.c`) for
//! properties carrying `E_SOURCE_PARAM_SETTING`, and nothing on a populate's
//! path writes one: `e_server_side_source_set_remote_creatable`'s
//! `remote-creatable` and `ESource`'s `connection-status` are both plain
//! `G_PARAM_READABLE`/`READWRITE` without that flag, the fan-out writes
//! children and never the account, and EDS's own `child_added` bindings run
//! collection-to-child only. `e_source_changed` also coalesces a burst of
//! edits into a single idle emission, so an account editor's *Apply* arrives
//! as one `"changed"` rather than one per field.

use std::sync::atomic::{AtomicBool, Ordering};

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
}

impl AccountWatch {
    /// Not connected, not proven: the zeroed state, spelled out.
    pub const fn new() -> Self {
        Self {
            connected: AtomicBool::new(false),
            proven: AtomicBool::new(false),
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

    /// A populate has begun, so this account's settings are unproven again
    /// until a fan-out gets through with them.
    pub fn populating(&self) {
        self.proven.store(false, Ordering::SeqCst);
    }

    /// A fan-out succeeded: the account names a server this code reached and
    /// credentials it accepted.
    pub fn proven(&self) {
        self.proven.store(true, Ordering::SeqCst);
    }

    /// Whether an edit of the account is worth re-running populate for.
    pub fn wants_repopulate(&self) -> bool {
        !self.proven.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::AccountWatch;

    #[test]
    fn only_the_first_caller_connects() {
        let watch = AccountWatch::new();
        assert!(watch.claim_connection());
        assert!(!watch.claim_connection());
        assert!(!watch.claim_connection());
    }

    #[test]
    fn an_account_that_never_got_through_is_worth_another_try() {
        let watch = AccountWatch::new();
        assert!(watch.wants_repopulate());

        watch.populating();
        assert!(
            watch.wants_repopulate(),
            "a populate that has not fanned out yet leaves the account unproven"
        );
    }

    #[test]
    fn an_edit_after_a_working_login_asks_for_nothing() {
        let watch = AccountWatch::new();
        watch.populating();
        watch.proven();

        assert!(
            !watch.wants_repopulate(),
            "renaming a working account must not cost it a fresh fan-out"
        );
    }

    #[test]
    fn a_reconnect_makes_the_account_unproven_again() {
        let watch = AccountWatch::new();
        watch.proven();
        watch.populating();

        assert!(
            watch.wants_repopulate(),
            "the fan-out that proved the account was for the previous populate"
        );
    }

    #[test]
    fn the_connection_survives_a_repopulate() {
        let watch = AccountWatch::new();
        assert!(watch.claim_connection());
        watch.proven();
        watch.populating();

        assert!(
            !watch.claim_connection(),
            "a populate re-run by the handler must not connect a second one"
        );
    }
}
