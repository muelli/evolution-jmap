// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Marshaling between Rust types and `ECalMetaBackend` C representations.
//! Allocates with GLib and transfers ownership to callers per EDS contracts.

use std::ffi::CStr;
use std::ptr;
use std::sync::{Mutex, PoisonError};

use eds_sys::{
    E_SOURCE_EXTENSION_CALENDAR, ECalComponent, ESource, ESourceSelectable, ETimezoneCache,
    I_CAL_ANY_PROPERTY, I_CAL_RECURRENCEID_PROPERTY, I_CAL_TZID_PARAMETER, I_CAL_TZID_PROPERTY,
    I_CAL_VCALENDAR_COMPONENT, I_CAL_VEVENT_COMPONENT, ICalComponent, ICalComponentKind,
    ICalTimezone, e_cal_component_get_icalcomponent, e_cal_meta_backend_info_new,
    e_source_get_extension, e_source_selectable_get_color, e_timezone_cache_get_timezone,
    i_cal_component_as_ical_string, i_cal_component_clone, i_cal_component_get_first_component,
    i_cal_component_get_first_property, i_cal_component_get_next_component,
    i_cal_component_get_next_property, i_cal_component_get_timezone, i_cal_component_get_uid,
    i_cal_component_isa, i_cal_component_new_from_string, i_cal_component_new_vcalendar,
    i_cal_component_take_component, i_cal_parameter_get_tzid, i_cal_property_get_first_parameter,
    i_cal_property_set_tzid, i_cal_timezone_get_builtin_timezone,
    i_cal_timezone_get_builtin_timezone_from_tzid, i_cal_timezone_get_component, time_t,
};
use glib_sys::{
    GSList, g_date_time_format, g_date_time_new_from_unix_utc, g_date_time_unref, g_free,
    g_slist_prepend, gchar,
};
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::marshal::{dup_string, read_string};
use jmap_backend_core::owned::Owned;
use jmap_cal_sync::{ComponentInfo, FreeBusy};

/// The event a save is about, extracted from instances passed by EDS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedComponent {
    /// The master's iCalendar UID. None for local creates without an existing UID.
    pub uid: Option<String>,
    /// The master wrapped in a VCALENDAR, ready for `CalSync::save_component`.
    pub icalendar: String,
}

/// Wraps `infos` as an allocated `GSList` of `ECalMetaBackendInfo`.
pub fn info_list(infos: &[ComponentInfo]) -> *mut GSList {
    let mut list = ptr::null_mut();
    // Prepending is O(1); iterate in reverse order to preserve input sequence.
    for info in infos.iter().rev() {
        let uid = cstring_lossy(&info.uid);
        let revision = cstring_lossy(&info.revision);
        let object = cstring_lossy(&icalendar_with_time_zones(&info.icalendar));
        // SAFETY: The three pointers are valid NUL-terminated strings copied by EDS.
        let node = unsafe {
            e_cal_meta_backend_info_new(
                uid.as_ptr(),
                revision.as_ptr(),
                object.as_ptr(),
                ptr::null(),
            )
        };
        // SAFETY: `list` is a valid GSList and ownership of `node` passes to it.
        list = unsafe { g_slist_prepend(list, node.cast()) };
    }
    list
}

/// Wraps removed `uids` as a `GSList` of `ECalMetaBackendInfo` with only `uid` set.
pub fn removed_info_list(uids: &[String]) -> *mut GSList {
    let mut list = ptr::null_mut();
    for uid in uids.iter().rev() {
        let uid = cstring_lossy(uid);
        // SAFETY: `uid` is valid for the call, which copies it; NULL allowed for remaining fields.
        let node = unsafe {
            e_cal_meta_backend_info_new(uid.as_ptr(), ptr::null(), ptr::null(), ptr::null())
        };
        // SAFETY: Follows safety requirements of info_list.
        list = unsafe { g_slist_prepend(list, node.cast()) };
    }
    list
}

/// Parses an iCalendar string into an `ICalComponent`, or NULL if invalid or not containing an event.
pub fn component_from_ical(icalendar: &str) -> *mut ICalComponent {
    let text = cstring_lossy(icalendar);
    // SAFETY: `text` is valid NUL-terminated string; caller receives owned reference via `into_raw`.
    let component = unsafe { Owned::from_raw(i_cal_component_new_from_string(text.as_ptr())) };
    let Some(component) = component else {
        return ptr::null_mut();
    };
    // SAFETY: `component` holds a live reference for the rest of this scope.
    if unsafe { holds_event(component.as_ptr()) } {
        // SAFETY: `component` is valid; adds embedded time zone definitions.
        unsafe { take_event_time_zones(component.as_ptr()) };
        component.into_raw()
    } else {
        ptr::null_mut()
    }
}

/// Adds `VTIMEZONE` definitions to `icalendar` for all referenced timezones, or returns original text.
pub fn icalendar_with_time_zones(icalendar: &str) -> String {
    let text = cstring_lossy(icalendar);
    // SAFETY: `text` is valid for the call; `Owned` drops the component on exit.
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
    // SAFETY: the returned string is a GLib allocation this call takes ownership of.
    unsafe { take_string(i_cal_component_as_ical_string(component)) }
}

/// Extracts the event's UID from `component`, returning `None` if missing or empty.
///
/// # Safety
///
/// `component` must be NULL or a valid `ICalComponent`.
pub unsafe fn component_uid(component: *mut ICalComponent) -> Option<String> {
    if component.is_null() {
        return None;
    }
    // SAFETY: Caller guarantees valid component; returned string is borrowed until mutated.
    let uid = unsafe { i_cal_component_get_uid(component) };
    if uid.is_null() {
        return None;
    }
    // SAFETY: Non-NULL libical string is NUL-terminated.
    let uid = unsafe { CStr::from_ptr(uid) }
        .to_string_lossy()
        .into_owned();
    (!uid.is_empty()).then_some(uid)
}

/// Extracts master component and referenced timezones from `instances` for `save_component_sync`.
///
/// # Safety
///
/// `instances` must be NULL or a valid `GSList` of `ECalComponent *`; `zones` NULL or valid `ETimezoneCache`.
pub unsafe fn icalendar_from_instances(
    instances: *const GSList,
    zones: *mut ETimezoneCache,
) -> Option<SavedComponent> {
    // SAFETY: Caller guarantees valid GSList of ECalComponent.
    let components = unsafe { instance_components(instances) };
    // SAFETY: Each component is valid for the duration of the list.
    let master = unsafe { find_master(&components) }?;
    // SAFETY: `master` is a valid component.
    let uid = unsafe { component_uid(master) };

    // SAFETY: Fresh envelope; cloned instances are transferred into the new calendar.
    let icalendar = unsafe {
        let calendar = Owned::from_raw(i_cal_component_new_vcalendar())?;
        take_referenced_time_zones(calendar.as_ptr(), &components, zones);
        i_cal_component_take_component(calendar.as_ptr(), i_cal_component_clone(master));
        for instance in &components {
            if !ptr::eq(*instance, master) {
                i_cal_component_take_component(calendar.as_ptr(), i_cal_component_clone(*instance));
            }
        }
        ical_from_component(calendar.as_ptr())?
    };
    Some(SavedComponent { uid, icalendar })
}

/// Drops a reference taken by [`component_from_ical`].
///
/// # Safety
///
/// `component` must be NULL or an owned `ICalComponent` reference.
pub unsafe fn component_unref(component: *mut ICalComponent) {
    // SAFETY: Caller guarantees NULL or owned reference dropped via Owned::from_raw.
    drop(unsafe { Owned::from_raw(component) });
}

/// Returns whether `component` is a VEVENT or contains one.
///
/// # Safety
///
/// `component` must be a valid `ICalComponent`.
unsafe fn holds_event(component: *mut ICalComponent) -> bool {
    // SAFETY: Caller guarantees valid component.
    unsafe {
        if i_cal_component_isa(component) == I_CAL_VEVENT_COMPONENT {
            return true;
        }
        Owned::from_raw(i_cal_component_get_first_component(
            component,
            I_CAL_VEVENT_COMPONENT,
        ))
        .is_some()
    }
}

/// Borrowed `ICalComponent` pointers extracted from `instances`.
///
/// # Safety
///
/// Follows safety requirements of [`icalendar_from_instances`].
unsafe fn instance_components(instances: *const GSList) -> Vec<*mut ICalComponent> {
    let mut components = Vec::new();
    let mut node = instances;
    while !node.is_null() {
        // SAFETY: Caller guarantees valid list of ECalComponent.
        unsafe {
            let component = (*node).data.cast::<ECalComponent>();
            node = (*node).next;
            if component.is_null() {
                continue;
            }
            let inner = e_cal_component_get_icalcomponent(component);
            if !inner.is_null() {
                components.push(inner);
            }
        }
    }
    components
}

/// Finds the master component carrying no `RECURRENCE-ID` in `components`.
///
/// # Safety
///
/// Each item in `components` must be a valid `ICalComponent`.
unsafe fn find_master(components: &[*mut ICalComponent]) -> Option<*mut ICalComponent> {
    // SAFETY: Caller guarantees valid components; property reference is dropped via Owned.
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

/// Adds `VTIMEZONE` components into `calendar` for referenced event timezones.
///
/// # Safety
///
/// `calendar` must be a valid, owned `ICalComponent`.
unsafe fn take_event_time_zones(calendar: *mut ICalComponent) -> bool {
    // SAFETY: Caller guarantees valid component.
    unsafe {
        if i_cal_component_isa(calendar) != I_CAL_VCALENDAR_COMPONENT {
            return false;
        }
        let events = child_components(calendar, I_CAL_VEVENT_COMPONENT);
        let borrowed: Vec<*mut ICalComponent> = events.iter().map(Owned::as_ptr).collect();
        take_referenced_time_zones(calendar, &borrowed, ptr::null_mut())
    }
}

/// Returns owned child components of `component` matching `kind`.
///
/// # Safety
///
/// `component` must be a valid `ICalComponent`.
unsafe fn child_components(
    component: *mut ICalComponent,
    kind: ICalComponentKind,
) -> Vec<Owned<ICalComponent>> {
    let mut children = Vec::new();
    // SAFETY: Caller guarantees valid component; each returned child is wrapped in Owned.
    unsafe {
        let mut child = i_cal_component_get_first_component(component, kind);
        while let Some(owned) = Owned::from_raw(child) {
            children.push(owned);
            child = i_cal_component_get_next_component(component, kind);
        }
    }
    children
}

/// Resolves and embeds `VTIMEZONE` definitions into `calendar` for `components`.
///
/// # Safety
///
/// `calendar` must be an owned valid component; each item in `components` valid.
unsafe fn take_referenced_time_zones(
    calendar: *mut ICalComponent,
    components: &[*mut ICalComponent],
    zones: *mut ETimezoneCache,
) -> bool {
    let mut defined = false;
    // SAFETY: Caller guarantees valid components.
    for tzid in unsafe { referenced_tzids(components) } {
        let name = cstring_lossy(&tzid);
        // SAFETY: Lookups borrow timezone references; definitions are cloned and taken into calendar.
        unsafe {
            if defines_time_zone(calendar, name.as_ptr()) {
                continue;
            }
            let zone = resolve_time_zone(name.as_ptr(), zones);
            if zone.is_null() {
                continue;
            }
            let Some(definition) = Owned::from_raw(i_cal_timezone_get_component(zone)) else {
                continue;
            };
            let Some(copy) = Owned::from_raw(i_cal_component_clone(definition.as_ptr())) else {
                continue;
            };
            if rename_time_zone(copy.as_ptr(), name.as_ptr()) {
                i_cal_component_take_component(calendar, copy.into_raw());
                defined = true;
            }
        }
    }
    defined
}

/// Resolves timezone `tzid` from builtin database or `zones` cache.
///
/// # Safety
///
/// `tzid` must be a valid NUL-terminated string; `zones` NULL or valid `ETimezoneCache`.
unsafe fn resolve_time_zone(tzid: *const gchar, zones: *mut ETimezoneCache) -> *mut ICalTimezone {
    // SAFETY: Caller guarantees arguments; returned timezone is borrowed from cache/table.
    unsafe {
        let zone = {
            let _table = BUILTIN_ZONES.lock().unwrap_or_else(PoisonError::into_inner);
            let zone = i_cal_timezone_get_builtin_timezone_from_tzid(tzid);
            if zone.is_null() {
                i_cal_timezone_get_builtin_timezone(tzid)
            } else {
                zone
            }
        };
        if !zone.is_null() || zones.is_null() {
            return zone;
        }
        e_timezone_cache_get_timezone(zones, tzid)
    }
}

/// Fills libical's builtin zone table from `zone.tab`.
///
/// libical 3.0 publishes the table before it fills it, and a lookup takes no lock
/// when the table exists. EDS looks up zones without our lock, so call this
/// before a second thread can look up a zone.
pub fn load_builtin_zones() {
    let _table = BUILTIN_ZONES.lock().unwrap_or_else(PoisonError::into_inner);
    // SAFETY: the name is a NUL-terminated literal; the borrowed zone is not used.
    unsafe { i_cal_timezone_get_builtin_timezone(c"UTC".as_ptr()) };
}

/// Serializes this crate's lookups in libical's builtin zone table.
///
/// libical 3.0 adds a zone that `zone.tab` does not list (for example `Etc/UTC`)
/// to that table without its lock. A parallel lookup then reads a half-written entry.
static BUILTIN_ZONES: Mutex<()> = Mutex::new(());

/// Checks whether `calendar` defines `tzid`.
///
/// # Safety
///
/// `calendar` must be a valid `ICalComponent`; `tzid` a valid NUL-terminated string.
unsafe fn defines_time_zone(calendar: *mut ICalComponent, tzid: *const gchar) -> bool {
    // SAFETY: Caller guarantees arguments; timezone reference is dropped via Owned.
    unsafe { Owned::from_raw(i_cal_component_get_timezone(calendar, tzid)).is_some() }
}

/// Sets the TZID property on `definition`.
///
/// # Safety
///
/// `definition` must be a valid `ICalComponent`; `tzid` a valid NUL-terminated string.
unsafe fn rename_time_zone(definition: *mut ICalComponent, tzid: *const gchar) -> bool {
    // SAFETY: Caller guarantees arguments; property dropped via Owned.
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

/// Collects unique TZID strings referenced across `components`.
///
/// # Safety
///
/// Each item in `components` must be a valid `ICalComponent`.
unsafe fn referenced_tzids(components: &[*mut ICalComponent]) -> Vec<String> {
    let mut tzids: Vec<String> = Vec::new();
    for component in components {
        // SAFETY: Caller guarantees valid component; property/parameter references dropped via Owned.
        unsafe {
            let mut next = i_cal_component_get_first_property(*component, I_CAL_ANY_PROPERTY);
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
/// `raw` must be NULL or a GLib-allocated NUL-terminated string owned by caller.
unsafe fn take_string(raw: *mut gchar) -> Option<String> {
    if raw.is_null() {
        return None;
    }
    // SAFETY: Caller guarantees owned NUL-terminated string.
    unsafe {
        let value = CStr::from_ptr(raw).to_string_lossy().into_owned();
        g_free(raw.cast());
        Some(value)
    }
}

// ---------------------------------------------------------------------------
// free/busy

/// Reads user email addresses from `users` GSList for `get_free_busy_sync`.
///
/// # Safety
///
/// `users` must be NULL or a valid `GSList` of NUL-terminated strings.
pub unsafe fn user_list(users: *const GSList) -> Vec<String> {
    let mut addresses = Vec::new();
    let mut node = users;
    // SAFETY: Caller guarantees well-formed list with valid strings.
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

/// Formats free/busy answers into an allocated GSList of iCalendar strings.
pub fn free_busy_list(answers: &[FreeBusy]) -> *mut GSList {
    let mut list = ptr::null_mut();
    // Prepending is O(1); iterate in reverse order to preserve sequence.
    for answer in answers.iter().rev() {
        // SAFETY: dup_string allocates with GLib; ownership passes into list.
        let node = unsafe { dup_string(&answer.icalendar) };
        // SAFETY: `list` is a valid GSList.
        list = unsafe { g_slist_prepend(list, node.cast()) };
    }
    list
}

// ---------------------------------------------------------------------------
// the ESource itself

/// Reads calendar color from `source`'s "Calendar" extension.
///
/// # Safety
///
/// `source` must be a valid `ESource` carrying a "Calendar" extension.
pub unsafe fn selectable_color(source: *mut ESource) -> Option<String> {
    // SAFETY: Caller guarantees source carries the extension; returned string is borrowed.
    unsafe {
        let selectable: *mut ESourceSelectable =
            e_source_get_extension(source, E_SOURCE_EXTENSION_CALENDAR.as_ptr()).cast();
        read_string(e_source_selectable_get_color(selectable))
    }
}

/// Formats seconds since epoch as an RFC 3339 UTC date string.
pub fn utc_date(seconds: time_t) -> Option<String> {
    // SAFETY: NULL returned if out of GDateTime range; datetime reference released below.
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
