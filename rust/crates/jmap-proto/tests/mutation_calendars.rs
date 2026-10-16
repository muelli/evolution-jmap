// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeMap;

use jmap_proto::Id;
use jmap_proto::calendars::{
    Alert, Calendar, CalendarAlert, CalendarEvent, CalendarEventNotification,
    CalendarEventNotificationQueryFilter, CalendarEventQueryFilter, CalendarGroup,
    CalendarPreferences, CalendarPreferencesCapability, CalendarRights, CalendarsCapability,
    GroupEntries, Link, Location, NDay, OffsetTrigger, Participant, ParticipantIdentity,
    ParticipantProblem, RecurrenceRule, SendCalendarEvent, SendCalendarEventResult,
    VirtualLocation, participant_problem_kind,
};
use jmap_proto::principals::Principal;
use jmap_proto::state::UtcDate;
use serde_json::json;

#[test]
fn calendar_builders_properties_and_extra_preservation() {
    let mut extra = BTreeMap::new();
    extra.insert("customVendorCalendarFlag".to_string(), json!("premium"));
    extra.insert("syncIntervalSeconds".to_string(), json!(300));

    let mut calendar = Calendar::new("Work Calendar")
        .with_description("Primary work schedule")
        .with_color("#225588")
        .with_sort_order(10)
        .is_default(true)
        .is_subscribed(true)
        .is_visible(false)
        .with_time_zone("Europe/Berlin")
        .with_extra(extra);
    calendar.id = Some(Id::from("cal-test-1"));

    assert_eq!(calendar.id, Some(Id::from("cal-test-1")));
    assert_eq!(calendar.name, "Work Calendar");
    assert_eq!(
        calendar.description.as_deref(),
        Some("Primary work schedule")
    );
    assert_eq!(calendar.color.as_deref(), Some("#225588"));
    assert_eq!(calendar.sort_order, Some(10));
    assert_eq!(calendar.is_default, Some(true));
    assert_eq!(calendar.is_subscribed, Some(true));
    assert_eq!(calendar.is_visible, Some(false));
    assert_eq!(calendar.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(
        calendar.extra.get("customVendorCalendarFlag"),
        Some(&json!("premium"))
    );
    assert_eq!(calendar.extra.get("syncIntervalSeconds"), Some(&json!(300)));

    let val = serde_json::to_value(&calendar).expect("serialize Calendar");
    assert_eq!(val["id"], "cal-test-1");
    assert_eq!(val["name"], "Work Calendar");
    assert_eq!(val["description"], "Primary work schedule");
    assert_eq!(val["color"], "#225588");
    assert_eq!(val["sortOrder"], 10);
    assert_eq!(val["isDefault"], true);
    assert_eq!(val["isSubscribed"], true);
    assert_eq!(val["isVisible"], false);
    assert_eq!(val["timeZone"], "Europe/Berlin");
    assert_eq!(val["customVendorCalendarFlag"], "premium");
    assert_eq!(val["syncIntervalSeconds"], 300);

    let back: Calendar = serde_json::from_value(val).expect("deserialize Calendar");
    assert_eq!(back, calendar);
}

#[test]
fn calendar_rights_presets_permissions_logic_and_extra_preservation() {
    let all = CalendarRights::all();
    assert_eq!(all.may_read_free_busy, Some(true));
    assert_eq!(all.may_read_items, Some(true));
    assert_eq!(all.may_write_all, Some(true));
    assert_eq!(all.may_write_own, Some(true));
    assert_eq!(all.may_update_private, Some(true));
    assert_eq!(all.may_rsvp, Some(true));
    assert_eq!(all.may_share, Some(true));
    assert_eq!(all.may_delete, Some(true));
    assert!(all.is_writable());

    let ro = CalendarRights::read_only();
    assert_eq!(ro.may_read_items, Some(true));
    assert_eq!(ro.may_read_free_busy, Some(true));
    assert_eq!(ro.may_write_all, None);
    assert_eq!(ro.may_write_own, None);
    assert_eq!(ro.may_update_private, None);
    assert_eq!(ro.may_rsvp, None);
    assert_eq!(ro.may_share, None);
    assert_eq!(ro.may_delete, None);
    assert!(!ro.is_writable());

    let mut logic_test = CalendarRights::default();
    assert!(!logic_test.is_writable());

    logic_test.may_write_all = Some(true);
    logic_test.may_write_own = Some(false);
    assert!(logic_test.is_writable());

    logic_test.may_write_all = Some(false);
    logic_test.may_write_own = Some(true);
    assert!(logic_test.is_writable());

    logic_test.may_write_all = Some(false);
    logic_test.may_write_own = Some(false);
    assert!(!logic_test.is_writable());

    logic_test.may_write_all = None;
    logic_test.may_write_own = None;
    assert!(!logic_test.is_writable());

    logic_test.may_write_all = Some(true);
    logic_test.may_write_own = Some(true);
    assert!(logic_test.is_writable());

    let mut extra = BTreeMap::new();
    extra.insert("vendorAdminOverride".to_string(), json!(true));
    let with_extra = all.with_extra(extra);
    assert_eq!(with_extra.may_read_free_busy, Some(true));
    assert_eq!(with_extra.may_read_items, Some(true));
    assert_eq!(with_extra.may_write_all, Some(true));
    assert_eq!(with_extra.may_write_own, Some(true));
    assert_eq!(
        with_extra.extra.get("vendorAdminOverride"),
        Some(&json!(true))
    );

    let val = serde_json::to_value(&with_extra).expect("serialize CalendarRights");
    let back: CalendarRights = serde_json::from_value(val).expect("deserialize CalendarRights");
    assert_eq!(back, with_extra);
}

#[test]
fn calendar_event_simple_constructor_and_fields() {
    let event = CalendarEvent::simple(
        "cal-proj-1",
        "Architecture Review",
        "2026-07-01T14:00:00",
        "PT1H30M",
    );

    let ids = event.calendar_ids.as_ref().expect("calendar_ids present");
    assert_eq!(ids.get(&Id::from("cal-proj-1")), Some(&true));
    assert_eq!(event.event_type.as_deref(), Some("Event"));
    assert_eq!(event.version.as_deref(), Some("2.0"));
    assert_eq!(event.title.as_deref(), Some("Architecture Review"));
    assert_eq!(event.start.as_deref(), Some("2026-07-01T14:00:00"));
    assert_eq!(event.time_zone.as_deref(), Some("Etc/UTC"));
    assert_eq!(event.duration.as_deref(), Some("PT1H30M"));
    assert_eq!(event.status.as_deref(), Some("confirmed"));
}

#[test]
fn calendar_event_builders_and_bis_properties() {
    let mut extra = BTreeMap::new();
    extra.insert("customClassification".to_string(), json!("confidential"));

    let rule = RecurrenceRule::new("daily").with_interval(2);
    let updated = UtcDate::new("2026-08-01T10:00:00Z");

    let event = CalendarEvent::default()
        .with_id("ev-custom-1")
        .with_calendar_id("cal-custom-1")
        .with_uid("uid-custom-1")
        .with_title("Security Sync")
        .with_description("Bi-daily security review")
        .with_start("2026-08-01T09:00:00")
        .with_time_zone("Europe/London")
        .with_duration("PT45M")
        .with_status("confirmed")
        .with_free_busy_status("busy")
        .with_priority(1)
        .with_privacy("private")
        .show_without_time(false)
        .use_default_alerts(true)
        .with_color("#e6194b")
        .with_locale("en-GB")
        .with_recurrence_rule(rule.clone())
        .with_end_time_zone("Europe/London")
        .with_main_location_id("loc-hq-1")
        .with_schedule_sequence(4)
        .with_schedule_updated(updated.clone())
        .may_invite_self(true)
        .may_invite_others(false)
        .with_extra(extra);

    assert_eq!(event.id, Some(Id::from("ev-custom-1")));
    let ids = event.calendar_ids.as_ref().expect("calendar_ids present");
    assert_eq!(ids.get(&Id::from("cal-custom-1")), Some(&true));
    assert_eq!(event.uid.as_deref(), Some("uid-custom-1"));
    assert_eq!(event.title.as_deref(), Some("Security Sync"));
    assert_eq!(
        event.description.as_deref(),
        Some("Bi-daily security review")
    );
    assert_eq!(event.start.as_deref(), Some("2026-08-01T09:00:00"));
    assert_eq!(event.time_zone.as_deref(), Some("Europe/London"));
    assert_eq!(event.duration.as_deref(), Some("PT45M"));
    assert_eq!(event.status.as_deref(), Some("confirmed"));
    assert_eq!(event.free_busy_status.as_deref(), Some("busy"));
    assert_eq!(event.priority, Some(1));
    assert_eq!(event.privacy.as_deref(), Some("private"));
    assert_eq!(event.show_without_time, Some(false));
    assert_eq!(event.use_default_alerts, Some(true));
    assert_eq!(event.color.as_deref(), Some("#e6194b"));
    assert_eq!(event.locale.as_deref(), Some("en-GB"));
    assert_eq!(event.recurrence_rule.as_ref(), Some(&rule));
    assert_eq!(event.end_time_zone.as_deref(), Some("Europe/London"));
    assert_eq!(event.main_location_id, Some(Id::from("loc-hq-1")));
    assert_eq!(event.schedule_sequence, Some(4));
    assert_eq!(event.schedule_updated, Some(updated));
    assert_eq!(event.may_invite_self, Some(true));
    assert_eq!(event.may_invite_others, Some(false));
    assert_eq!(
        event.extra.get("customClassification"),
        Some(&json!("confidential"))
    );

    let val = serde_json::to_value(&event).expect("serialize CalendarEvent");
    assert_eq!(val["id"], "ev-custom-1");
    assert_eq!(val["title"], "Security Sync");
    assert_eq!(val["scheduleSequence"], 4);
    assert_eq!(val["mayInviteSelf"], true);
    assert_eq!(val["mayInviteOthers"], false);
    assert_eq!(val["customClassification"], "confidential");

    let back: CalendarEvent = serde_json::from_value(val).expect("deserialize CalendarEvent");
    assert_eq!(back, event);
}

#[test]
fn nday_and_recurrence_rule_builders_and_properties() {
    let nday = NDay::new("we").with_nth_of_period(3);
    assert_eq!(nday.day, "we");
    assert_eq!(nday.nth_of_period, Some(3));

    let rule = RecurrenceRule::new("monthly")
        .with_interval(2)
        .with_count(12)
        .with_until("2026-12-31T23:59:59Z")
        .with_by_day(vec![nday.clone()])
        .with_by_month(vec!["1", "3", "5", "7", "9", "11"])
        .with_rscale("gregorian")
        .with_skip("backward");

    assert_eq!(rule.frequency, "monthly");
    assert_eq!(rule.interval, Some(2));
    assert_eq!(rule.count, Some(12));
    assert_eq!(rule.until.as_deref(), Some("2026-12-31T23:59:59Z"));
    assert_eq!(rule.by_day.as_deref(), Some(&[nday][..]));
    assert_eq!(
        rule.by_month.as_deref(),
        Some(
            &[
                "1".to_string(),
                "3".to_string(),
                "5".to_string(),
                "7".to_string(),
                "9".to_string(),
                "11".to_string()
            ][..]
        )
    );
    assert_eq!(rule.rscale.as_deref(), Some("gregorian"));
    assert_eq!(rule.skip.as_deref(), Some("backward"));

    let val = serde_json::to_value(&rule).expect("serialize RecurrenceRule");
    assert_eq!(val["frequency"], "monthly");
    assert_eq!(val["interval"], 2);
    assert_eq!(val["count"], 12);
    assert_eq!(val["skip"], "backward");

    let back: RecurrenceRule = serde_json::from_value(val).expect("deserialize RecurrenceRule");
    assert_eq!(back, rule);
}

#[test]
fn calendar_event_query_filter_constructors_and_builders() {
    let in_cal = CalendarEventQueryFilter::in_calendar("cal-filter-9");
    assert_eq!(in_cal.in_calendar, Some(Id::from("cal-filter-9")));
    assert_eq!(in_cal.after, None);
    assert_eq!(in_cal.before, None);

    let time_range =
        CalendarEventQueryFilter::time_range("2026-01-01T00:00:00Z", "2026-12-31T23:59:59Z");
    assert_eq!(time_range.after.as_deref(), Some("2026-01-01T00:00:00Z"));
    assert_eq!(time_range.before.as_deref(), Some("2026-12-31T23:59:59Z"));
    assert_eq!(time_range.in_calendar, None);

    let filter = CalendarEventQueryFilter::default()
        .with_time_range("2026-05-01T00:00:00Z", "2026-05-31T23:59:59Z")
        .after("2026-06-01T00:00:00Z")
        .before("2026-06-30T23:59:59Z")
        .text("planning")
        .title("Quarterly Planning")
        .description("Q3 deliverables")
        .location("Main Auditorium")
        .uid("uid-query-target-1");

    assert_eq!(filter.after.as_deref(), Some("2026-06-01T00:00:00Z"));
    assert_eq!(filter.before.as_deref(), Some("2026-06-30T23:59:59Z"));
    assert_eq!(filter.text.as_deref(), Some("planning"));
    assert_eq!(filter.title.as_deref(), Some("Quarterly Planning"));
    assert_eq!(filter.description.as_deref(), Some("Q3 deliverables"));
    assert_eq!(filter.location.as_deref(), Some("Main Auditorium"));
    assert_eq!(filter.uid.as_deref(), Some("uid-query-target-1"));

    let val = serde_json::to_value(&filter).expect("serialize CalendarEventQueryFilter");
    assert_eq!(val["text"], "planning");
    assert_eq!(val["title"], "Quarterly Planning");
    assert_eq!(val["uid"], "uid-query-target-1");

    let back: CalendarEventQueryFilter =
        serde_json::from_value(val).expect("deserialize CalendarEventQueryFilter");
    assert_eq!(back, filter);
}

#[test]
fn participant_serde_and_builders() {
    let mut roles = BTreeMap::new();
    roles.insert("attendee".to_string(), true);
    roles.insert("chair".to_string(), true);

    let mut extra = BTreeMap::new();
    extra.insert("department".to_string(), json!("Engineering"));

    let participant = Participant::new("Alice Smith", "alice@example.com")
        .with_roles(roles.clone())
        .with_participation_status("accepted")
        .with_participation_comment("Attending in person")
        .with_attendance("required")
        .expect_reply(true)
        .with_schedule_agent("client")
        .with_calendar_address("mailto:alice@example.com")
        .with_description_content_type("text/plain")
        .with_send_to_method("other", "mailto:alice-backup@example.com")
        .with_extra(extra);

    assert_eq!(participant.participant_type.as_deref(), Some("Participant"));
    assert_eq!(participant.name.as_deref(), Some("Alice Smith"));
    assert_eq!(participant.email.as_deref(), Some("alice@example.com"));
    assert_eq!(participant.roles.as_ref(), Some(&roles));
    assert_eq!(
        participant.participation_status.as_deref(),
        Some("accepted")
    );
    assert_eq!(
        participant.participation_comment.as_deref(),
        Some("Attending in person")
    );
    assert_eq!(participant.attendance.as_deref(), Some("required"));
    assert_eq!(participant.expect_reply, Some(true));
    assert_eq!(participant.schedule_agent.as_deref(), Some("client"));
    assert_eq!(
        participant.calendar_address.as_deref(),
        Some("mailto:alice@example.com")
    );
    assert_eq!(
        participant.description_content_type.as_deref(),
        Some("text/plain")
    );
    let send_to = participant.send_to.as_ref().expect("send_to present");
    assert_eq!(
        send_to.get("other").map(String::as_str),
        Some("mailto:alice-backup@example.com")
    );
    assert_eq!(
        participant.extra.get("department"),
        Some(&json!("Engineering"))
    );

    let val = serde_json::to_value(&participant).expect("serialize Participant");
    assert_eq!(val["@type"], "Participant");
    assert_eq!(val["name"], "Alice Smith");
    assert_eq!(val["email"], "alice@example.com");
    assert_eq!(val["calendarAddress"], "mailto:alice@example.com");
    assert_eq!(val["attendance"], "required");
    assert_eq!(val["expectReply"], true);
    assert_eq!(val["department"], "Engineering");

    let back: Participant = serde_json::from_value(val).expect("deserialize Participant");
    assert_eq!(back.name.as_deref(), Some("Alice Smith"));
    assert_eq!(back.email.as_deref(), Some("alice@example.com"));
    assert_eq!(
        back.calendar_address.as_deref(),
        Some("mailto:alice@example.com")
    );
    assert_eq!(back.attendance.as_deref(), Some("required"));
    assert_eq!(back.expect_reply, Some(true));
}

#[test]
fn location_virtual_location_link_alert_builders() {
    let loc = Location::new("Innovation Lab")
        .with_description("Innovation Lab, 3rd Floor")
        .with_time_zone("Europe/Paris")
        .with_coordinates("geo:48.8566,2.3522");

    assert_eq!(loc.location_type.as_deref(), Some("Location"));
    assert_eq!(loc.name.as_deref(), Some("Innovation Lab"));
    assert_eq!(
        loc.description.as_deref(),
        Some("Innovation Lab, 3rd Floor")
    );
    assert_eq!(loc.time_zone.as_deref(), Some("Europe/Paris"));
    assert_eq!(loc.coordinates.as_deref(), Some("geo:48.8566,2.3522"));

    let mut vloc_features = BTreeMap::new();
    vloc_features.insert("audio".to_string(), true);
    vloc_features.insert("video".to_string(), true);

    let vloc = VirtualLocation::new("https://meet.example.com/conf")
        .with_name("Virtual Conference Room")
        .with_description("Join via WebRTC")
        .with_features(vloc_features.clone());

    assert_eq!(
        vloc.virtual_location_type.as_deref(),
        Some("VirtualLocation")
    );
    assert_eq!(vloc.uri, "https://meet.example.com/conf");
    assert_eq!(vloc.name.as_deref(), Some("Virtual Conference Room"));
    assert_eq!(vloc.description.as_deref(), Some("Join via WebRTC"));
    assert_eq!(vloc.features, Some(vloc_features));

    let mut link_display = BTreeMap::new();
    link_display.insert("badge".to_string(), true);

    let link = Link::new("https://example.com/brief.pdf")
        .with_content_type("application/pdf")
        .with_size(65536)
        .with_rel("enclosure")
        .with_display(link_display.clone())
        .with_title("Meeting Brief");

    assert_eq!(link.link_type.as_deref(), Some("Link"));
    assert_eq!(link.href, "https://example.com/brief.pdf");
    assert_eq!(link.content_type.as_deref(), Some("application/pdf"));
    assert_eq!(link.size, Some(65536));
    assert_eq!(link.rel.as_deref(), Some("enclosure"));
    assert_eq!(link.display, Some(link_display));
    assert_eq!(link.title.as_deref(), Some("Meeting Brief"));

    let offset = OffsetTrigger::new("-PT15M").relative_to("end");
    assert_eq!(offset.trigger_type.as_deref(), Some("OffsetTrigger"));
    assert_eq!(offset.offset, "-PT15M");
    assert_eq!(offset.relative_to.as_deref(), Some("end"));

    let ack = UtcDate::new("2026-06-01T11:45:00Z");
    let alert = Alert::new(
        "display",
        json!({"@type": "OffsetTrigger", "offset": "-PT15M"}),
    )
    .with_acknowledged(ack.clone())
    .with_related_to("start");

    assert_eq!(alert.alert_type.as_deref(), Some("Alert"));
    assert_eq!(alert.action.as_deref(), Some("display"));
    assert_eq!(alert.acknowledged, Some(ack));
    assert_eq!(alert.related_to.as_deref(), Some("start"));

    let val = serde_json::to_value(&alert).expect("serialize Alert");
    let back: Alert = serde_json::from_value(val).expect("deserialize Alert");
    assert_eq!(back, alert);
}

#[test]
fn calendar_preferences_and_capabilities_builders() {
    let pref = CalendarPreferences::new()
        .with_id("pref-user-1")
        .with_time_zone("America/Chicago")
        .with_first_day_of_week("su");

    assert_eq!(pref.id, Some(Id::from("pref-user-1")));
    assert_eq!(pref.time_zone.as_deref(), Some("America/Chicago"));
    assert_eq!(pref.first_day_of_week.as_deref(), Some("su"));

    let pref_cap = CalendarPreferencesCapability::new().with_extra(json!({"extendedPref": true}));
    assert_eq!(pref_cap.extra.get("extendedPref"), Some(&json!(true)));

    let mut cal_extra = BTreeMap::new();
    cal_extra.insert("customVendorQuota".to_string(), json!(1000));
    let cal_cap = CalendarsCapability::new()
        .with_max_size_attachments_per_event(52428800)
        .with_max_concurrent_availabilities(16)
        .with_extra(cal_extra);

    assert_eq!(cal_cap.max_size_attachments_per_event, 52428800);
    assert_eq!(cal_cap.max_concurrent_availabilities, 16);
    assert_eq!(cal_cap.extra.get("customVendorQuota"), Some(&json!(1000)));

    let val = serde_json::to_value(&cal_cap).expect("serialize CalendarsCapability");
    let back: CalendarsCapability =
        serde_json::from_value(val).expect("deserialize CalendarsCapability");
    assert_eq!(back, cal_cap);
}

#[test]
fn calendar_group_builders_and_entries() {
    let mut keywords = BTreeMap::new();
    keywords.insert("tech".to_string(), json!(true));

    let mut categories = BTreeMap::new();
    categories.insert("development".to_string(), true);

    let mut links = BTreeMap::new();
    links.insert(
        "homepage".to_string(),
        json!({"href": "https://dev.example.org"}),
    );

    let updated = UtcDate::new("2026-06-01T15:00:00Z");
    let entries = GroupEntries::from(vec![json!({"id": "event-1"}), json!({"id": "event-2"})]);

    let group = CalendarGroup::new("Developer Conference 2026")
        .with_id("grp-dev-2026")
        .with_uid("uid-grp-dev-2026")
        .with_title("DevConf 2026")
        .with_description("Annual Developer Assembly")
        .with_time_zone("Europe/Amsterdam")
        .with_updated(updated.clone())
        .with_entries(entries)
        .with_keywords(keywords.clone())
        .with_categories(categories.clone())
        .with_color("#3b5998")
        .with_source("https://devconf.example.org/calendar.ics")
        .with_links(links.clone());

    assert_eq!(group.group_type.as_deref(), Some("Group"));
    assert_eq!(group.id, Some(Id::from("grp-dev-2026")));
    assert_eq!(group.uid.as_deref(), Some("uid-grp-dev-2026"));
    assert_eq!(group.title.as_deref(), Some("DevConf 2026"));
    assert_eq!(
        group.description.as_deref(),
        Some("Annual Developer Assembly")
    );
    assert_eq!(group.time_zone.as_deref(), Some("Europe/Amsterdam"));
    assert_eq!(group.updated, Some(updated));
    assert_eq!(group.keywords, Some(keywords));
    assert_eq!(group.categories, Some(categories));
    assert_eq!(group.color.as_deref(), Some("#3b5998"));
    assert_eq!(
        group.source.as_deref(),
        Some("https://devconf.example.org/calendar.ics")
    );
    assert_eq!(group.links, Some(links));

    let val = serde_json::to_value(&group).expect("serialize CalendarGroup");
    assert_eq!(val["@type"], "Group");
    assert_eq!(val["id"], "grp-dev-2026");
    assert_eq!(val["color"], "#3b5998");

    let back: CalendarGroup = serde_json::from_value(val).expect("deserialize CalendarGroup");
    assert_eq!(back.id, Some(Id::from("grp-dev-2026")));
    assert_eq!(back.title.as_deref(), Some("DevConf 2026"));
}

#[test]
fn default_calendar_alert_and_event_notification_types_serde() {
    let alert_json = json!({
        "accountId": "acc-alert-1",
        "calendarId": "cal-alert-1",
        "calendarEventId": "ev-alert-1",
        "alertId": "al-1"
    });
    let alert: CalendarAlert =
        serde_json::from_value(alert_json).expect("deserialize CalendarAlert");
    assert_eq!(alert.kind, CalendarAlert::TYPE);
    assert_eq!(alert.kind, "CalendarAlert");

    let notif_json = json!({
        "created": "2026-06-01T12:00:00Z",
        "eventId": "ev-notif-99"
    });
    let notif: CalendarEventNotification =
        serde_json::from_value(notif_json).expect("deserialize CalendarEventNotification");
    assert_eq!(notif.kind, CalendarEventNotification::TYPE);
    assert_eq!(notif.kind, "CalendarEventNotification");
}

#[test]
fn calendar_event_notification_and_query_filter_builders() {
    let created = UtcDate::new("2026-06-01T12:00:00Z");
    let principal = Principal::new("Automated Sync");
    let event = CalendarEvent::simple("cal-notif-1", "Review", "2026-06-15T10:00:00", "PT30M");

    let notif = CalendarEventNotification::new(created.clone(), "ev-target-1")
        .with_id("notif-target-1")
        .with_kind("CalendarEventNotification")
        .with_changed_by(principal.clone())
        .with_comment("Rescheduled by coordinator")
        .with_notification_type("eventUpdated")
        .with_recurrence_id("2026-06-15T10:00:00")
        .with_event(event.clone());

    assert_eq!(notif.id, Some(Id::from("notif-target-1")));
    assert_eq!(notif.kind, "CalendarEventNotification");
    assert_eq!(notif.created, created);
    assert_eq!(notif.event_id, Id::from("ev-target-1"));
    assert_eq!(notif.changed_by, Some(principal));
    assert_eq!(notif.comment.as_deref(), Some("Rescheduled by coordinator"));
    assert_eq!(notif.notification_type.as_deref(), Some("eventUpdated"));
    assert_eq!(notif.recurrence_id.as_deref(), Some("2026-06-15T10:00:00"));
    assert_eq!(notif.event, Some(event));

    let after_date = UtcDate::new("2026-06-01T00:00:00Z");
    let before_date = UtcDate::new("2026-06-30T23:59:59Z");
    let filter = CalendarEventNotificationQueryFilter::default()
        .with_after(after_date.clone())
        .with_before(before_date.clone())
        .with_types(vec!["eventCreated".to_string(), "eventUpdated".to_string()]);

    assert_eq!(filter.after, Some(after_date));
    assert_eq!(filter.before, Some(before_date));
    assert_eq!(
        filter.types.as_deref(),
        Some(&["eventCreated".to_string(), "eventUpdated".to_string()][..])
    );

    let val = serde_json::to_value(&filter).expect("serialize filter");
    let back: CalendarEventNotificationQueryFilter =
        serde_json::from_value(val).expect("deserialize filter");
    assert_eq!(back, filter);
}

#[test]
fn send_calendar_event_result_and_participant_problem_builders() {
    let mut send_to = BTreeMap::new();
    send_to.insert(
        "imip".to_string(),
        "mailto:colleague@example.org".to_string(),
    );

    let event = CalendarEvent::simple("cal-snd-1", "Design Session", "2026-07-10T11:00:00", "PT1H");

    let send_event = SendCalendarEvent::new()
        .with_recipient("colleague@example.org")
        .with_send_to(send_to.clone())
        .with_calendar_event(event.clone())
        .with_include_old_properties(true);

    assert_eq!(
        send_event.recipient.as_deref(),
        Some("colleague@example.org")
    );
    assert_eq!(send_event.send_to, Some(send_to));
    assert_eq!(send_event.calendar_event, Some(event));
    assert_eq!(send_event.include_old_properties, Some(true));

    let problem = ParticipantProblem::new(participant_problem_kind::INVALID_EMAIL)
        .with_description("Mail server rejected recipient");

    assert_eq!(
        problem.kind.as_deref(),
        Some(participant_problem_kind::INVALID_EMAIL)
    );
    assert_eq!(
        problem.description.as_deref(),
        Some("Mail server rejected recipient")
    );

    let mut problems = BTreeMap::new();
    problems.insert("p-colleague-1".to_string(), problem.clone());

    let res = SendCalendarEventResult::new()
        .with_send_status("rejected")
        .with_participant_problems(problems.clone());

    assert_eq!(res.send_status.as_deref(), Some("rejected"));
    assert_eq!(res.participant_problems, Some(problems));

    let val = serde_json::to_value(&res).expect("serialize SendCalendarEventResult");
    let back: SendCalendarEventResult =
        serde_json::from_value(val).expect("deserialize SendCalendarEventResult");
    assert_eq!(back, res);
}

#[test]
fn participant_identity_builders_and_properties() {
    let mut send_to = BTreeMap::new();
    send_to.insert(
        "imip".to_string(),
        "mailto:lead@company.example".to_string(),
    );

    let mut extra = BTreeMap::new();
    extra.insert("customDirectoryId".to_string(), json!("dir-ldap-99"));

    let identity = ParticipantIdentity::new("Team Lead")
        .with_id("ident-lead-1")
        .with_schedule_id("sched-lead-1")
        .with_calendar_address("mailto:lead@company.example")
        .with_send_to(send_to.clone())
        .with_send_to_method("itip", "mailto:itip-lead@company.example")
        .is_default(true)
        .with_extra(extra);

    assert_eq!(identity.name, "Team Lead");
    assert_eq!(identity.id, Some(Id::from("ident-lead-1")));
    assert_eq!(identity.schedule_id.as_deref(), Some("sched-lead-1"));
    assert_eq!(
        identity.calendar_address.as_deref(),
        Some("mailto:lead@company.example")
    );
    let current_send_to = identity.send_to.as_ref().expect("send_to present");
    assert_eq!(
        current_send_to.get("imip").map(String::as_str),
        Some("mailto:lead@company.example")
    );
    assert_eq!(
        current_send_to.get("itip").map(String::as_str),
        Some("mailto:itip-lead@company.example")
    );
    assert_eq!(identity.is_default, Some(true));
    assert_eq!(
        identity.extra.get("customDirectoryId"),
        Some(&json!("dir-ldap-99"))
    );

    let val = serde_json::to_value(&identity).expect("serialize ParticipantIdentity");
    assert_eq!(val["name"], "Team Lead");
    assert_eq!(val["id"], "ident-lead-1");
    assert_eq!(val["isDefault"], true);
    assert_eq!(val["customDirectoryId"], "dir-ldap-99");

    let back: ParticipantIdentity =
        serde_json::from_value(val).expect("deserialize ParticipantIdentity");
    assert_eq!(back, identity);
}
