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
    ECalComponent, ETimezoneCache, I_CAL_ANY_PROPERTY, I_CAL_RECURRENCEID_PROPERTY,
    I_CAL_TZID_PARAMETER, I_CAL_TZID_PROPERTY, I_CAL_VCALENDAR_COMPONENT, I_CAL_VEVENT_COMPONENT,
    ICalComponent, ICalComponentKind, ICalTimezone, e_cal_component_get_icalcomponent,
    e_cal_meta_backend_info_new, e_timezone_cache_get_timezone, i_cal_component_as_ical_string,
    i_cal_component_clone, i_cal_component_get_first_component, i_cal_component_get_first_property,
    i_cal_component_get_next_component, i_cal_component_get_next_property,
    i_cal_component_get_timezone, i_cal_component_get_uid, i_cal_component_isa,
    i_cal_component_new_from_string, i_cal_component_new_vcalendar, i_cal_component_take_component,
    i_cal_parameter_get_tzid, i_cal_property_get_first_parameter, i_cal_property_set_tzid,
    i_cal_timezone_get_builtin_timezone, i_cal_timezone_get_builtin_timezone_from_tzid,
    i_cal_timezone_get_component, time_t,
};
use glib_sys::{
    GSList, g_date_time_format, g_date_time_new_from_unix_utc, g_date_time_unref, g_free,
    g_slist_prepend, gchar,
};
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::marshal::{dup_string, read_string};
use jmap_backend_core::owned::Owned;
use jmap_cal_sync::{ComponentInfo, FreeBusy};

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
/// Each object is handed on with the zones it refers to *defined* — see
/// [`icalendar_with_time_zones`], which is why this is not the pure marshalling
/// its address-book counterpart is.
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
        let object = cstring_lossy(&icalendar_with_time_zones(&info.icalendar));
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
///
/// What comes back defines the zones it refers to, for the reason
/// [`icalendar_with_time_zones`] gives.
pub fn component_from_ical(icalendar: &str) -> *mut ICalComponent {
    let text = cstring_lossy(icalendar);
    // SAFETY: `text` is a valid NUL-terminated string for the duration of the
    // call, which copies what it needs. The reference it hands back is ours, and
    // an `Owned` is what releases it on the path that does not return it.
    let component = unsafe { Owned::from_raw(i_cal_component_new_from_string(text.as_ptr())) };
    let Some(component) = component else {
        return ptr::null_mut();
    };
    // SAFETY: `component` holds a live reference for the rest of this scope.
    if unsafe { holds_event(component.as_ptr()) } {
        // SAFETY: as above; the component is ours and this only adds to it.
        unsafe { take_event_time_zones(component.as_ptr()) };
        // The caller takes the reference over, and drops it with
        // [`component_unref`].
        component.into_raw()
    } else {
        // Not something to hand back: the reference goes out with the scope,
        // rather than by an unref this branch has to remember.
        ptr::null_mut()
    }
}

/// `icalendar` with a `VTIMEZONE` in it for every zone its events name and
/// libical can resolve, or the text as it stands when there is none to add.
///
/// This is the outgoing half of what [`icalendar_from_instances`] does on the
/// way in, and it exists for the same clause: RFC 5545 §3.2.19 says a `TZID`
/// parameter names a `VTIMEZONE` in the *same object*. `jmap-ical` writes a
/// plain IANA name — `DTSTART;TZID=Europe/Zurich` — and no definition beside it,
/// leaning on libical resolving the name out of its builtin table. libical does;
/// nothing else has to, and an object that says a wall-clock time in an
/// undefined zone is not a calendar object. It reaches a file export, an
/// invitation forwarded on, and any reader of the EDS cache that is not libical.
///
/// Text that does not parse is handed back untouched: it is `jmap-ical`'s
/// rendering, so a failure here is a bug on this side, and refusing it is
/// [`load_component`](crate::ops::load_component)'s decision to make rather than
/// this function's to make silently.
///
/// So is text with no zone to define — it goes back **byte for byte**, because
/// defining one means rebuilding the object through libical, which respells what
/// it was given. Nothing to add, nothing rebuilt.
pub fn icalendar_with_time_zones(icalendar: &str) -> String {
    let text = cstring_lossy(icalendar);
    // SAFETY: `text` is valid for the call; the component is a fresh allocation
    // this scope owns, and the `Owned` drops it on every path out — including
    // the "not a calendar object" one that returns the text untouched.
    unsafe {
        let Some(calendar) = Owned::from_raw(i_cal_component_new_from_string(text.as_ptr())) else {
            return icalendar.to_owned();
        };
        let defined = take_event_time_zones(calendar.as_ptr());
        let rendered = if defined {
            ical_from_component(calendar.as_ptr())
        } else {
            None
        };
        rendered.unwrap_or_else(|| icalendar.to_owned())
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
/// The zones those instances refer to go in ahead of them, because a `TZID`
/// alone is not a zone: RFC 5545 §3.2.19 says it names a `VTIMEZONE` in the same
/// object, and what Evolution's editor writes is libical's own
/// `/freeassociation.sourceforge.net/Europe/Berlin`, which nothing outside
/// libical resolves. An envelope built out of the instances and nothing else —
/// which this used to be — is therefore not a calendar object, and worse than
/// malformed: the mapping cannot name the zone, so `patch::diff` leaves
/// `timeZone` alone and the zone the user chose never reaches the server. See
/// `take_referenced_time_zones`.
///
/// A set of instances with no master at all is refused instead — there is
/// nothing honest to send, and a visible failure beats rewriting a series to
/// look like one moved day.
///
/// `zones` is the calendar the instances came from, asked for the definition of
/// any zone libical's builtin table does not hold — see
/// `take_referenced_time_zones`. NULL asks nothing, which leaves such a zone
/// undefined and is what a caller with no calendar to hand gets.
///
/// # Safety
///
/// `instances` must be NULL or a valid `GSList` whose nodes are
/// `ECalComponent *`, which is what the vfunc receives, and `zones` NULL or a
/// valid `ETimezoneCache`.
pub unsafe fn icalendar_from_instances(
    instances: *const GSList,
    zones: *mut ETimezoneCache,
) -> Option<SavedComponent> {
    // SAFETY: the caller guarantees the list's shape; the components are
    // borrowed from the ECalComponents that own them.
    let components = unsafe { instance_components(instances) };
    // SAFETY: each is a valid component for as long as the list is.
    let master = unsafe { find_master(&components) }?;
    // SAFETY: `master` is one of those.
    let uid = unsafe { component_uid(master) };

    // SAFETY: a fresh envelope, and clones of the instances to put in it —
    // `take_component` takes ownership, and they are not ours to give.
    let icalendar = unsafe {
        let calendar = Owned::from_raw(i_cal_component_new_vcalendar())?;
        take_referenced_time_zones(calendar.as_ptr(), &components, zones);
        i_cal_component_take_component(calendar.as_ptr(), i_cal_component_clone(master));
        for instance in &components {
            if !ptr::eq(*instance, master) {
                i_cal_component_take_component(calendar.as_ptr(), i_cal_component_clone(*instance));
            }
        }
        // The `?` is the exit path this used to have to unref on by hand: an
        // envelope that will not render is still an envelope to release.
        ical_from_component(calendar.as_ptr())?
    };
    Some(SavedComponent { uid, icalendar })
}

/// Drops a reference taken by [`component_from_ical`].
///
/// Kept as a function because [`component_from_ical`] hands the vfuncs a raw
/// pointer — that is the shape `load_component_sync` needs — so somebody outside
/// this module still has to release one. Inside, it is the same [`Owned`] every
/// other reference here goes through, so the crate has exactly one unref site.
///
/// # Safety
///
/// `component` must be NULL or a valid `ICalComponent` this caller owns a
/// reference to.
pub unsafe fn component_unref(component: *mut ICalComponent) {
    // SAFETY: the caller guarantees NULL or an owned reference, which is
    // `Owned::from_raw`'s own contract; the drop is the unref.
    drop(unsafe { Owned::from_raw(component) });
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
        // The returned reference is ours; we only wanted to know it exists, so
        // asking the `Owned` whether there is one is also what releases it.
        Owned::from_raw(i_cal_component_get_first_component(
            component,
            I_CAL_VEVENT_COMPONENT,
        ))
        .is_some()
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

/// The first of `components` that carries no `RECURRENCE-ID`, borrowed from the
/// `ECalComponent` that owns it.
///
/// # Safety
///
/// Each of `components` must be a valid `ICalComponent`.
unsafe fn find_master(components: &[*mut ICalComponent]) -> Option<*mut ICalComponent> {
    // SAFETY: the caller guarantees valid components. A property reference is
    // ours to drop, like a component's — and the `Owned` drops it whichever
    // answer it gave, which the two-branch version had to write twice.
    unsafe {
        components.iter().copied().find(|inner| {
            Owned::from_raw(i_cal_component_get_first_property(
                *inner,
                I_CAL_RECURRENCEID_PROPERTY,
            ))
            .is_none()
        })
    }
}

/// Puts a `VTIMEZONE` into `calendar` for every zone the events *inside* it
/// name, and says whether any was added.
///
/// Only into a `VCALENDAR`: a `VTIMEZONE` is a child of the calendar object, and
/// `load_component_sync` may be asked for a bare `VEVENT`, which has nowhere to
/// put one. Such a component keeps naming a zone it does not define, which is
/// the state of everything this backend rendered before this existed and is no
/// worse than it was.
///
/// # Safety
///
/// `calendar` must be a valid `ICalComponent` this caller owns.
unsafe fn take_event_time_zones(calendar: *mut ICalComponent) -> bool {
    // SAFETY: the caller guarantees the component.
    unsafe {
        if i_cal_component_isa(calendar) != I_CAL_VCALENDAR_COMPONENT {
            return false;
        }
        let events = child_components(calendar, I_CAL_VEVENT_COMPONENT);
        // Borrowed for the call, which only reads them: the `Owned`s stay the
        // ones that release the references, at the end of this scope.
        let borrowed: Vec<*mut ICalComponent> = events.iter().map(Owned::as_ptr).collect();
        // No cache: this is the way *out*, and an object built by `jmap-ical`
        // carries the definition of any zone it names that is not an IANA one.
        take_referenced_time_zones(calendar, &borrowed, ptr::null_mut())
    }
}

/// The children of `component` of the given kind, each an owned reference which
/// goes out with the `Vec` the caller holds.
///
/// # Safety
///
/// `component` must be a valid `ICalComponent`.
unsafe fn child_components(
    component: *mut ICalComponent,
    kind: ICalComponentKind,
) -> Vec<Owned<ICalComponent>> {
    let mut children = Vec::new();
    // SAFETY: the caller guarantees the component; each reference the iteration
    // hands back is ours, and is handed on to the caller inside an `Owned` —
    // which is also the loop's NULL test, so the end of the iteration and the
    // ownership of what it produced are the same decision.
    unsafe {
        let mut child = i_cal_component_get_first_component(component, kind);
        while let Some(owned) = Owned::from_raw(child) {
            children.push(owned);
            child = i_cal_component_get_next_component(component, kind);
        }
    }
    children
}

/// Puts a `VTIMEZONE` into `calendar` for every zone `components` refer to and
/// libical can resolve, and says whether any was added.
///
/// The definition is libical's own, copied out of its builtin zone — the same
/// text Evolution writes when it saves a zoned appointment to a file — and it is
/// renamed to the identifier the properties use, so that the envelope defines
/// the zone it refers to rather than a different spelling of the same city. Its
/// `X-LIC-LOCATION` is what [`jmap_cal_sync`](jmap_cal_sync) translates
/// libical's identifier into a JSCalendar `TimeZoneId` by.
///
/// Two identifiers this deliberately leaves undefined:
/// - one no zone database knows, such as Windows' `W. Europe Standard Time`,
///   because the only honest alternatives are guessing a city or failing the
///   save, and the mapping already refuses a zone it cannot name — which keeps
///   the server's own value rather than overwriting it with a guess;
/// - `UTC`, which libical resolves and has no component for. It is the absence
///   of transition rules, not a zone with any, and there is nothing to copy.
///
/// A zone `calendar` already defines is left alone: a second copy of one
/// `VTIMEZONE` is a duplicate `TZID` in one object. The envelope a save is built
/// into is fresh and defines none, but an object that came from elsewhere may.
///
/// # Safety
///
/// `calendar` must be a valid `ICalComponent` this caller owns, and each of
/// `components` a valid `ICalComponent`.
unsafe fn take_referenced_time_zones(
    calendar: *mut ICalComponent,
    components: &[*mut ICalComponent],
    zones: *mut ETimezoneCache,
) -> bool {
    let mut defined = false;
    // SAFETY: the caller guarantees the components.
    for tzid in unsafe { referenced_tzids(components) } {
        let name = cstring_lossy(&tzid);
        // SAFETY: `name` is valid for the calls, which copy what they keep.
        // Every zone *lookup* hands back a zone its owner keeps — the library's
        // builtin table or the calendar's cache — transfer none, which is why
        // `resolve_time_zone`'s answer stays a raw pointer while everything here
        // that *is* ours is an `Owned`. That is the distinction this function had
        // to make in prose, across four early exits, before the type made it.
        unsafe {
            if defines_time_zone(calendar, name.as_ptr()) {
                continue;
            }
            let zone = resolve_time_zone(name.as_ptr(), zones);
            if zone.is_null() {
                continue;
            }
            // This reference *is* ours (transfer full), whatever the native
            // component behind it belongs to, so the clone is what goes in the
            // envelope and this goes out with the iteration.
            let Some(definition) = Owned::from_raw(i_cal_timezone_get_component(zone)) else {
                continue;
            };
            let Some(copy) = Owned::from_raw(i_cal_component_clone(definition.as_ptr())) else {
                continue;
            };
            // A definition under another name defines another zone, so a copy
            // that cannot be renamed is not put in at all — and is released by
            // its own drop, not by an `else` branch that says so.
            if rename_time_zone(copy.as_ptr(), name.as_ptr()) {
                // `take_component` takes the reference, so it is handed over
                // rather than borrowed.
                i_cal_component_take_component(calendar, copy.into_raw());
                defined = true;
            }
        }
    }
    defined
}

/// The zone `tzid` names, borrowed from whoever holds it, or NULL.
///
/// Three places, in this order, and the order is the point:
/// 1. libical's builtin table under libical's own identifier
///    (`/freeassociation.sourceforge.net/Europe/Berlin`), which is what
///    Evolution's editor writes;
/// 2. the same table under a plain IANA name;
/// 3. `zones` — the calendar the instances came from.
///
/// The table comes first because it is the zone database: for a name it knows,
/// its answer is the one every other client would give, where a calendar's copy
/// is whatever some client happened to send once and may be years stale. Only
/// what the database has never heard of falls through, which is exactly RFC 8984
/// §1.4.9's other kind of identifier — the solidus-prefixed one that resolves
/// nowhere but the document it travels in.
///
/// That kind reaches a save through the calendar and through nothing else. EDS
/// does not leave a client's `VTIMEZONE` in the component it came with: it files
/// the zone in the calendar's `ETimezoneCache` and the instance keeps naming it.
/// So an envelope built from the instances alone names a zone nothing can
/// resolve — and `jmap-ical`'s `maps_time_zone` then refuses it, which leaves the
/// server's own `timeZone` standing and the user's choice nowhere.
///
/// # Safety
///
/// `tzid` must be a valid NUL-terminated string and `zones` NULL or a valid
/// `ETimezoneCache`.
unsafe fn resolve_time_zone(tzid: *const gchar, zones: *mut ETimezoneCache) -> *mut ICalTimezone {
    // SAFETY: the caller guarantees both. Every lookup here is transfer none —
    // the builtin table and the cache each keep owning what they hand back.
    unsafe {
        let zone = i_cal_timezone_get_builtin_timezone_from_tzid(tzid);
        if !zone.is_null() {
            return zone;
        }
        let zone = i_cal_timezone_get_builtin_timezone(tzid);
        if !zone.is_null() || zones.is_null() {
            return zone;
        }
        e_timezone_cache_get_timezone(zones, tzid)
    }
}

/// Whether `calendar` already carries a `VTIMEZONE` defining `tzid`.
///
/// `i_cal_component_get_timezone` searches the object's own definitions and does
/// not fall back to the builtin table, which is exactly the question: a zone
/// libical could resolve anyway is one this object still has to define.
///
/// # Safety
///
/// `calendar` must be a valid `ICalComponent` and `tzid` a valid NUL-terminated
/// string.
unsafe fn defines_time_zone(calendar: *mut ICalComponent, tzid: *const gchar) -> bool {
    // SAFETY: the caller guarantees both; the zone comes back transfer full and
    // is released by the `Owned`'s drop — only its existence was asked about.
    unsafe { Owned::from_raw(i_cal_component_get_timezone(calendar, tzid)).is_some() }
}

/// Sets the `TZID` of the `VTIMEZONE` `definition`, or false if it has none —
/// which no `VTIMEZONE` libical builds is, and an unnamed one defines nothing.
///
/// # Safety
///
/// `definition` must be a valid `ICalComponent` and `tzid` a valid
/// NUL-terminated string.
unsafe fn rename_time_zone(definition: *mut ICalComponent, tzid: *const gchar) -> bool {
    // SAFETY: the caller guarantees both; the property reference is ours to
    // drop, and `set_tzid` copies the string.
    unsafe {
        let Some(property) = Owned::from_raw(i_cal_component_get_first_property(
            definition,
            I_CAL_TZID_PROPERTY,
        )) else {
            return false;
        };
        i_cal_property_set_tzid(property.as_ptr(), tzid);
        true
    }
}

/// Every zone `components` name, in the order first seen and without repeats: a
/// second copy of one `VTIMEZONE` would be a duplicate `TZID` in one object.
///
/// Every property of every instance is asked, rather than each instance's
/// `DTSTART`: a detached occurrence states the instant it replaces in the zone of
/// the series and may itself have been moved into another, and an `EXDATE` or an
/// `RDATE` carries a zone of its own too.
///
/// # Safety
///
/// Each of `components` must be a valid `ICalComponent`.
unsafe fn referenced_tzids(components: &[*mut ICalComponent]) -> Vec<String> {
    let mut tzids: Vec<String> = Vec::new();
    for component in components {
        // SAFETY: the caller guarantees a valid component; each property and
        // parameter reference the iteration hands back is ours to drop.
        unsafe {
            let mut next = i_cal_component_get_first_property(*component, I_CAL_ANY_PROPERTY);
            // Two nested references per property, each released by its own
            // scope: the parameter at the end of the `if let`, the property at
            // the end of the iteration. The identifier is copied out of the
            // parameter while it is still held.
            while let Some(property) = Owned::from_raw(next) {
                if let Some(parameter) = Owned::from_raw(i_cal_property_get_first_parameter(
                    property.as_ptr(),
                    I_CAL_TZID_PARAMETER,
                )) {
                    let tzid = i_cal_parameter_get_tzid(parameter.as_ptr());
                    if !tzid.is_null() {
                        let tzid = CStr::from_ptr(tzid).to_string_lossy().into_owned();
                        if !tzid.is_empty() && !tzids.contains(&tzid) {
                            tzids.push(tzid);
                        }
                    }
                }
                next = i_cal_component_get_next_property(*component, I_CAL_ANY_PROPERTY);
            }
        }
    }
    tzids
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

// ---------------------------------------------------------------------------
// free/busy

/// The addresses `get_free_busy_sync` was asked about, read out of its `GSList`
/// of `gchar *`.
///
/// EDS still owns the list and every string in it, so nothing here is freed.
/// Empty and NULL entries are dropped rather than carried: they name nobody,
/// and each one would otherwise cost a `Principal/query` round trip that can
/// only come back empty.
///
/// # Safety
///
/// `users` must be NULL or a valid `GSList` whose `data` pointers are NULL or
/// NUL-terminated strings, all valid for the duration of the call.
pub unsafe fn user_list(users: *const GSList) -> Vec<String> {
    let mut addresses = Vec::new();
    let mut node = users;
    // SAFETY: the caller guarantees a well-formed list, which ends at NULL, and
    // strings valid for the call.
    unsafe {
        while !node.is_null() {
            if let Some(user) = read_string((*node).data.cast()) {
                addresses.push(user);
            }
            node = (*node).next;
        }
    }
    addresses
}

/// The answers as the `GSList` of `gchar *` `get_free_busy_sync` hands back,
/// in the order given.
///
/// A list of plain strings, unlike [`info_list`]'s structs: the vfunc's
/// `freebusyobjs` is documented `(element-type utf8) (transfer full)`, and
/// `e_data_cal_respond_get_free_busy` reads each node as an iCalendar string.
/// Ownership of the list and of every string in it passes to the caller, which
/// frees them with `g_free`.
pub fn free_busy_list(answers: &[FreeBusy]) -> *mut GSList {
    let mut list = ptr::null_mut();
    // Prepending is the only O(1) GSList insertion, so walk backwards and the
    // result comes out in the order the caller gave.
    for answer in answers.iter().rev() {
        // SAFETY: the duplicate is a GLib allocation ownership of which passes
        // into the list, and from the list to EDS.
        let node = unsafe { dup_string(&answer.icalendar) };
        // SAFETY: `list` is a valid GSList (initially the empty one).
        list = unsafe { g_slist_prepend(list, node.cast()) };
    }
    list
}

/// A `time_t` as the JMAP `UTCDate` (RFC 3339, `Z`) that
/// `Principal/getAvailability` takes.
///
/// Through GLib rather than by hand: turning seconds-since-the-epoch into a
/// broken-down UTC date is calendar arithmetic — leap years, and the leap
/// seconds the Unix epoch does not count — and `GDateTime` already does it,
/// correctly, in a library this backend is linked against anyway. `jmap-ical`
/// and `jmap-mock` both go out of their way to avoid date arithmetic; this is
/// the one place in the calendar path that genuinely needs it, and it is
/// borrowed rather than written.
///
/// `None` for an instant outside `GDateTime`'s range (years 1 to 9999), which
/// no calendar view can scroll to; the caller chains up rather than reporting a
/// failure, since the parent takes the same `time_t` and can still answer.
pub fn utc_date(seconds: time_t) -> Option<String> {
    // `seconds` goes straight through: `time_t` and `gint64` are both `i64` on
    // every target this backend builds for, and glibc's 64-bit-`time_t`
    // transition has made that true of 32-bit Linux too. A target where they
    // differ would be a type error here rather than a silently truncated date.
    //
    // SAFETY: no preconditions; NULL is the documented answer for an
    // out-of-range instant, and the reference taken is released below.
    unsafe {
        let datetime = g_date_time_new_from_unix_utc(seconds);
        if datetime.is_null() {
            return None;
        }
        let formatted = g_date_time_format(datetime, c"%Y-%m-%dT%H:%M:%SZ".as_ptr());
        g_date_time_unref(datetime);
        take_string(formatted)
    }
}
