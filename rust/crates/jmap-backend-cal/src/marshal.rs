// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Rust values ↔ the C types the `ECalMetaBackend` vfuncs traffic in.
//!
//! The address book's boundary was strings on both sides; this one is not. A
//! component comes back out of `load_component_sync` as an `ICalComponent *`,
//! a save arrives as a `GSList` of `ECalComponent *` EDS still owns, and even
//! the removals are `ECalMetaBackendInfo`s rather than the bare uids
//! `EBookMetaBackend` takes. Each of those is a way to get the ownership wrong
//! in a process that is not ours, so all of it lives here, with tests.
//!
//! Everything that goes *out* is a GLib allocation the caller takes ownership
//! of, because that is what the vfunc contract says: EDS frees an info list
//! with `e_cal_meta_backend_info_free` and an `out_new_sync_tag` with `g_free`.
//! Everything that comes *in* stays borrowed.

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    ECalComponent, I_CAL_RECURRENCEID_PROPERTY, I_CAL_VEVENT_COMPONENT, ICalComponent,
    e_cal_component_get_icalcomponent, e_cal_meta_backend_info_new, i_cal_component_as_ical_string,
    i_cal_component_clone, i_cal_component_get_first_component, i_cal_component_get_first_property,
    i_cal_component_get_uid, i_cal_component_isa, i_cal_component_new_from_string,
    i_cal_component_new_vcalendar, i_cal_component_take_component,
};
use glib_sys::{GSList, g_free, g_slist_prepend, gchar};
use gobject_sys::g_object_unref;
use jmap_backend_core::error::cstring_lossy;
use jmap_cal_sync::ComponentInfo;

/// The event a save is about, extracted from the instances EDS handed over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedComponent {
    /// The master's iCalendar `UID`, which for anything the server has already
    /// seen is the JMAP id. `None` for a component Evolution created without
    /// one, which can only be a create.
    pub uid: Option<String>,
    /// The master wrapped in a `VCALENDAR`, ready for `CalSync::save_component`.
    pub icalendar: String,
}

/// Wraps `infos` as a `GSList` of `ECalMetaBackendInfo`, the payload
/// `list_existing_sync` and `get_changes_sync` hand back. An empty slice is the
/// NULL list, which is what EDS reads as "no objects".
///
/// The `extra` field stays NULL: it is per-object opaque state a backend can
/// park in the EDS cache, and this backend has none — the JMAP id *is* the uid,
/// and the revision already carries the change token.
pub fn info_list(infos: &[ComponentInfo]) -> *mut GSList {
    let mut list = ptr::null_mut();
    // Prepending is the only O(1) GSList insertion, so walk backwards and the
    // result comes out in the order the caller gave.
    for info in infos.iter().rev() {
        let uid = cstring_lossy(&info.uid);
        let revision = cstring_lossy(&info.revision);
        let object = cstring_lossy(&info.icalendar);
        // SAFETY: the three pointers are valid for the call, which copies each
        // of them; a NULL `extra` is explicitly allowed.
        let node = unsafe {
            e_cal_meta_backend_info_new(
                uid.as_ptr(),
                revision.as_ptr(),
                object.as_ptr(),
                ptr::null(),
            )
        };
        // SAFETY: `list` is a valid GSList (initially the empty one) and `node`
        // is a fresh allocation ownership of which passes to it.
        list = unsafe { g_slist_prepend(list, node.cast()) };
    }
    list
}

/// The same, for `out_removed_objects`.
///
/// This is where the calendar parts company with the address book, which
/// reports removals as a list of bare strings: here they are infos too, so a
/// `GSList` of `gchar *` would be read as structs and the first bytes of a uid
/// dereferenced as pointers. Only the uid is filled in — a component that is
/// gone has no revision and no object to describe, and both are documented
/// nullable while the uid is not.
pub fn removed_info_list(uids: &[String]) -> *mut GSList {
    let mut list = ptr::null_mut();
    for uid in uids.iter().rev() {
        let uid = cstring_lossy(uid);
        // SAFETY: `uid` is valid for the call, which copies it; NULL is allowed
        // for the other three.
        let node = unsafe {
            e_cal_meta_backend_info_new(uid.as_ptr(), ptr::null(), ptr::null(), ptr::null())
        };
        // SAFETY: as in `info_list`.
        list = unsafe { g_slist_prepend(list, node.cast()) };
    }
    list
}

/// Parses an iCalendar object, or NULL if the text is not one that holds an
/// event.
///
/// libical says "not a calendar object" by returning NULL, which `EVCard` did
/// not — but it parses an *empty* `VCALENDAR` happily, and handing that back
/// from `load_component_sync` would reach Evolution as an appointment that
/// exists and has no properties. So the envelope has to contain something:
/// either the component is a `VEVENT` itself, or it has one inside.
pub fn component_from_ical(icalendar: &str) -> *mut ICalComponent {
    let text = cstring_lossy(icalendar);
    // SAFETY: `text` is a valid NUL-terminated string for the duration of the
    // call, which copies what it needs.
    let component = unsafe { i_cal_component_new_from_string(text.as_ptr()) };
    if component.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: `component` is the fresh allocation just checked for NULL.
    if unsafe { holds_event(component) } {
        component
    } else {
        // SAFETY: the reference is ours and is being dropped unreturned.
        unsafe { component_unref(component) };
        ptr::null_mut()
    }
}

/// Renders `component` back to iCalendar text.
///
/// # Safety
///
/// `component` must be NULL or a valid `ICalComponent`.
pub unsafe fn ical_from_component(component: *mut ICalComponent) -> Option<String> {
    if component.is_null() {
        return None;
    }
    // SAFETY: the returned string is a GLib allocation this call takes
    // ownership of.
    unsafe { take_string(i_cal_component_as_ical_string(component)) }
}

/// The `UID` of the event `component` describes, or `None` if it has none.
///
/// Works on an envelope as readily as on a bare `VEVENT`, and both arrive:
/// `jmap-ical` renders a `VCALENDAR`, while the instances a save is handed are
/// single components. No descent is written here, because libical does it —
/// `icalcomponent_get_uid` reads the first *real* component of what it is
/// given, which for an envelope is the event inside it.
///
/// Absent has to stay distinguishable from empty. The save path tells a create
/// from an edit by whether there is a uid, and an empty string would be sent to
/// the server as the identifier of an event to patch.
///
/// # Safety
///
/// `component` must be NULL or a valid `ICalComponent`.
pub unsafe fn component_uid(component: *mut ICalComponent) -> Option<String> {
    if component.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees a valid component; the string belongs to it
    // and is valid until it is mutated, which nothing here does.
    let uid = unsafe { i_cal_component_get_uid(component) };
    if uid.is_null() {
        return None;
    }
    // SAFETY: a non-NULL libical string is NUL-terminated.
    let uid = unsafe { CStr::from_ptr(uid) }
        .to_string_lossy()
        .into_owned();
    (!uid.is_empty()).then_some(uid)
}

/// The event to save, out of the instances `save_component_sync` was given.
///
/// EDS passes every instance of one uid it has: the master, and one component
/// per detached occurrence. The master is the one **without** a
/// `RECURRENCE-ID`, and it is found by that rather than by position, because
/// taking the first node would map a single moved occurrence as if it were the
/// whole series.
///
/// All of them go into the envelope, master first — the shape `jmap-ical` reads
/// a series and its `recurrenceOverrides` out of, and the shape it renders back.
/// Dropping the detached ones here, which this used to do, is no longer merely
/// a loss: the mapping now *draws* an edited instance, so a save that sent only
/// the master would read the component as a series whose instance was edited
/// back to the parent's title, and patch that over the server's copy.
///
/// A set of instances with no master at all is refused instead — there is
/// nothing honest to send, and a visible failure beats rewriting a series to
/// look like one moved day.
///
/// # Safety
///
/// `instances` must be NULL or a valid `GSList` whose nodes are
/// `ECalComponent *`, which is what the vfunc receives.
pub unsafe fn icalendar_from_instances(instances: *const GSList) -> Option<SavedComponent> {
    // SAFETY: the caller guarantees the list's shape; the components are
    // borrowed from the ECalComponents that own them.
    let master = unsafe { find_master(instances) }?;
    // SAFETY: `master` is a valid component for as long as the list is.
    let uid = unsafe { component_uid(master) };

    // SAFETY: a fresh envelope, and clones of the instances to put in it —
    // `take_component` takes ownership, and they are not ours to give.
    let icalendar = unsafe {
        let calendar = i_cal_component_new_vcalendar();
        if calendar.is_null() {
            return None;
        }
        i_cal_component_take_component(calendar, i_cal_component_clone(master));
        for instance in instance_components(instances) {
            if !ptr::eq(instance, master) {
                i_cal_component_take_component(calendar, i_cal_component_clone(instance));
            }
        }
        let rendered = ical_from_component(calendar);
        component_unref(calendar);
        rendered?
    };
    Some(SavedComponent { uid, icalendar })
}

/// Drops a reference taken by [`component_from_ical`].
///
/// # Safety
///
/// `component` must be NULL or a valid `ICalComponent` this caller owns a
/// reference to.
pub unsafe fn component_unref(component: *mut ICalComponent) {
    if !component.is_null() {
        // SAFETY: ICalComponent is a GObject and the caller owns the reference.
        unsafe { g_object_unref(component.cast()) }
    }
}

/// Whether `component` is an event or contains one.
///
/// # Safety
///
/// `component` must be a valid `ICalComponent`.
unsafe fn holds_event(component: *mut ICalComponent) -> bool {
    // SAFETY: the caller guarantees a valid component.
    unsafe {
        if i_cal_component_isa(component) == I_CAL_VEVENT_COMPONENT {
            return true;
        }
        let event = i_cal_component_get_first_component(component, I_CAL_VEVENT_COMPONENT);
        // The returned reference is ours; we only wanted to know it exists.
        component_unref(event);
        !event.is_null()
    }
}

/// The `ICalComponent` inside each node of `instances`, in list order, borrowed
/// from the `ECalComponent` that owns it. Nodes holding nothing are skipped.
///
/// # Safety
///
/// As [`icalendar_from_instances`].
unsafe fn instance_components(instances: *const GSList) -> Vec<*mut ICalComponent> {
    let mut components = Vec::new();
    let mut node = instances;
    while !node.is_null() {
        // SAFETY: the caller guarantees a valid list of ECalComponent.
        unsafe {
            let component = (*node).data.cast::<ECalComponent>();
            node = (*node).next;
            if component.is_null() {
                continue;
            }
            // Borrowed: the ECalComponent keeps owning it.
            let inner = e_cal_component_get_icalcomponent(component);
            if !inner.is_null() {
                components.push(inner);
            }
        }
    }
    components
}

/// The first instance in `instances` that carries no `RECURRENCE-ID`, borrowed
/// from the `ECalComponent` that owns it.
///
/// # Safety
///
/// As [`icalendar_from_instances`].
unsafe fn find_master(instances: *const GSList) -> Option<*mut ICalComponent> {
    // SAFETY: the caller guarantees a valid list of ECalComponent.
    unsafe {
        instance_components(instances).into_iter().find(|inner| {
            let recurrence_id =
                i_cal_component_get_first_property(*inner, I_CAL_RECURRENCEID_PROPERTY);
            if recurrence_id.is_null() {
                return true;
            }
            // A property reference is ours to drop, like a component's.
            g_object_unref(recurrence_id.cast());
            false
        })
    }
}

/// Reads an owned `gchar *` and frees it.
///
/// # Safety
///
/// `raw` must be NULL or a GLib-allocated NUL-terminated string this caller
/// owns.
unsafe fn take_string(raw: *mut gchar) -> Option<String> {
    if raw.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees an owned NUL-terminated string.
    unsafe {
        let value = CStr::from_ptr(raw).to_string_lossy().into_owned();
        g_free(raw.cast());
        Some(value)
    }
}
