// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The offline half of a populate: the cached children of previous sessions,
//! back in the sidebar before anything is asked of the server.
//!
//! [`crate::fan_out`] is what one *successful login* does to a collection's
//! children. This is what happens before that, and whether or not it ever
//! happens: `ECollectionBackendClass::populate` is the vfunc
//! `evolution-source-registry` schedules on an idle as soon as an account is
//! added, on every reconnect, and whenever the account changes. It returns
//! `void`, is handed nothing, and has no `GError` — so it cannot fan out and it
//! cannot report. What it can do is the two things that need no server:
//!
//! 1. **Export the children of previous sessions.** EDS loads a collection's
//!    cached `.source` files into an unclaimed table at construction time and
//!    exports none of them. Until a populate claims them and passes each to
//!    `e_source_registry_server_add_source()`, the account's address books and
//!    calendars are files on disk that Evolution cannot see. This is what makes
//!    an account work offline, and it is the whole reason `populate` runs before
//!    a password exists.
//! 2. **Ask EDS to authenticate the account**, which is what eventually produces
//!    a fan-out — see [`crate::authenticate`] for why the credentials arrive
//!    through `authenticate_sync` rather than here.
//!
//! ## Why the claimed sources are exported directly
//!
//! EDS's own `EWebDAVCollectionBackend` calls `e_collection_backend_new_child()`
//! for each claimed source before exporting it, and this module deliberately does
//! not. `e_collection_backend_claim_all_resources()` *empties* the unclaimed
//! table it draws from ("previously used sources can only be claimed once"), and
//! the claimed sources are not in the backend's `children` table yet either —
//! nothing has exported them, and that table is filled from the `child-added`
//! signal. So a `new_child()` after the claim can find neither, and mints a
//! brand-new `EServerSideSource` with a fresh uid and a fresh file name instead;
//! the WebDAV backend then exports the *claimed* source and unreferences the new
//! one, whose only remaining trace is an entry in the backend's `new_sources`
//! table. Copying that would mean creating and discarding one source per cached
//! child per populate, so what is copied instead is what
//! `claim_all_resources()`'s own documentation asks for: "export the remaining
//! instances with `e_source_registry_server_add_source()`".
//!
//! Exporting is also what pairs the child with its resource id again, which is
//! the part a `new_child()` might have looked necessary for. The export makes the
//! registry server emit `source-added`; `ECollectionBackend` listens for it,
//! recognises a source whose parent is its own collection, emits `child-added`
//! and inserts the child into its `children` table. A later
//! `new_child(resource_id)` — the fan-out's — walks that table asking
//! `dup_resource_id` about each entry, finds this child, and so reuses it instead
//! of creating a second source for the same collection.
//!
//! ## The freeze is a debt, not a lock
//!
//! `e_collection_backend_freeze_populate()` increments a counter and answers
//! whether *this* caller is the one that got the freeze — `return
//! !g_atomic_int_add (&count, 1)`. The increment happens either way, so a
//! populate that lost the race still owes a thaw, which is why EDS's own backends
//! spell the guard `if (!freeze) { thaw (); return; }` rather than as an early
//! return. Getting that wrong is invisible until it is permanent: a missing thaw
//! freezes the account's populate for the life of the process, and an extra one
//! lets two populates work on the same children at once.
//!
//! That is also why the thaw here is a destructor rather than a statement at the
//! end. A populate runs from an idle callback in `evolution-source-registry`, and
//! the panic guard in front of the vfunc keeps a Rust panic from unwinding into
//! C — but it cannot undo a freeze, so a panic between the two would silence this
//! account's populate for good.
//!
//! ## Which accounts are asked to authenticate, and how
//!
//! Two decisions, and both are about not spending a prompt on nothing:
//!
//! - **Only an account with a part this backend makes children for.** Contacts
//!   or calendars: a mail-only account has nothing for a login to discover, since
//!   this backend creates no mail children (see
//!   `jmap_collection_sync::children`). EDS's WebDAV backend gates on the same
//!   pair, for the same reason.
//! - **A password only for an account that names a user.**
//!   `e_backend_schedule_credentials_required()` is how a backend asks EDS to
//!   resolve a password — libsecret, or a prompt through Evolution — and
//!   `e_backend_schedule_authenticate()` is how it asks to be authenticated now
//!   with what it already has. [`credentials`] reads an account that names no
//!   user as anonymous *on purpose*, so asking for a password for one would put a
//!   prompt in front of someone who needs none and then drop what they typed.
//!
//! An OAuth 2.0 account is not a third condition here the way it is for EDS's
//! WebDAV backend: whichever of the two calls above a populate makes,
//! `authenticate_sync` runs afterwards, and
//! [`authenticate_with`](crate::authenticate::authenticate_with) is what reads
//! `[Authentication] Method` and fetches the access token — this module never
//! needs to know which scheme the account uses.
//!
//! ## What is not here
//!
//! Removals. [`crate::removal`] needs a [`Fanout`](jmap_collection_sync::Fanout)
//! to ask `is_obsolete`, a populate has none, and "the server did not say" is not
//! a deletion — see that module and `jmap_collection_sync::parts`.
//!
//! [`credentials`]: jmap_backend_core::connect::credentials

use eds_sys::ESource;
use gobject_sys::g_object_unref;
use jmap_collection_sync::{ChildKind, Parts};

use crate::resource_id::resource_id_of;

/// The calls on a live collection that a populate makes.
///
/// A trait for the same reason [`Collection`](crate::fan_out::Collection) is one:
/// a real `ECollectionBackend` needs a running `evolution-source-registry` on the
/// session bus, which neither this machine nor CI has, while the decisions above
/// — the order, the freeze debt, which accounts are asked to authenticate and how
/// — are testable against real `ESource`s without one. What is left untestable
/// here is one line per method.
///
/// # Safety
///
/// [`claim_all_resources`](Populating::claim_all_resources) must answer with
/// valid `ESource` pointers or NULL, each carrying a **reference of its own**
/// that the caller consumes — which is what
/// `e_collection_backend_claim_all_resources()` does (`(transfer full)`, one
/// reference per source). [`freeze`](Populating::freeze) and
/// [`thaw`](Populating::thaw) must be the matching halves of one counter, or the
/// debt this module pays back is not the one it took on.
pub unsafe trait Populating {
    /// `e_collection_backend_freeze_populate (backend)`: whether this caller got
    /// the freeze. False means another populate of this account is running — and
    /// the counter was incremented anyway, so a thaw is still owed.
    fn freeze(&self) -> bool;

    /// `e_collection_backend_thaw_populate (backend)`: gives one freeze back.
    fn thaw(&self);

    /// `E_COLLECTION_BACKEND_CLASS (parent_class)->populate (backend)`.
    ///
    /// A placeholder in EDS 3.52 — "so subclasses can safely chain up" — which
    /// is exactly why it is called: a populate that skipped it looks identical
    /// today and breaks on the release that fills the slot in.
    fn chain_up(&self);

    /// `e_collection_backend_claim_all_resources (backend)`: the cached children
    /// of previous sessions, claimed. Answers empty on every call after the
    /// first, which is EDS's contract and not an error.
    fn claim_all_resources(&self) -> Vec<*mut ESource>;

    /// `e_source_registry_server_add_source (server, child)`: exports the child
    /// over D-Bus, which is what makes it a source Evolution can see.
    ///
    /// Takes the pointer rather than a reference to it: the caller keeps the
    /// reference [`claim_all_resources`](Populating::claim_all_resources) handed
    /// over and drops it after.
    fn publish(&self, child: *mut ESource);

    /// `e_backend_schedule_credentials_required (backend,
    /// E_SOURCE_CREDENTIALS_REASON_REQUIRED, …)`: asks EDS to resolve the
    /// account's password and call `authenticate_sync` with it.
    fn request_credentials(&self);

    /// `e_backend_schedule_authenticate (backend, NULL)`: asks EDS to call
    /// `authenticate_sync` now, with no credentials, which is what an account
    /// that names no user is authenticated with.
    fn authenticate_anonymously(&self);

    /// `e_server_side_source_set_remote_creatable (account_source, offer)`:
    /// whether Evolution may offer this account as the place a new address book
    /// or calendar is created.
    ///
    /// The one thing a populate writes onto the *account* rather than onto a
    /// child, and the gate on `create_resource_sync` being reachable at all:
    /// `server_side_source_remote_create_sync()` refuses outright for a
    /// collection source that does not carry the flag, so the vfunc is dead code
    /// without it.
    ///
    /// Called on every populate with the answer for the account as it stands
    /// *now*, in both directions — see [`populate`] on why it is not a
    /// once-at-construction fact.
    fn offer_creation(&self, offer: bool);
}

/// How this populate asked to be authenticated — which is what turns a populate
/// into a fan-out, one vfunc later.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    /// Nothing: no part this backend makes children for is switched on, so
    /// there is nothing for a login to discover.
    #[default]
    Nothing,
    /// `e_backend_schedule_credentials_required`: the account names a user, so
    /// EDS resolves its password before calling back.
    Credentials,
    /// `e_backend_schedule_authenticate` with NULL: the account names no user,
    /// so there is no password to resolve and the fan-out is anonymous.
    Anonymously,
}

/// What one populate did, and everything about it that is worth a log line.
///
/// A populate returns `void`, so this is a report and not a result: there is
/// nobody to raise anything to, and the recovery available is the next populate
/// finding the same state.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Restored {
    /// The resource ids of the cached children exported again, in the order they
    /// were claimed.
    pub children: Vec<String>,
    /// How many claimed sources were dropped unexported because this backend
    /// could not name them. Unreachable through EDS — see [`populate`] — and a
    /// count rather than a list because there is no name to put in one.
    pub unidentified: usize,
    /// How the account was asked to authenticate, if it was.
    pub asked: Asked,
    /// Whether this account was left one Evolution may create an address book or
    /// a calendar in — see [`Populating::offer_creation`].
    pub creatable: bool,
}

/// `ECollectionBackendClass::populate` for a JMAP collection, minus the
/// instance.
///
/// `parts` and `user` are what the account itself says, read by
/// [`parts_of`](crate::collection_source::parts_of) and
/// [`user_of`](crate::collection_source::user_of) — everything a populate knows,
/// since it is handed nothing else and cannot contact anything.
///
/// `None` is a populate that lost the freeze to another one and did nothing but
/// give it back.
///
/// A claimed source whose resource id this backend cannot read is dropped
/// unexported and counted. It cannot arrive through EDS — EDS only caches a
/// source `dup_resource_id` answered for in the first place — but the answer
/// matters, because exporting it would put a child in the sidebar that no
/// resource id can be paired with again: `e_collection_backend_new_child()`
/// finds an existing child by asking `dup_resource_id` about each one, so a child
/// that answers `None` is one every later populate recreates rather than reuses.
/// That is [`adopt`](crate::fan_out::adopt)'s rule — nothing that cannot be
/// written in full is exported — reached from the reading side.
///
/// # Safety
///
/// `collection`'s methods must satisfy [`Populating`]'s contract.
pub unsafe fn populate<P: Populating + ?Sized>(
    collection: &P,
    parts: Parts,
    user: Option<&str>,
) -> Option<Restored> {
    let held = collection.freeze();
    // Constructed whatever the answer was, and before the early return below:
    // the freeze incremented the counter either way, so the debt exists either
    // way. A destructor rather than a call at the end of the function so that a
    // panic in the middle does not freeze this account's populate for good.
    let _frozen = Frozen(collection);
    if !held {
        return None;
    }

    collection.chain_up();

    let mut report = Restored::default();
    for source in collection.claim_all_resources() {
        if source.is_null() {
            continue;
        }

        // SAFETY: a non-NULL source EDS loaded from this collection's cache
        // directory, alive for as long as this scope's reference to it.
        match unsafe { resource_id_of(source) } {
            Some(resource_id) => {
                collection.publish(source);
                report.children.push(resource_id);
            }
            None => report.unidentified += 1,
        }

        // The reference the claim transferred. Dropped whatever happened: an
        // exported child is held by the registry server, and an unexported one
        // is held by nothing and should be.
        // SAFETY: the reference `claim_all_resources` handed over, not used
        // again.
        unsafe { g_object_unref(source.cast()) };
    }

    // Whether this account is one Evolution may create a collection in, and the
    // same condition that decides whether it is worth authenticating: an account
    // with neither contacts nor calendars switched on has no children of this
    // backend's at all, so offering to create one would offer a source the very
    // next populate treats as dormant.
    //
    // Written on every populate rather than once, and in both directions,
    // because the condition is a *setting* the user can change: an account whose
    // owner switches contacts back on has to become creatable again without
    // being removed and re-added. EDS's setter early-returns when the value is
    // unchanged, so the repetition costs nothing.
    report.creatable = parts.wants(ChildKind::AddressBook) || parts.wants(ChildKind::Calendar);
    collection.offer_creation(report.creatable);

    // Last, because the cached children have to be in the sidebar before a login
    // that may never succeed — see the module comment on the order.
    report.asked = if report.creatable {
        match user {
            Some(_) => {
                collection.request_credentials();
                Asked::Credentials
            }
            None => {
                collection.authenticate_anonymously();
                Asked::Anonymously
            }
        }
    } else {
        Asked::Nothing
    };

    Some(report)
}

/// One freeze, owed back.
struct Frozen<'a, P: Populating + ?Sized>(&'a P);

impl<P: Populating + ?Sized> Drop for Frozen<'_, P> {
    fn drop(&mut self) {
        self.0.thaw();
    }
}
