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
//! [`crate::store::JmapStore::messages`] or [`crate::store::JmapStore::messages_since`]. That is also why
//! the disconnected case reports
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
//! ## Ask what changed, not what is there
//!
//! A refresh is a poll: Camel runs one when the folder is opened and again on a
//! timer for as long as it stays open, and nearly every one of them finds a
//! mailbox nobody has touched. Listing answers that at the price of the whole
//! mailbox — one query and one `Email/get` per page of rows the folder already
//! has — so what this vfunc asks first is [`crate::store::JmapStore::messages_since`], which
//! is one `Email/changes` and, for the usual answer, nothing else at all.
//!
//! The state to ask from is the one the last refresh recorded, and it lives in
//! the summary rather than here because it is a fact about the rows; a folder
//! that has none has never listed this mailbox, and lists. `MessageUpdate`'s
//! three answers are then dispatched below, and which of the two application
//! paths a delta and a listing take is the distinction [`crate::summary`]
//! exists to keep: silence in a listing means a message has left the mailbox,
//! and silence in a delta means nothing was said about it.
//!
//! It is also what fills the `recent` list, which every refresh before this one
//! left empty. Only a delta can honestly say a message *arrived* — it is asked
//! from a state this folder recorded, so a message it names that the folder has
//! no row for reached the mailbox since then — and [`crate::changes`] documents
//! why that matters more than it looks: recent is what runs the user's incoming
//! filters.
//!
//! ## How far behind is too far
//!
//! A folder that has been closed for a fortnight asks a fortnight-old question,
//! and `Email/changes` answers it for the whole *account*: every message anyone
//! touched anywhere, each one an id that has to be fetched before this mailbox
//! can say whether it holds it. Past some size the listing this was avoiding is
//! the cheaper answer, and the size is the mailbox's own — which is why the
//! summary's row count is passed down with the state it was current at. The
//! judgement itself is `jmap-mail-sync`'s, in `catch_up_limit`.
//!
//! ## Stopping one
//!
//! A listing of a large mailbox is many round trips where a folder list is one
//! or two, so this is the vfunc whose `cancellable` matters most, and it is
//! [`observe`]d for the length of the call: the user's Stop lands between two
//! requests and the rest are never made. What has already been written to the
//! summary stays written, and the state is only recorded for an update that
//! completed — a refresh stopped half way is a refresh that did not happen, and
//! the next one asks the same question again.

use eds_sys::{
    CamelFolder, CamelFolderClass, CamelFolderSummary, camel_folder_changed,
    camel_folder_get_folder_summary, camel_folder_get_full_name,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GTRUE, gboolean};
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::fail_bool;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::trampoline::guard_bool;
use jmap_mail_sync::MessageUpdate;
use jmap_proto::Id;

use crate::changes::Changes;
use crate::connect::StoreError;
use crate::folder::{JmapFolder, parent_store};
use crate::summary::{apply_delta, apply_listing, set_summary_state, summary_rows, summary_state};

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

/// Asks what the mailbox has done since the folder last looked, and brings the
/// folder in line with the answer — listing it in full when there is no such
/// "since" to ask from.
///
/// `TRUE` for a refresh that happened, `FALSE` with the error set for one that
/// could not — which is Camel's convention and, in particular, is what
/// `camel_folder_refresh_info_sync`'s callers test before they believe the
/// folder.
unsafe extern "C" fn refresh_info_sync(
    folder: *mut CamelFolder,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, and an
    // out-parameter that is NULL or writable and currently NULL.
    unsafe {
        guard_bool("refresh_info_sync", error, || {
            // SAFETY: Camel keeps its cancellable alive for the length of the
            // call, so it outlives this observation — which is what makes
            // every request below here stop when the user presses Stop.
            let _cancel = observe(cancellable);

            let Some((mailbox, summary)) = target(folder) else {
                return fail_bool(
                    error,
                    &StoreError::NoFolder(name_of(folder)),
                    StoreError::to_gerror,
                );
            };
            let Some(store) = parent_store(folder) else {
                return fail_bool(error, &StoreError::Disconnected, StoreError::to_gerror);
            };

            // The question this folder is in a position to ask. A summary that
            // remembers a state can ask what changed since it; one that does
            // not — a mailbox never refreshed, or one whose header was written
            // by a version of this provider that kept no state — has nothing
            // to ask from and lists, which is the same answer phrased as the
            // update the two paths share.
            //
            // The row count goes with the state because the two are one
            // question — what this folder holds, and when it was true — and
            // because the folder is the only side that has the count without
            // asking for it.
            let update = match summary_state(summary) {
                Some(since) => store.messages_since(mailbox, &since, summary_rows(summary)),
                None => store
                    .messages(mailbox)
                    .map(|(state, messages)| MessageUpdate::Relisted { state, messages }),
            };
            let update = match update {
                Ok(update) => update,
                Err(failure) => return fail_bool(error, &failure, StoreError::to_gerror),
            };

            let (state, changes) = match update {
                // Nearly every poll, and the whole point of asking: the folder
                // is already right, one round trip said so, and the only thing
                // to record is that it is right as of a newer state.
                MessageUpdate::Unchanged(state) => (state, Changes::new()),
                // A delta says what this mailbox holds for the messages that
                // moved and what it no longer holds; every row neither list
                // names is left exactly where it is. Handing this to
                // `apply_listing` would empty the folder — see
                // [`crate::summary`].
                MessageUpdate::Changed {
                    state,
                    present,
                    absent,
                } => (state, apply_delta(summary, &present, &absent)),
                // And the whole mailbox, when the server would not calculate a
                // delta from the state this folder had. Reconciled rather than
                // applied: a listing is the mailbox, so a row it does not name
                // is a message that has left.
                MessageUpdate::Relisted { state, messages } => {
                    (state, apply_listing(summary, &messages))
                }
            };
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
