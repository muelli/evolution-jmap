// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `get_quota_info_sync`: what Evolution's folder-properties dialog reads to
//! show a "Quota usage" row.
//!
//! RFC 9425's `Quota` objects (`jmap-proto`'s [`Quota`], `Client::quotas` in
//! `jmap-client`) have been modeled, mocked and capability-advertised since
//! the previous increment; this is the vfunc that puts them where Evolution
//! looks. `CamelFolderClass::get_quota_info_sync` is per-folder in Camel's own
//! vocabulary — IMAPX answers it with the IMAP quota root a mailbox happens to
//! sit under — but RFC 9425 scopes a `Quota` to the account, not to a mailbox
//! (§2.2's `scope` is `account`, `domain` or `global`, never one folder), so
//! every folder of a JMAP account answers this vfunc from the same list:
//! [`crate::store::JmapStore::quotas`].
//!
//! ## One list, one filter
//!
//! An account's `Quota/get` answers with every quota the server tracks —
//! Contacts and Calendars among them, on a server that also hosts CardDAV and
//! CalDAV data for the same user — and a folder-properties dialog asking about
//! a mailbox has no use for a quota that says nothing about Mail. RFC 9425
//! §2.3 gives the rule for which is which: a `dataTypes` naming `"Mail"`
//! applies, and so — this is the part easy to miss — does one that is absent
//! or empty, because an empty `dataTypes` means "every data type this account
//! has", not "none". [`applies_to_mail`] is that rule, kept as a plain
//! function over [`Quota`] so it can be tested without a `CamelFolder` to call
//! the vfunc on.
//!
//! ## The chain, and the empty case
//!
//! `CamelFolderQuotaInfo` is a linked list rather than an array — one node per
//! quota, `used`/`total` and a `next` Camel walks until it is NULL — built by
//! [`chain`] straight out of the filtered list. An account with nothing that
//! applies to Mail is not a failure of the connection or the account, but it
//! is not silence either: `get_quota_info_sync`'s own contract is NULL *with*
//! the error set for "nothing to report", the same one IMAPX's own
//! implementation reaches for when a mailbox has no quota root
//! (`camel-imapx-folder.c`) — [`crate::connect::StoreError::NoQuota`],
//! `G_IO_ERROR_NOT_SUPPORTED`.

use std::ptr;

use eds_sys::{CamelFolder, CamelFolderClass, CamelFolderQuotaInfo, camel_folder_quota_info_new};
use gio_sys::GCancellable;
use glib_sys::GError;
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::{cstring_lossy, fail};
use jmap_backend_core::trampoline::guard_ptr;
use jmap_proto::quota::{Quota, quota_data_type};

use crate::connect::StoreError;
use crate::folder::parent_store;

/// Installs the vfunc on a class whose first member is a `CamelFolderClass`.
///
/// # Safety
///
/// `class` must point at an initialised class struct that leads with a
/// `CamelFolderClass` — which is every descendant of `CamelFolder`.
pub unsafe fn install_vfuncs(class: *mut CamelFolderClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.get_quota_info_sync = Some(get_quota_info_sync);
}

/// Answers what the account's Mail quota looks like, account-wide — see the
/// module docs for why one folder's answer is every folder's answer.
///
/// A new chain on success, NULL with the error set on failure — Camel's
/// convention for the object-returning vfuncs, and what
/// `camel_folder_get_quota_info_sync`'s callers test before they render
/// anything.
unsafe extern "C" fn get_quota_info_sync(
    folder: *mut CamelFolder,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> *mut CamelFolderQuotaInfo {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, and an
    // out-parameter that is NULL or writable and currently NULL.
    unsafe {
        guard_ptr("get_quota_info_sync", error, || {
            // SAFETY: Camel keeps its cancellable alive for the length of the
            // call, so it outlives this observation — which is what makes the
            // one request below stop when the user presses Stop.
            let _cancel = observe(cancellable);

            let Some(store) = parent_store(folder) else {
                return fail(error, &StoreError::Disconnected, StoreError::to_gerror);
            };

            let quotas = match store.quotas() {
                Ok(quotas) => quotas,
                Err(failure) => return fail(error, &failure, StoreError::to_gerror),
            };

            let mail_quotas: Vec<Quota> = quotas.into_iter().filter(applies_to_mail).collect();

            match chain(&mail_quotas) {
                Some(head) => head,
                None => fail(error, &StoreError::NoQuota, StoreError::to_gerror),
            }
        })
    }
}

/// Whether a `Quota` describes Mail usage, per RFC 9425 §2.3: an absent or
/// empty `dataTypes` applies to every data type the account has, not to none
/// of them, so it is a match here as much as one that names `"Mail"` is.
fn applies_to_mail(quota: &Quota) -> bool {
    quota.data_types.is_empty()
        || quota
            .data_types
            .iter()
            .any(|data_type| data_type == quota_data_type::MAIL)
}

/// Builds the linked list the vfunc answers with, one node per quota in
/// order. `None` for an empty slice rather than a chain of nothing: a NULL
/// return has to be told apart from "this is genuinely empty", because a
/// vfunc that answered NULL without setting the error would be lying about
/// what happened.
fn chain(quotas: &[Quota]) -> Option<*mut CamelFolderQuotaInfo> {
    let mut head: *mut CamelFolderQuotaInfo = ptr::null_mut();
    let mut tail: *mut CamelFolderQuotaInfo = ptr::null_mut();
    for quota in quotas {
        let name = cstring_lossy(&quota.name);
        // SAFETY: `name` is NUL-terminated and alive for the call, which
        // copies it into the node it allocates; `used` and `limit` are plain
        // integers.
        let node = unsafe { camel_folder_quota_info_new(name.as_ptr(), quota.used, quota.limit) };
        if head.is_null() {
            head = node;
        } else {
            // SAFETY: `tail` is the previous iteration's `node`, a live
            // allocation this function has not handed to any other owner yet.
            unsafe { (*tail).next = node };
        }
        tail = node;
    }
    (!head.is_null()).then_some(head)
}

#[cfg(test)]
mod tests {
    use jmap_proto::quota::{quota_resource_type, quota_scope};

    use super::*;

    fn octets(name: &str, data_types: impl IntoIterator<Item = &'static str>) -> Quota {
        Quota::new(
            "Q1",
            name,
            quota_resource_type::OCTETS,
            0,
            1_073_741_824,
            quota_scope::ACCOUNT,
            data_types,
        )
    }

    #[test]
    fn a_quota_naming_mail_applies() {
        assert!(applies_to_mail(&octets("Storage", [quota_data_type::MAIL])));
    }

    #[test]
    fn a_quota_with_no_data_types_applies_to_everything_including_mail() {
        assert!(applies_to_mail(&octets("Storage", [])));
    }

    #[test]
    fn a_quota_scoped_to_other_data_types_does_not_apply() {
        assert!(!applies_to_mail(&octets(
            "Storage",
            [quota_data_type::CONTACTS, quota_data_type::CALENDARS]
        )));
    }

    #[test]
    fn an_empty_list_chains_to_nothing() {
        assert!(chain(&[]).is_none());
    }
}
