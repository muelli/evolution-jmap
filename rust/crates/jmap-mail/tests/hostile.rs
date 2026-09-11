// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The claims this crate's instance structs rest on that the compiler is never
//! shown.
//!
//! `jmap-backend-book` and `jmap-backend-cal` each carry one of these — audit
//! finding F7, which pinned the threading claim behind `Slot` as a compile
//! error rather than a comment. The mail provider grew six instance structs
//! since and had none, which is `docs/AUDIT-FFI-20260810.md`'s F13.
//!
//! Camel is a harder case than EDS, not an easier one: a `CamelStore` is driven
//! from more threads than a meta backend, and unlike the two backends this crate
//! also carries a hand-written `unsafe impl Send`/`Sync`, whose justification is
//! only true while nothing else can reach the pointer it guards.

use std::sync::{Arc, Mutex, RwLock};

use jmap_backend_core::instance::Slot;
use jmap_backend_core::push::PushRefresh;
use jmap_mail::cache::MessageCache;
use jmap_mail::push::FolderRefresh;
use jmap_mail_sync::{FolderTree, Keywords, MailSync};
use jmap_proto::{Id, State};

/// Everything a `Slot` in this crate holds, and everything inside it.
///
/// The instance arrives at a vfunc as a raw pointer and is turned into a `&` by
/// hand, so the compiler never sees the sharing: nothing would object if
/// `MailSync`, the `Client` inside it or the boxed `Transport` inside that grew
/// an `Rc` or a `RefCell`, and the result would be a data race in the process
/// serving every other mail account the user has.
///
/// One test over every field rather than one per struct, because they are one
/// claim. It has to name *all* of them to be that claim: between the
/// 2026-09-01 audit and this one the crate grew four more `Slot`s (the store's
/// push subscription and folder-refresh worker, the row's server thread id,
/// and an `Arc` around the store's own connection) while this list stood
/// still, so it asserted five of the ten the crate declares. See
/// `docs/AUDIT-FFI-20260911.md`, F24.
#[test]
fn everything_an_instance_holds_is_shareable_across_threads() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<MailSync>();
    assert_send_sync::<jmap_client::Client>();

    // `crate::transport`: the connection a send goes out over.
    assert_send_sync::<Slot<RwLock<Option<MailSync>>>>();
    // `crate::store`: the connection (behind an `Arc`, unlike the
    // transport's), the listing a refresh renews, the push subscription and
    // the coalescing folder-refresh worker. `Listing` is private to the
    // crate, so its two fields stand for it here.
    assert_send_sync::<Slot<RwLock<Option<Arc<MailSync>>>>>();
    assert_send_sync::<State>();
    assert_send_sync::<Arc<FolderTree>>();
    assert_send_sync::<Slot<Mutex<Option<PushRefresh>>>>();
    assert_send_sync::<Slot<Mutex<Option<FolderRefresh>>>>();
    // `crate::folder`: the mailbox this folder stands for, and the account's
    // message cache the folder is the owner of.
    assert_send_sync::<Slot<Id>>();
    assert_send_sync::<Slot<MessageCache>>();
    // `crate::summary`: the `Email` state the rows are current as of.
    assert_send_sync::<Slot<Mutex<Option<State>>>>();
    // `crate::message_info`: the keywords the last listing found, and the
    // thread id the server assigned the row.
    assert_send_sync::<Slot<Mutex<Keywords>>>();
    assert_send_sync::<Slot<Mutex<Option<Id>>>>();
}

/// The one hand-written `unsafe impl` in this crate, asserted where it is
/// claimed.
///
/// [`MessageCache`] wraps a `*mut CamelDataCache` — not `Send` or `Sync` on its
/// own — behind a `Mutex`, and says so in an `unsafe impl` of both. That is
/// sound only while the pointer never leaves the lock, which is a property of
/// the module rather than of the type, and there is nothing but this line to
/// notice an accessor that started handing it out.
#[test]
fn the_message_cache_still_claims_to_be_shareable() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<MessageCache>();
}
