// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Implementation bodies for `ECalMetaBackend` sync vfuncs operating over [`CalSync`].
//! Out-parameters are written and transferred to EDS on success; errors are set on failure.

use eds_sys::{
    E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND, E_CLIENT_ERROR_NOT_SUPPORTED, ETimezoneCache,
    ICalComponent, e_cal_client_error_create, e_client_error_create, time_t,
};
use glib_sys::{GError, GFALSE, GSList, GTRUE, gboolean, gchar};
use jmap_backend_core::error::{
    cstring_lossy, fail_bool, fail_invalid, invalid_arg_gerror, set_raw_gerror,
};
use jmap_backend_core::i18n::{translate, translate_with};
use jmap_backend_core::marshal::{dup_string, read_string, set_out_list, set_out_string};
use jmap_cal_sync::{CalSync, Itip, SyncError, Unscheduled, Unsendable};
use jmap_proto::State;

use crate::marshal;

/// Outcome of a `get_changes` operation.
#[derive(Debug)]
pub enum Outcome {
    /// Changes were calculated and out-parameters populated.
    Reported,
    /// Changes cannot be calculated; caller chains up to full cache diffing.
    ListInstead,
    /// Operation failed; GError set.
    Failed,
}

/// Lists all existing components in the calendar for `list_existing_sync`.
///
/// # Safety
///
/// Out-parameters and `error` must be valid pointers per EDS vfunc contracts.
pub unsafe fn list_existing(
    sync: &CalSync,
    out_new_sync_tag: *mut *mut gchar,
    out_existing_objects: *mut *mut GSList,
    error: *mut *mut GError,
) -> gboolean {
    let account_id = sync.account_id().as_str();
    let calendar_id = sync.calendar_id().as_str();
    tracing::debug!(
        account_id,
        calendar_id,
        "listing existing calendar components"
    );
    let (state, components) = match sync.list_existing() {
        Ok(listed) => listed,
        // SAFETY: `error` satisfies set_raw_gerror contract.
        Err(failure) => {
            tracing::debug!(
                account_id,
                calendar_id,
                ?failure,
                "listing existing calendar components failed"
            );
            return unsafe { fail_bool(error, &failure, to_gerror) };
        }
    };

    tracing::debug!(
        account_id,
        calendar_id,
        state = state.as_str(),
        count = components.len(),
        "listed existing calendar components"
    );

    // SAFETY: Out-parameter allocations transfer ownership to caller. All outputs
    // are built into locals first and published in a single step so no out-parameter
    // is written before every fallible allocation is complete (audit F6).
    unsafe {
        let existing_objects =
            (!out_existing_objects.is_null()).then(|| marshal::info_list(&components));
        let new_sync_tag = (!out_new_sync_tag.is_null()).then(|| dup_string(state.as_str()));

        if let Some(list) = existing_objects {
            *out_existing_objects = list;
        }
        if let Some(tag) = new_sync_tag {
            *out_new_sync_tag = tag;
        }
    }
    GTRUE
}

/// Computes component changes since `last_sync_tag` for `get_changes_sync`.
///
/// Returns [`Outcome::ListInstead`] if `last_sync_tag` is absent or the server
/// returns `cannotCalculateChanges` (RFC 8620 §5.2).
///
/// # Safety
///
/// As [`list_existing`], and `last_sync_tag` must be NULL or a valid NUL-terminated string.
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
    let account_id = sync.account_id().as_str();
    let calendar_id = sync.calendar_id().as_str();
    // SAFETY: the caller guarantees a valid string or NULL.
    let Some(tag) = (unsafe { read_string(last_sync_tag) }) else {
        tracing::debug!(
            account_id,
            calendar_id,
            "get_changes called with no last_sync_tag; listing calendar instead"
        );
        return Outcome::ListInstead;
    };

    tracing::debug!(
        account_id,
        calendar_id,
        last_sync_tag = tag.as_str(),
        "getting calendar changes"
    );

    let changes = match sync.get_changes(&State::from(tag)) {
        Ok(changes) => changes,
        Err(failure) if failure.is_cannot_calculate_changes() => {
            tracing::debug!(
                account_id,
                calendar_id,
                "server cannot calculate changes; listing calendar instead"
            );
            return Outcome::ListInstead;
        }
        Err(failure) => {
            tracing::debug!(
                account_id,
                calendar_id,
                ?failure,
                "getting calendar changes failed"
            );
            // SAFETY: `error` satisfies the contract by this function's own.
            unsafe { set_raw_gerror(error, to_gerror(&failure)) };
            return Outcome::Failed;
        }
    };

    tracing::debug!(
        account_id,
        calendar_id,
        new_sync_tag = changes.new_state.as_str(),
        changed_count = changes.changed.len(),
        removed_count = changes.removed.len(),
        "reported calendar changes"
    );

    // SAFETY: Out-parameter allocations transfer ownership to caller. All outputs
    // are built into locals first and published in a single step so no out-parameter
    // is written before every fallible allocation is complete (audit F6).
    unsafe {
        let modified =
            (!out_modified_objects.is_null()).then(|| marshal::info_list(&changes.changed));
        let removed =
            (!out_removed_objects.is_null()).then(|| marshal::removed_info_list(&changes.removed));
        let new_sync_tag =
            (!out_new_sync_tag.is_null()).then(|| dup_string(changes.new_state.as_str()));

        if let Some(list) = modified {
            *out_modified_objects = list;
        }
        if let Some(list) = removed {
            *out_removed_objects = list;
        }
        if let Some(tag) = new_sync_tag {
            *out_new_sync_tag = tag;
        }
        // Paging occurs inside CalSync::get_changes; signal completion to EDS.
        if !out_repeat.is_null() {
            *out_repeat = GFALSE;
        }
    }
    Outcome::Reported
}

/// Loads a component by UID for `load_component_sync`.
///
/// # Safety
///
/// As [`list_existing`], and `uid` must be NULL or a valid NUL-terminated string.
pub unsafe fn load_component(
    sync: &CalSync,
    uid: *const gchar,
    out_component: *mut *mut ICalComponent,
    _out_extra: *mut *mut gchar,
    error: *mut *mut GError,
) -> gboolean {
    let account_id = sync.account_id().as_str();
    let calendar_id = sync.calendar_id().as_str();
    // SAFETY: the caller guarantees a valid string or NULL.
    let Some(uid) = (unsafe { read_string(uid) }) else {
        tracing::debug!(
            account_id,
            calendar_id,
            "component was asked for without an identifier"
        );
        // SAFETY: `error` satisfies the contract by this function's own.
        return unsafe { fail_invalid(error, "a component was asked for without an identifier") };
    };

    tracing::debug!(
        account_id,
        calendar_id,
        uid = uid.as_str(),
        "loading component"
    );

    let info = match sync.load_component(&uid) {
        Ok(info) => info,
        // SAFETY: as above.
        Err(failure) => {
            tracing::debug!(
                account_id,
                calendar_id,
                uid = uid.as_str(),
                ?failure,
                "loading component failed"
            );
            return unsafe { fail_bool(error, &failure, to_gerror) };
        }
    };

    let component = marshal::component_from_ical(&info.icalendar);
    if component.is_null() {
        tracing::debug!(
            account_id,
            calendar_id,
            uid = uid.as_str(),
            "component could not be rendered as iCalendar"
        );
        // SAFETY: as above.
        return unsafe {
            fail_invalid(
                error,
                &format!("the event {uid} could not be rendered as iCalendar"),
            )
        };
    }

    tracing::debug!(
        account_id,
        calendar_id,
        uid = uid.as_str(),
        revision = info.revision.as_str(),
        "loaded component"
    );

    // SAFETY: `out_component` receives component ownership or it is dropped.
    unsafe {
        if out_component.is_null() {
            marshal::component_unref(component);
        } else {
            *out_component = component;
        }
    }
    GTRUE
}

/// Creates or updates a component for `save_component_sync`.
///
/// `itip` says whether the server may tell the guests about this change
/// (see [`crate::scheduling::itip`]).
///
/// # Safety
///
/// As [`list_existing`], `instances` must be a valid GSList of `ECalComponent *`,
/// and `zones` NULL or a valid `ETimezoneCache`.
#[allow(clippy::too_many_arguments)] // the vfunc's signature, plus `itip`
pub unsafe fn save_component(
    sync: &CalSync,
    overwrite_existing: gboolean,
    instances: *const GSList,
    zones: *mut ETimezoneCache,
    itip: Itip,
    out_new_uid: *mut *mut gchar,
    _out_new_extra: *mut *mut gchar,
    error: *mut *mut GError,
) -> gboolean {
    let account_id = sync.account_id().as_str();
    let calendar_id = sync.calendar_id().as_str();
    let is_overwrite = overwrite_existing != GFALSE;

    // SAFETY: the caller guarantees the list's shape.
    let Some(saved) = (unsafe { marshal::icalendar_from_instances(instances, zones) }) else {
        tracing::debug!(
            account_id,
            calendar_id,
            "component to save has no master instance to send"
        );
        // SAFETY: `error` satisfies the contract by this function's own.
        return unsafe {
            fail_invalid(
                error,
                "the component to save has no master instance to send",
            )
        };
    };

    let existing_uid = if !is_overwrite {
        // A create. Local UID is ignored; the server assigns the identifier.
        None
    } else {
        match saved.uid {
            Some(uid) => Some(uid),
            // Editing without a UID would duplicate rather than update; report invalid argument.
            // SAFETY: as above.
            None => {
                tracing::debug!(
                    account_id,
                    calendar_id,
                    "component was edited without an identifier"
                );
                return unsafe {
                    fail_invalid(error, "a component was edited without an identifier")
                };
            }
        }
    };

    tracing::debug!(
        account_id,
        calendar_id,
        overwrite_existing = is_overwrite,
        existing_uid = existing_uid.as_deref(),
        ?itip,
        "saving component"
    );

    let info = match sync.save_component_with(&saved.icalendar, existing_uid.as_deref(), itip) {
        Ok(info) => info,
        // SAFETY: as above.
        Err(failure) => {
            tracing::debug!(
                account_id,
                calendar_id,
                overwrite_existing = is_overwrite,
                existing_uid = existing_uid.as_deref(),
                ?failure,
                "saving component failed"
            );
            return unsafe { fail_bool(error, &failure, to_gerror) };
        }
    };

    tracing::debug!(
        account_id,
        calendar_id,
        uid = info.uid.as_str(),
        revision = info.revision.as_str(),
        "saved component"
    );

    // SAFETY: `out_new_uid` receives ownership of allocated string.
    unsafe { set_out_string(out_new_uid, &info.uid) };
    GTRUE
}

/// Destroys a component for `remove_component_sync`, with `itip` as
/// [`save_component`]'s.
///
/// # Safety
///
/// As [`list_existing`], and `uid` must be NULL or a valid NUL-terminated string.
pub unsafe fn remove_component(
    sync: &CalSync,
    uid: *const gchar,
    itip: Itip,
    error: *mut *mut GError,
) -> gboolean {
    let account_id = sync.account_id().as_str();
    let calendar_id = sync.calendar_id().as_str();
    // SAFETY: the caller guarantees a valid string or NULL.
    let Some(uid) = (unsafe { read_string(uid) }) else {
        tracing::debug!(
            account_id,
            calendar_id,
            "component was removed without an identifier"
        );
        // SAFETY: `error` satisfies the contract by this function's own.
        return unsafe { fail_invalid(error, "a component was removed without an identifier") };
    };

    tracing::debug!(
        account_id,
        calendar_id,
        uid = uid.as_str(),
        ?itip,
        "removing component"
    );

    match sync.remove_component_with(&uid, itip) {
        Ok(()) => {
            tracing::debug!(
                account_id,
                calendar_id,
                uid = uid.as_str(),
                "removed component"
            );
            GTRUE
        }
        // SAFETY: as above.
        Err(failure) => {
            tracing::debug!(
                account_id,
                calendar_id,
                uid = uid.as_str(),
                ?failure,
                "removing component failed"
            );
            unsafe { fail_bool(error, &failure, to_gerror) }
        }
    }
}

/// Allocates a `GError` for `failure`, mapping `NotFound` to `E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND`.
pub fn to_gerror(failure: &SyncError) -> *mut GError {
    match failure {
        SyncError::Client(error) => jmap_backend_core::error::to_gerror(error),
        SyncError::NotFound(_) => {
            let message = cstring_lossy(&failure.to_string());
            // SAFETY: code is valid enum value and message is copied.
            unsafe {
                e_cal_client_error_create(E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND, message.as_ptr())
            }
        }
        SyncError::ICal(_) => invalid_arg_gerror(&failure.to_string()),
        SyncError::Unsendable(reason) => invalid_arg_gerror(&refusal(reason)),
        SyncError::Unscheduled(reason @ Unscheduled::Unreachable { .. }) => {
            invalid_arg_gerror(&unscheduled(reason))
        }
        SyncError::Unscheduled(reason @ Unscheduled::NotImplemented) => {
            let message = cstring_lossy(&unscheduled(reason));
            // SAFETY: code is valid enum value and message is copied.
            unsafe { e_client_error_create(E_CLIENT_ERROR_NOT_SUPPORTED, message.as_ptr()) }
        }
    }
}

/// Translates the user-facing message for a change the server would not
/// schedule. Evolution was told the server mails the guests, so each says the
/// change was not saved and what to do about it.
fn unscheduled(reason: &Unscheduled) -> String {
    match reason {
        Unscheduled::Unreachable { addresses } if addresses.is_empty() => translate(
            // TRANSLATORS: shown when the server refused to send a meeting's invitations.
            c"The server cannot send meeting invitations to every guest, so the change was not saved. Check that each guest has a valid email address.",
        ),
        Unscheduled::Unreachable { addresses } => translate_with(
            // TRANSLATORS: shown when the server refused to send a meeting's invitations. %1$s is a comma-separated list of guest addresses.
            c"The server cannot send meeting invitations to every guest, so the change was not saved. It has no way to reach: %1$s.",
            &[addresses.join(", ").as_str()],
        ),
        Unscheduled::NotImplemented => translate(
            // TRANSLATORS: shown when the server does not send meeting invitations itself. "Server handles meeting invitations" is the calendar's setting of that name.
            c"This server does not send meeting invitations itself, so the change was not saved. Turn off “Server handles meeting invitations” for this calendar, and Evolution will send them instead.",
        ),
    }
}

/// Translates user-facing refusal message for recurrence constraints.
fn refusal(reason: &Unsendable) -> String {
    match reason {
        Unsendable::RecurrenceEnd { until, zone } => translate_with(
            // TRANSLATORS: shown when a new recurring appointment could not be saved.
            c"This event repeats until %1$s, and the time zone it is in, %2$s, is not defined in this calendar entry in a way that instant can be converted out of — so the event was not created. Stating the recurrence as a repeat count works instead.",
            &[until.as_str(), zone.as_str()],
        ),
        Unsendable::Recurrence => translate(
            // TRANSLATORS: shown when a new recurring appointment could not be saved.
            c"This event repeats in a way that cannot be stored on the server, so it was not created. Stating the recurrence as a repeat count is the spelling that always works.",
        ),
    }
}

/// Outcome of a `get_free_busy` query.
#[derive(Debug)]
pub enum FreeBusyOutcome {
    /// Out-parameters are populated; caller returns without chaining up.
    Reported,
    /// No busy periods known; caller chains up to parent cache search.
    NothingKnown,
    /// Operation failed; GError set.
    Failed,
}

/// Queries busy periods of specified attendees for `get_free_busy_sync`.
///
/// # Safety
///
/// `users` must be a valid GSList of NUL-terminated strings; out-parameters and `error` valid per EDS contracts.
pub unsafe fn get_free_busy(
    sync: &CalSync,
    users: *const GSList,
    start: time_t,
    end: time_t,
    out_freebusy: *mut *mut GSList,
    error: *mut *mut GError,
) -> FreeBusyOutcome {
    let account_id = sync.account_id().as_str();
    let calendar_id = sync.calendar_id().as_str();
    // SAFETY: the caller guarantees a well-formed list of live strings.
    let users = unsafe { marshal::user_list(users) };
    if users.is_empty() {
        tracing::debug!(
            account_id,
            calendar_id,
            "get_free_busy called with empty user list; falling through to parent"
        );
        return FreeBusyOutcome::NothingKnown;
    }

    let (Some(utc_start), Some(utc_end)) = (marshal::utc_date(start), marshal::utc_date(end))
    else {
        tracing::debug!(
            account_id,
            calendar_id,
            start,
            end,
            "get_free_busy called with invalid time range; falling through to parent"
        );
        return FreeBusyOutcome::NothingKnown;
    };

    tracing::debug!(
        account_id,
        calendar_id,
        user_count = users.len(),
        start,
        end,
        "querying free-busy"
    );

    let answers = match sync.free_busy(&users, &utc_start, &utc_end) {
        Ok(answers) => answers,
        Err(failure) => {
            tracing::debug!(
                account_id,
                calendar_id,
                user_count = users.len(),
                ?failure,
                "querying free-busy failed"
            );
            // SAFETY: `error` satisfies set_raw_gerror contract.
            unsafe { fail_bool(error, &failure, to_gerror) };
            return FreeBusyOutcome::Failed;
        }
    };
    if answers.is_empty() {
        tracing::debug!(
            account_id,
            calendar_id,
            "querying free-busy returned no answers; falling through to parent"
        );
        return FreeBusyOutcome::NothingKnown;
    }

    tracing::debug!(
        account_id,
        calendar_id,
        answer_count = answers.len(),
        "querying free-busy returned answers"
    );

    // SAFETY: Out-parameter list allocations transfer ownership to caller.
    unsafe { set_out_list(out_freebusy, || marshal::free_busy_list(&answers)) };
    FreeBusyOutcome::Reported
}

/// Outcome of a calendar color change evaluation.
#[derive(Debug)]
pub enum ColorOutcome {
    /// Color matches baseline; no change sent.
    Unchanged,
    /// Color differed and was pushed to server; contains updated baseline.
    Pushed(Option<String>),
    /// Push failed; baseline remains unchanged.
    Failed,
}

/// Evaluates and pushes local calendar color edits for `source_changed`.
pub fn on_source_changed(
    sync: &CalSync,
    current: Option<&str>,
    baseline: Option<&str>,
) -> ColorOutcome {
    let account_id = sync.account_id().as_str();
    let calendar_id = sync.calendar_id().as_str();
    if current == baseline {
        tracing::debug!(
            account_id,
            calendar_id,
            current,
            baseline,
            "calendar color unchanged"
        );
        return ColorOutcome::Unchanged;
    }
    tracing::debug!(
        account_id,
        calendar_id,
        current,
        baseline,
        "pushing calendar color change"
    );
    match sync.set_color(current) {
        Ok(()) => {
            tracing::debug!(
                account_id,
                calendar_id,
                current,
                "pushed calendar color change"
            );
            ColorOutcome::Pushed(current.map(str::to_owned))
        }
        Err(failure) => {
            tracing::debug!(
                account_id,
                calendar_id,
                current,
                ?failure,
                "pushing calendar color change failed"
            );
            ColorOutcome::Failed
        }
    }
}
