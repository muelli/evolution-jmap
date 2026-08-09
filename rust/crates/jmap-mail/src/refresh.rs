// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `refresh_info_sync`: the vfunc where a folder and the server meet.
//!
//! Four increments have built the two ends of one path without joining them.
//! `jmap-mail-sync` lists a mailbox and knows nothing of Camel;
//! [`crate::message_info`] turns one row of that listing into a
//! `CamelMessageInfo`; [`crate::summary`] reconciles a whole listing against
//! the rows a folder already holds. All three have had to be handed a listing
//! by a test. This is the vfunc Camel calls, and it is where the listing comes
//! from a server.
//!
//! ## Why the folder asks the store
//!
//! A `CamelFolder` has no connection. What it has is the JMAP mailbox id
//! [`crate::folder`] put on it and `camel_folder_get_parent_store`, and the
//! store is where [`crate::service`] left the client — so a refresh is the
//! folder's mailbox id asked of the store's connection, which is
//! [`JmapStore::messages`]. That is also why the disconnected case reports
//! `CAMEL_SERVICE_ERROR_NOT_CONNECTED` rather than anything about the folder:
//! nothing is wrong with the folder, and that code is what makes Camel connect
//! and ask again instead of showing the account as broken.
//!
//! ## Two answers, not one
//!
//! Rewriting the summary brings a folder that is *about to be opened* up to
//! date. A folder that is already open — Evolution's message list, drawn once
//! and kept — is brought up to date by the `changed` signal and by nothing
//! else, so the second half of this vfunc is emitting it. Emitting it only when
//! there is something to say is not an optimisation: Camel polls a folder on a
//! timer, and a folder that announced a change every time would move the list
//! under the user while they read it.
//!
//! ## What is not here yet
//!
//! The whole mailbox is still listed on every refresh, and every piece of not
//! doing so now exists: `jmap-mail-sync`'s `messages_since` turns one
//! `Email/changes` into the rows this mailbox holds and the uids it does not,
//! [`crate::summary`] keeps the state such a question is asked from across a
//! restart — which is what this vfunc records below — and
//! [`crate::summary::apply_delta`] applies that answer to a summary without
//! `apply_listing`'s rule that anything unnamed has left the mailbox. What is
//! missing is only the join: this vfunc still asks [`JmapStore::messages`] for
//! the whole mailbox rather than asking for a delta from the state it recorded
//! last time, and choosing between the two paths by which of
//! `jmap_mail_sync::MessageUpdate`'s three answers came back. That is also what
//! the `recent` list [`crate::changes`] leaves empty here is waiting for, since
//! only the delta path may fill it. Listing meanwhile is correct; it is only
//! expensive.
//!
//! `cancellable` is not observed, the same gap [`crate::folders`] documents and
//! for the same reason: [`Client`] takes its [`CancelFlag`] when it is built.
//! It matters more here than there — a listing of a large mailbox is many round
//! trips where a folder list is one or two — which is why it is named again
//! rather than assumed to be understood.
//!
//! [`Client`]: jmap_client::Client
//! [`CancelFlag`]: jmap_client::transport::CancelFlag

use eds_sys::{
    CamelFolder, CamelFolderClass, CamelFolderSummary, camel_folder_changed,
    camel_folder_get_folder_summary, camel_folder_get_full_name,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, GTRUE, gboolean};
use jmap_backend_core::error::set_raw_gerror;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::trampoline::guard_bool;
use jmap_proto::Id;

use crate::connect::StoreError;
use crate::folder::{JmapFolder, parent_store};
use crate::summary::{apply_listing, set_summary_state};

/// Installs the folder's own vfuncs on a class whose first member is a
/// `CamelFolderClass`.
///
/// # Safety
///
/// `class` must point at an initialised class struct that leads with a
/// `CamelFolderClass` — which is every descendant of `CamelFolder`.
pub unsafe fn install_vfuncs(class: *mut CamelFolderClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.refresh_info_sync = Some(refresh_info_sync);
}

/// Lists the mailbox and brings the folder in line with what it found.
///
/// `TRUE` for a refresh that happened, `FALSE` with the error set for one that
/// could not — which is Camel's convention and, in particular, is what
/// `camel_folder_refresh_info_sync`'s callers test before they believe the
/// folder.
unsafe extern "C" fn refresh_info_sync(
    folder: *mut CamelFolder,
    _cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, and an
    // out-parameter that is NULL or writable and currently NULL.
    unsafe {
        guard_bool("refresh_info_sync", error, || {
            let Some((mailbox, summary)) = target(folder) else {
                return fail(error, &StoreError::NoFolder(name_of(folder)));
            };
            let Some(store) = parent_store(folder) else {
                return fail(error, &StoreError::Disconnected);
            };

            let (state, messages) = match store.messages(mailbox) {
                Ok(listing) => listing,
                Err(failure) => return fail(error, &failure),
            };

            let changes = apply_listing(summary, &messages);
            // Recorded after the rows and not before them, because what the
            // state says is what the rows are current as of: a summary that
            // claimed one it had not applied yet would, if the process died in
            // between, come back holding the older rows and asking for changes
            // since the newer state — and never hear about the ones in between.
            set_summary_state(summary, state);
            if !changes.is_empty() {
                camel_folder_changed(folder, changes.as_ptr());
            }
            GTRUE
        })
    }
}

/// The two things a refresh writes to: the mailbox it lists, and the summary it
/// lists into.
///
/// Both come from [`crate::folder::new_folder`] and neither can be absent on a
/// folder it built, so `None` means a `CamelJmapFolder` that something else
/// constructed — a `g_object_new` on the type, which is not how a folder is
/// meant to arrive. Reported rather than asserted, because a vfunc is not the
/// place to take the process down.
///
/// # Safety
///
/// `folder` must be NULL or point at a live `JmapFolder`.
unsafe fn target<'a>(folder: *mut CamelFolder) -> Option<(&'a Id, *mut CamelFolderSummary)> {
    // SAFETY: the contract above, and the summary accessor borrows what the
    // folder owns for as long as the folder lives.
    unsafe {
        let mailbox = JmapFolder::borrow(folder)?.mailbox()?;
        let summary = camel_folder_get_folder_summary(folder);
        (!summary.is_null()).then_some((mailbox, summary))
    }
}

/// The path Camel keys the folder by, for an error message about it.
///
/// # Safety
///
/// `folder` must point at a live `CamelFolder`.
unsafe fn name_of(folder: *mut CamelFolder) -> String {
    // SAFETY: the accessor returns a string the folder owns and outlives the
    // call; `read_string` copies it.
    unsafe { read_string(camel_folder_get_full_name(folder)).unwrap_or_default() }
}

/// Reports a failure and answers with it.
///
/// # Safety
///
/// As [`set_raw_gerror`].
unsafe fn fail(error: *mut *mut GError, failure: &StoreError) -> gboolean {
    // SAFETY: `to_gerror` hands over an owned GError, and `error` meets
    // `set_raw_gerror`'s contract by this function's.
    unsafe { set_raw_gerror(error, failure.to_gerror()) };
    GFALSE
}
