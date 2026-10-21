// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelFolderClass::search_body_sync` (EDS 3.58 and later): the server
//! answers the words of a body-contains term.
//!
//! Up to 3.52, [`crate::folder`] sends a full search expression to the server
//! through [`crate::search_sexp`]. From 3.58, `CamelStoreSearch` evaluates the
//! expression over the local database, which does not have the body of a
//! message that this provider did not download.
//!
//! For a body term that the message preview cannot answer, `CamelStoreSearch`
//! calls this vfunc (`_camel_store_search_check_body_match` in
//! `camel-store-search.c`):
//!
//! - TRUE: the returned uid set is authoritative for the folder.
//! - FALSE: the search opens every message in the folder and scans its body
//!   locally. This downloads the full folder, and a message that it cannot
//!   fetch does not match.
//!
//! ## Which words match
//!
//! Camel matches each word as a case-insensitive substring
//! (`CAMEL_SEARCH_MATCH_CONTAINS`), and a message must contain all words.
//! RFC 8621 §4.4.1 lets the server decide how `body` matches, and servers
//! usually match tokens, so a partial word can fail to match. The
//! `body-contains` translation of `search_sexp` and IMAPX have the same
//! difference. Each word becomes a separate condition in an AND, because
//! RFC 8621 lets a server interpret a multi-word string as it chooses.

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
/// The slot exists from EDS 3.58. An older EDS sends the body term to the
/// server through `search_by_expression`, which 3.58 removed.
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

/// The JMAP filter for the `words` of a body term, or `None` if no word is left.
/// A message matches when its body contains all the words.
///
/// The filter ignores an empty word. Camel matches an empty substring to all
/// messages, so this does not change the result. RFC 8621 does not define an
/// empty `body` condition. If all words are empty, the result is `None`, and the
/// vfunc refuses the search: it does not answer "all messages".
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

/// `CamelFolderClass.search_body_sync`: the server finds the messages of this
/// folder that have `words` in their body.
///
/// On success, the result is TRUE and `out_uids` is set. An empty array means
/// "no message matches", not "unknown". On failure, the result is FALSE and
/// `error` is set, and `CamelStoreSearch` then scans the messages locally.
///
/// A term with no non-empty word, a folder with no mailbox id, and a store with
/// no connection all give FALSE. The last two are the offline case: the messages
/// on disk give a better answer there, and an empty array can be wrong.
///
/// This function compiles on every EDS, because it uses no type that older
/// headers do not have. Thus the tests of this crate can call it.
///
/// # Safety
///
/// The Camel contract for the vfunc: `folder` is a live instance of this
/// provider's folder, `words` is NULL or a live `GPtrArray` of NUL-terminated
/// strings, and `out_uids` is a valid location for one array pointer.
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

/// The `GPtrArray` that this vfunc returns: pooled strings, with a free
/// function for them.
///
/// The caller only calls `g_ptr_array_unref` on this array, so the array must
/// free its elements. (The base class frees the result of the older search
/// vfuncs of [`crate::folder`] element by element, with `camel_folder_search_free`.)
/// The base implementation and IMAPX also use `camel_pstring_strdup` with
/// `camel_pstring_free` for this array.
fn pooled_uid_array(uids: &[&str]) -> *mut GPtrArray {
    // SAFETY: the array is new, and its free function can free every pointer
    // that goes into it.
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

/// `camel_pstring_free` as a `GDestroyNotify`. A cast is not sufficient, because
/// the argument types of the two signatures are different.
///
/// # Safety
///
/// `string` must be NULL or a pointer from the string pool.
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

    /// Camel matches an empty word to all messages, so the filter can ignore it
    /// without a change to the result.
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
