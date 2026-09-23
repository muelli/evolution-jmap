// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelFolderClass::search_body_sync`, from EDS 3.58: the words of a
//! body-contains term, answered by the server.
//!
//! [`crate::search_sexp`] already turns a whole Camel search expression into an
//! `Email/query` filter, and up to 3.52 that is the only server-side search
//! this provider has: [`crate::folder`] fills `search_by_expression` and
//! `search_by_uids` with it. From 3.58 those two slots are gone and the base
//! class evaluates an expression itself through `CamelStoreSearch`, over the
//! store's own database — which can answer a header or a flag out of the
//! summary rows, but not a body, because a body this provider never downloaded
//! is not in them.
//!
//! So 3.58 leaves the provider one slot for exactly that case. When
//! `CamelStoreSearch` reaches a body term whose answer is not already in the
//! message preview, it calls this vfunc with the term's words
//! (`camel-store-search.c`, `_camel_store_search_check_body_match`), and what
//! it does with the answer is the whole reason this is worth filling in:
//!
//! - Answered (TRUE): the uid set we hand back is authoritative for this
//!   folder. Every other message is decided against without being looked at.
//! - Refused (FALSE): the term is marked failed for the folder, and the search
//!   falls back to opening *every* message in it and scanning the body
//!   locally.
//!
//! The fallback is correct rather than broken, which corrects the premise this
//! item was queued under ("silently finds nothing"): what an unfilled slot
//! costs is a folder-sized download per body search, plus the messages that
//! are not cached and cannot be fetched, which do silently fail to match.
//! Answering from `Email/query` is one round trip instead.
//!
//! ## Which words match
//!
//! Camel means each word as a case-insensitive substring
//! (`CAMEL_SEARCH_MATCH_CONTAINS`) and a message matches when it contains all
//! of them; RFC 8621 §4.4.1 leaves `body` matching to the server, which in
//! practice tokenises. A server therefore answers a partial word where Camel
//! would have matched it. That difference is not new here: it is the same one
//! `search_sexp`'s `body-contains` translation has taken since it shipped, and
//! the same one IMAPX takes handing `BODY` to a server, so this slot keeps the
//! provider consistent with itself rather than inventing a second policy.
//! Multiple words become an AND of one condition each, not one condition of
//! several words, because a multi-word string is one more thing RFC 8621 lets
//! a server interpret.

use eds_sys::{CamelFolder, camel_pstring_free, camel_pstring_strdup};
use gio_sys::GCancellable;
use glib_sys::{
    GError, GPtrArray, GTRUE, g_ptr_array_add, g_ptr_array_new_with_free_func, gboolean, gpointer,
};
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::{fail_bool, fail_invalid};
use jmap_backend_core::trampoline::guard_bool;
use jmap_proto::mail::EmailQueryFilter;
use jmap_proto::methods::Filter;

use crate::connect::StoreError;
use crate::folder::{JmapFolder, parent_store, string_array};
use crate::folder_info::c_string;

/// Installs the vfunc on a class whose first member is a `CamelFolderClass`.
///
/// Only from EDS 3.58, where the slot exists at all. Nothing stands in for it
/// on an older EDS, and nothing needs to: there the same body term reaches the
/// server through `search_by_expression`, which 3.58 is the release that took
/// away.
///
/// # Safety
///
/// As [`crate::cache::install_vfuncs`].
#[cfg(camel_folder_search_body_sync)]
pub unsafe fn install_vfuncs(class: *mut eds_sys::CamelFolderClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.search_body_sync = Some(search_body_sync);
}

/// The JMAP filter for a body term's `words`, or `None` if there is nothing to
/// ask: a message matches when its body contains every one of them.
///
/// An empty word is dropped rather than asked about. Camel's own
/// substring match treats one as matching everything, so dropping it cannot
/// narrow the answer, while asking a server for it would be a condition whose
/// meaning RFC 8621 does not fix. A term of nothing but empty words is
/// therefore `None`, which the vfunc reports as "cannot search" rather than as
/// "matches everything".
pub(crate) fn body_filter(words: &[String]) -> Option<Filter<EmailQueryFilter>> {
    let mut conditions: Vec<Filter<EmailQueryFilter>> = words
        .iter()
        .filter(|word| !word.is_empty())
        .map(|word| Filter::condition(EmailQueryFilter::default().body(word.as_str())))
        .collect();
    match conditions.len() {
        0 => None,
        1 => conditions.pop(),
        _ => Some(Filter::and(conditions)),
    }
}

/// `CamelFolderClass.search_body_sync`: which of this folder's messages have
/// `words` in their body, as the server answers it.
///
/// TRUE with `out_uids` set — possibly to an empty array, which means "none",
/// not "could not tell" — or FALSE with the error set, which
/// `CamelStoreSearch` reads as "ask the messages themselves instead". Nothing
/// to ask about, a folder with no mailbox id yet, and a store with no
/// connection are all refusals rather than empty answers: the second and third
/// are the offline case, where the messages already on disk are the better
/// source, and an empty array there would be a wrong answer stated
/// confidently.
///
/// Compiled on every EDS although only 3.58 and newer have a slot to put it
/// in: it names no type older headers lack, so keeping it out of the `#[cfg]`
/// is what lets this runner's own tests drive it.
///
/// # Safety
///
/// Camel's contract for the vfunc: `folder` a live instance of ours, `words`
/// NULL or a live `GPtrArray` of NUL-terminated strings, `out_uids` a valid
/// place to write one array pointer.
pub unsafe extern "C" fn search_body_sync(
    folder: *mut CamelFolder,
    words: *mut GPtrArray,
    out_uids: *mut *mut GPtrArray,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: the contract above.
    unsafe {
        guard_bool("search_body_sync", error, || {
            let _cancel = observe(cancellable);

            if out_uids.is_null() {
                return fail_invalid(error, "search_body_sync was given nowhere to answer");
            }
            let words = if words.is_null() {
                Vec::new()
            } else {
                string_array(words)
            };
            let Some(filter) = body_filter(&words) else {
                return fail_invalid(error, "a body search of no words is not supported");
            };

            let Some(mailbox) = JmapFolder::borrow(folder).and_then(|f| f.mailbox().cloned())
            else {
                return fail_bool(error, &StoreError::Disconnected, StoreError::to_gerror);
            };
            let Some(store) = parent_store(folder) else {
                return fail_bool(error, &StoreError::Disconnected, StoreError::to_gerror);
            };

            match store.search(&mailbox, filter) {
                Ok(ids) => {
                    let uids: Vec<&str> = ids.iter().map(jmap_proto::Id::as_str).collect();
                    *out_uids = pooled_uid_array(&uids);
                    GTRUE
                }
                Err(failure) => fail_bool(error, &failure, StoreError::to_gerror),
            }
        })
    }
}

/// The `GPtrArray` this vfunc answers with: pooled strings, and a free
/// function for them.
///
/// Unlike the array [`crate::folder`]'s older search vfuncs return, which the
/// base class frees element by element through `camel_folder_search_free`,
/// this one is only ever `g_ptr_array_unref`'d by its caller — so the elements
/// leak unless the array frees them itself. `camel_pstring_strdup` paired with
/// `camel_pstring_free` is the pairing both the base implementation and IMAPX
/// build this particular array with.
fn pooled_uid_array(uids: &[&str]) -> *mut GPtrArray {
    // SAFETY: a fresh array, and every pointer put in it is one the free
    // function it was created with can free.
    unsafe {
        let array = g_ptr_array_new_with_free_func(Some(free_pstring));
        for uid in uids {
            let cuid = c_string(uid);
            let pooled = camel_pstring_strdup(cuid.as_ptr());
            g_ptr_array_add(array, pooled.cast_mut().cast());
        }
        array
    }
}

/// `camel_pstring_free` as a `GDestroyNotify`, which is a wrapper rather than a
/// cast because the two signatures differ in their argument type.
///
/// # Safety
///
/// `string` must be NULL or a pointer the string pool handed out.
unsafe extern "C" fn free_pstring(string: gpointer) {
    // SAFETY: the contract above.
    unsafe { camel_pstring_free(string.cast()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn no_words_is_nothing_to_ask() {
        assert_eq!(body_filter(&[]), None);
    }

    #[test]
    fn one_word_is_one_condition() {
        assert_eq!(
            body_filter(&words(&["quarterly"])),
            Some(Filter::condition(
                EmailQueryFilter::default().body("quarterly")
            ))
        );
    }

    #[test]
    fn several_words_all_have_to_match() {
        assert_eq!(
            body_filter(&words(&["quarterly", "report"])),
            Some(Filter::and([
                Filter::condition(EmailQueryFilter::default().body("quarterly")),
                Filter::condition(EmailQueryFilter::default().body("report")),
            ]))
        );
    }

    /// An empty word matches everything Camel would have matched anyway, so
    /// dropping it leaves the rest of the term asking for exactly as much.
    #[test]
    fn an_empty_word_is_dropped_rather_than_asked_about() {
        assert_eq!(
            body_filter(&words(&["", "report"])),
            Some(Filter::condition(
                EmailQueryFilter::default().body("report")
            ))
        );
        assert_eq!(body_filter(&words(&["", ""])), None);
    }
}
