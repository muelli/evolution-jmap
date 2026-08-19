// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The bodies of the `ECalMetaBackend` sync vfuncs.
//!
//! Each function here has the shape of the vfunc it implements — the same
//! out-parameters, the same "FALSE means `error` is set" contract — but takes a
//! `&CalSync` instead of an `ECalMetaBackend *`. The subclass on top is then a
//! panic guard, a look at the connection slot, and a call to one of these; and,
//! more to the point, all of this is testable, because constructing a real
//! `ECalMetaBackend` needs an `ESourceRegistry` and so a running
//! `evolution-source-registry` on the session bus.
//!
//! The out-parameter discipline is [`jmap_backend_core::marshal`]'s, and it is
//! what EDS relies on: on success every out-parameter the caller asked for is
//! written and ownership of it passes to EDS; on failure nothing is written and
//! `error` is set instead, because EDS only frees the outputs of a call that
//! succeeded; a NULL out-parameter means "not interested" and is skipped.
//!
//! Three arguments of the real vfuncs do not appear here. `extra` is the
//! per-object opaque state a backend may park in the EDS cache, and this one has
//! none — the JMAP id *is* the uid and the revision already carries the change
//! token. `EConflictResolution` is a promise this backend cannot keep yet: JMAP
//! can express it with a `CalendarEvent/set` `ifInState`, but `CalSync` does not
//! send one, so accepting the argument and ignoring it would read as support.
//! `ECalOperationFlags` carries iTIP scheduling requests, which this milestone
//! does not implement either.

use eds_sys::{
    E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND, ETimezoneCache, ICalComponent, e_cal_client_error_create,
};
use glib_sys::{GError, GFALSE, GSList, GTRUE, gboolean, gchar};
use jmap_backend_core::error::{
    cstring_lossy, fail_bool, fail_invalid, invalid_arg_gerror, set_raw_gerror,
};
use jmap_backend_core::i18n::{translate, translate_with};
use jmap_backend_core::marshal::{read_string, set_out_list, set_out_string};
use jmap_cal_sync::{CalSync, SyncError, Unsendable};
use jmap_proto::State;

use crate::marshal;

/// What `get_changes_sync` decided, which is one answer more than a `gboolean`
/// can carry.
#[derive(Debug)]
pub enum Outcome {
    /// The out-parameters are filled in; the vfunc returns TRUE.
    Reported,
    /// This delta cannot be computed, and that is not a failure: the caller
    /// chains up to `ECalMetaBackend`'s own `get_changes_sync`, which lists the
    /// calendar in full and diffs it against the offline cache. Nothing has been
    /// written and no error has been set.
    ListInstead,
    /// `error` is set; the vfunc returns FALSE.
    Failed,
}

/// Every event in the calendar — `list_existing_sync`.
///
/// # Safety
///
/// The out-parameters must each be NULL or point at a writable, currently-NULL
/// location of the matching type, and `error` must be NULL or a valid,
/// currently-NULL `GError **`. That is what an EDS vfunc receives.
pub unsafe fn list_existing(
    sync: &CalSync,
    out_new_sync_tag: *mut *mut gchar,
    out_existing_objects: *mut *mut GSList,
    error: *mut *mut GError,
) -> gboolean {
    let (state, components) = match sync.list_existing() {
        Ok(listed) => listed,
        // SAFETY: `error` satisfies set_raw_gerror's contract by this
        // function's own.
        Err(failure) => return unsafe { fail_bool(error, &failure, to_gerror) },
    };

    // SAFETY: as above for the out-parameters; both allocations are GLib ones
    // ownership of which passes to the caller.
    unsafe {
        set_out_string(out_new_sync_tag, state.as_str());
        set_out_list(out_existing_objects, || marshal::info_list(&components));
    }
    GTRUE
}

/// What changed since `last_sync_tag` — `get_changes_sync`.
///
/// Two of the three answers are "list the whole calendar instead": no tag at
/// all, which is the first sync, and a tag the server will not diff from
/// (RFC 8620 §5.2, `cannotCalculateChanges`). Reporting either as a failure
/// would leave the calendar empty until someone deleted the cache by hand.
///
/// Everything that changed is reported as **modified** rather than split across
/// `out_created_objects` and `out_modified_objects`. JMAP does draw that
/// distinction, but `CalSync::get_changes` has already spent it on a question
/// only it can answer — an event that shows up as *updated* and is no longer
/// filed in this calendar has been moved out and must be reported gone, whereas
/// a *created* one that is not ours never was our business — and what remains is
/// a set of events that exist and are ours. EDS runs both lists through the same
/// loader, so the split is presentational; inventing one would be a guess
/// dressed up as information.
///
/// `is_repeat` is EDS saying it has come back for the rest of a delta. It cannot
/// be true of anything this backend asked for, because the paging happens inside
/// `CalSync::get_changes` and `out_repeat` is always FALSE — and the delta a tag
/// names does not depend on how often it has been asked for, so the flag has
/// nothing to change.
///
/// # Safety
///
/// As [`list_existing`], and `last_sync_tag` must be NULL or a valid
/// NUL-terminated string.
#[allow(clippy::too_many_arguments)] // the vfunc's signature, not ours
pub unsafe fn get_changes(
    sync: &CalSync,
    last_sync_tag: *const gchar,
    _is_repeat: gboolean,
    out_new_sync_tag: *mut *mut gchar,
    out_repeat: *mut gboolean,
    _out_created_objects: *mut *mut GSList,
    out_modified_objects: *mut *mut GSList,
    out_removed_objects: *mut *mut GSList,
    error: *mut *mut GError,
) -> Outcome {
    // SAFETY: the caller guarantees a valid string or NULL.
    let Some(tag) = (unsafe { read_string(last_sync_tag) }) else {
        return Outcome::ListInstead;
    };

    let changes = match sync.get_changes(&State::from(tag)) {
        Ok(changes) => changes,
        Err(failure) if failure.is_cannot_calculate_changes() => return Outcome::ListInstead,
        Err(failure) => {
            // SAFETY: `error` satisfies the contract by this function's own.
            unsafe { set_raw_gerror(error, to_gerror(&failure)) };
            return Outcome::Failed;
        }
    };

    // SAFETY: as above for the out-parameters; the allocations are GLib ones
    // ownership of which passes to the caller.
    unsafe {
        set_out_string(out_new_sync_tag, changes.new_state.as_str());
        // The paging happens inside `CalSync::get_changes`, so there is never
        // anything left for EDS to ask again for.
        if !out_repeat.is_null() {
            *out_repeat = GFALSE;
        }
        set_out_list(out_modified_objects, || {
            marshal::info_list(&changes.changed)
        });
        set_out_list(out_removed_objects, || {
            marshal::removed_info_list(&changes.removed)
        });
    }
    Outcome::Reported
}

/// One event by identifier — `load_component_sync`.
///
/// `out_extra` is deliberately left alone, for the reason given in the module
/// docs.
///
/// # Safety
///
/// As [`list_existing`], and `uid` must be NULL or a valid NUL-terminated
/// string.
pub unsafe fn load_component(
    sync: &CalSync,
    uid: *const gchar,
    out_component: *mut *mut ICalComponent,
    _out_extra: *mut *mut gchar,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: the caller guarantees a valid string or NULL.
    let Some(uid) = (unsafe { read_string(uid) }) else {
        // SAFETY: `error` satisfies the contract by this function's own.
        return unsafe { fail_invalid(error, "a component was asked for without an identifier") };
    };

    let info = match sync.load_component(&uid) {
        Ok(info) => info,
        // SAFETY: as above.
        Err(failure) => return unsafe { fail_bool(error, &failure, to_gerror) },
    };

    let component = marshal::component_from_ical(&info.icalendar);
    if component.is_null() {
        // Our own rendering of the event is not a calendar object, which is a
        // bug here rather than anything the server did — but it still has to
        // reach EDS as a failure rather than as an empty appointment.
        // SAFETY: as above.
        return unsafe {
            fail_invalid(
                error,
                &format!("the event {uid} could not be rendered as iCalendar"),
            )
        };
    }

    // SAFETY: `out_component` satisfies the contract by this function's own; the
    // reference taken by `component_from_ical` passes to the caller, or is
    // dropped again if the caller did not want it.
    unsafe {
        if out_component.is_null() {
            marshal::component_unref(component);
        } else {
            *out_component = component;
        }
    }
    GTRUE
}

/// Store an event — `save_component_sync`.
///
/// `out_new_extra` is left alone, for the reason given in the module docs.
///
/// EDS passes every instance of one uid it holds; which of them is the event to
/// send, and why the detached occurrences are dropped rather than guessed at, is
/// [`marshal::icalendar_from_instances`]'s decision. A set of instances it will
/// not read is refused here rather than turned into a silent no-op: EDS would
/// otherwise report the edit as saved and the server would never have heard of
/// it.
///
/// `zones` is the calendar itself, which is where the definition of a zone no
/// zone database knows lives; the marshalling says what it is asked for.
///
/// # Safety
///
/// As [`list_existing`], and `instances` must be NULL or a valid `GSList` whose
/// nodes are `ECalComponent *`, and `zones` NULL or a valid `ETimezoneCache`.
pub unsafe fn save_component(
    sync: &CalSync,
    overwrite_existing: gboolean,
    instances: *const GSList,
    zones: *mut ETimezoneCache,
    out_new_uid: *mut *mut gchar,
    _out_new_extra: *mut *mut gchar,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: the caller guarantees the list's shape.
    let Some(saved) = (unsafe { marshal::icalendar_from_instances(instances, zones) }) else {
        // SAFETY: `error` satisfies the contract by this function's own.
        return unsafe {
            fail_invalid(
                error,
                "the component to save has no master instance to send",
            )
        };
    };

    let existing_uid = if overwrite_existing == GFALSE {
        // A create. The component's `UID` is a name Evolution invented locally
        // and never a JMAP id, so the server assigns the real one — and the
        // local name survives as the JSCalendar `uid`, which is `CalSync`'s
        // business rather than this layer's.
        None
    } else {
        match saved.uid {
            Some(uid) => Some(uid),
            // Sending this as a create would silently duplicate the user's
            // appointment on the server, which is worse than a visible failure.
            // SAFETY: as above.
            None => {
                return unsafe {
                    fail_invalid(error, "a component was edited without an identifier")
                };
            }
        }
    };

    let info = match sync.save_component(&saved.icalendar, existing_uid.as_deref()) {
        Ok(info) => info,
        // SAFETY: as above.
        Err(failure) => return unsafe { fail_bool(error, &failure, to_gerror) },
    };

    // SAFETY: `out_new_uid` satisfies the contract by this function's own, and
    // ownership of the duplicate passes through it.
    unsafe { set_out_string(out_new_uid, &info.uid) };
    GTRUE
}

/// Destroy an event — `remove_component_sync`.
///
/// # Safety
///
/// As [`list_existing`], and `uid` must be NULL or a valid NUL-terminated
/// string.
pub unsafe fn remove_component(
    sync: &CalSync,
    uid: *const gchar,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: the caller guarantees a valid string or NULL.
    let Some(uid) = (unsafe { read_string(uid) }) else {
        // SAFETY: `error` satisfies the contract by this function's own.
        return unsafe { fail_invalid(error, "a component was removed without an identifier") };
    };

    match sync.remove_component(&uid) {
        Ok(()) => GTRUE,
        // SAFETY: as above.
        Err(failure) => unsafe { fail_bool(error, &failure, to_gerror) },
    }
}

/// Allocates a `GError` describing `failure`. Ownership passes to the caller,
/// as with [`jmap_backend_core::error::to_gerror`], which handles the
/// [`SyncError::Client`] half.
///
/// The other half is the calendar's own domain: an event that is not there has
/// to be reported as `E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND`, because
/// `ECalMetaBackend` matches on exactly that domain and code to decide that a
/// component is gone rather than that the sync failed. The address book's
/// `CONTACT_NOT_FOUND` will not do: it is a different quark, so the match simply
/// fails and the cache entry never goes away.
pub fn to_gerror(failure: &SyncError) -> *mut GError {
    match failure {
        SyncError::Client(error) => jmap_backend_core::error::to_gerror(error),
        SyncError::NotFound(_) => {
            let message = cstring_lossy(&failure.to_string());
            // SAFETY: the code is one of the enum's own values and the message
            // is copied by the call.
            unsafe {
                e_cal_client_error_create(E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND, message.as_ptr())
            }
        }
        // An iCalendar object Evolution handed us that the mapping cannot read:
        // the argument was bad, not the server. The message is the mapping's
        // own account of what it could not parse — developer-facing text, and
        // deliberately not translated, because a user cannot act on it and a
        // bug report should quote it in the language it was written in.
        SyncError::ICal(_) => invalid_arg_gerror(&failure.to_string()),
        // A component we could read but cannot state as JSCalendar. The same
        // code — the save was refused over what the component says, so the
        // argument was bad rather than the server — and the one message here a
        // user is expected to read and act on, so this one is translated.
        SyncError::Unsendable(reason) => invalid_arg_gerror(&refusal(reason)),
    }
}

/// What the user is told when a create was refused over its recurrence.
///
/// The sentence lives here rather than in `jmap-cal-sync` because this is where
/// it can be translated: gettext is bound on this side of the FFI, and the sync
/// layer — whose tests link no EDS — cannot reach it. So the refusal arrives as
/// an [`Unsendable`] naming what could not be stated, and the wording is chosen
/// here, at the point where the user's language is also consulted.
///
/// Both messages end by naming the spelling that does work. A refusal that only
/// says no leaves the user to guess which part of an appointment offended, and
/// the answer — state the recurrence as a repeat count rather than an end date —
/// is one sentence long.
///
/// Each message is one long line, and has to stay one: `xgettext` reads the
/// source as C, where a backslash at the end of a line drops the newline and
/// keeps the indentation that follows, while Rust drops both. Wrapped, the
/// msgid in the catalogue would carry indentation the lookup does not and no
/// translation would ever be found — silently.
/// `jmap-backend-core/tests/potfiles.rs` holds every marked literal to that.
fn refusal(reason: &Unsendable) -> String {
    match reason {
        Unsendable::RecurrenceEnd { until, zone } => translate_with(
            // TRANSLATORS: shown when a new recurring appointment could not be
            // saved. %1$s is the date and time the series ends, as it was
            // written in the appointment; %2$s is a time zone identifier such
            // as "Europe/Berlin".
            c"This event repeats until %1$s, and the time zone it is in, %2$s, is not defined in this calendar entry in a way that instant can be converted out of — so the event was not created. Stating the recurrence as a repeat count works instead.",
            &[until.as_str(), zone.as_str()],
        ),
        // No placeholders, so no arguments: the mapping knows the rule cannot
        // be written back and nothing more precise than that is true.
        Unsendable::Recurrence => translate(
            // TRANSLATORS: shown when a new recurring appointment could not be
            // saved, and the reason is not something the user can be pointed
            // at more precisely than this.
            c"This event repeats in a way that cannot be stored on the server, so it was not created. Stating the recurrence as a repeat count is the spelling that always works.",
        ),
    }
}
