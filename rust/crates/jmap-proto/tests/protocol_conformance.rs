// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Protocol conformance and forward-compatibility tests across JMAP specifications.
//!
//! Covers:
//! - RFC 8620 (Core): Request/Response envelopes, Session capabilities & unknown URNs.
//! - RFC 8621 (Mail): Required vs optional fields, defaults, unknown mailbox roles ("snoozed" per draft-ietf-extra-email-snooze), snooze payloads on Email.
//! - RFC 9610 & RFC 9553 (Contacts / JSContact): Required vs optional fields, forward-compatibility with unknown kinds and components.
//! - draft-ietf-jmap-calendars-28 & RFC 8984 (Calendars / JSCalendar): Forward-compatibility with unknown event statuses, free/busy states, and alert actions.
//! - RFC 9670 (Principals): Forward-compatibility with unknown principal types and extension capability bags.

use jmap_proto::calendars::{
    Calendar, CalendarEvent, calendar_free_busy_status, event_status, free_busy_status,
};
use jmap_proto::contacts::{AddressBook, ContactCard, card_kind};
use jmap_proto::error::MethodError;
use jmap_proto::id::Id;
use jmap_proto::mail::{Email, EmailBodyPart, EmailBodyValue, Mailbox, role};
use jmap_proto::methods::{GetRequest, GetResponse};
use jmap_proto::principals::Principal;
use jmap_proto::request::Request;
use jmap_proto::response::Response;
use jmap_proto::session::{
    CAPABILITY_BLOB, CAPABILITY_CALENDARS, CAPABILITY_CONTACTS, CAPABILITY_CORE, CAPABILITY_MAIL,
    CAPABILITY_MAIL_SHARE, CAPABILITY_MDN, CAPABILITY_METADATA, CAPABILITY_PRINCIPALS,
    CAPABILITY_QUOTA, CAPABILITY_SIEVE, CAPABILITY_SUBMISSION, CAPABILITY_TASKS,
    CAPABILITY_VACATION_RESPONSE, CAPABILITY_WEBSOCKET, Session,
};
use jmap_proto::sieve::{
    SieveCapability, SieveScript, SieveScriptValidateResponse, sieve_set_error,
};
use jmap_proto::tasks::{
    Task, TaskList, TasksCapability, task_progress, task_set_error, task_status,
};
use serde_json::json;

// ===========================================================================
// RFC 8620 Core: Session, Envelopes & Forward Compatibility
// ===========================================================================

#[test]
fn session_forward_compatibility_with_unknown_capabilities_and_fields() {
    let raw_session = json!({
        "capabilities": {
            "urn:ietf:params:jmap:core": {
                "maxSizeUpload": 50000000,
                "maxConcurrentUpload": 4,
                "maxSizeRequest": 10000000,
                "maxConcurrentRequests": 8,
                "maxCallsInRequest": 16,
                "maxObjectsInGet": 500,
                "maxObjectsInSet": 500,
                "collationAlgorithms": ["i;ascii-casemap", "i;octet"]
            },
            "urn:ietf:params:jmap:mail": {
                "maxSizeAttachmentsPerEmail": 25000000,
                "maxSizeEmailInBytes": 50000000,
                "maxSizeBodyValueBytes": 2000000,
                "maxNumberOfAttachmentsPerEmail": 100,
                "maxNumberOfRecipientsPerEmail": 100,
                "mayCreateTopLevelMailbox": true
            },
            "urn:ietf:params:jmap:mail:snooze": {
                "maxSnoozeDuration": 2592000
            },
            "urn:ietf:params:jmap:principals:owner": {
                "customSetting": "allowed"
            },
            "https://custom-vendor.example.com/jmap/extension-2027": {
                "version": "1.4.0",
                "experimental": true
            }
        },
        "accounts": {
            "A_primary": {
                "name": "user@example.com",
                "isPersonal": true,
                "isReadOnly": false,
                "accountCapabilities": {
                    "urn:ietf:params:jmap:core": {},
                    "urn:ietf:params:jmap:mail": {},
                    "urn:ietf:params:jmap:mail:snooze": {
                        "supported": true
                    }
                },
                "vendorAccountMeta": {
                    "tier": "pro",
                    "storageQuotaBytes": 107374182400_u64
                }
            }
        },
        "primaryAccounts": {
            "urn:ietf:params:jmap:core": "A_primary",
            "urn:ietf:params:jmap:mail": "A_primary",
            "urn:ietf:params:jmap:mail:snooze": "A_primary"
        },
        "username": "user@example.com",
        "apiUrl": "https://api.example.com/jmap/",
        "downloadUrl": "https://api.example.com/download/{blobId}",
        "uploadUrl": "https://api.example.com/upload/",
        "eventSourceUrl": "https://api.example.com/events/",
        "state": "sess_state_v99",
        "serverMetadata": {
            "serverSoftware": "NextGenJmapServer/3.0",
            "clusterRegion": "us-east-1"
        }
    });

    let session: Session = serde_json::from_value(raw_session).expect("deserializes Session");

    // Standard core capability accessor works
    let core_cap = session.core_capability().expect("has core capability");
    assert_eq!(core_cap.max_size_upload, 50000000);
    assert_eq!(core_cap.max_concurrent_upload, 4);

    // Standard mail capability accessor works
    let mail_cap = session.mail_capability().expect("has mail capability");
    assert_eq!(mail_cap.max_size_email_in_bytes, 50000000);
    assert!(mail_cap.may_create_top_level_mailbox);

    // Unknown capability URNs survive intact in capabilities map
    assert!(
        session
            .capabilities
            .contains_key("urn:ietf:params:jmap:mail:snooze")
    );
    assert_eq!(
        session.capabilities["urn:ietf:params:jmap:mail:snooze"]["maxSnoozeDuration"],
        2592000
    );
    assert!(
        session
            .capabilities
            .contains_key("https://custom-vendor.example.com/jmap/extension-2027")
    );

    // Account unknown capabilities and extra fields survive
    let account = session
        .accounts
        .get(&Id::new("A_primary"))
        .expect("account A_primary");
    assert!(account.is_personal);
    assert!(
        account
            .account_capabilities
            .contains_key("urn:ietf:params:jmap:mail:snooze")
    );
    assert_eq!(account.extra["vendorAccountMeta"]["tier"], "pro");

    // Root extra fields survive in session.extra
    assert_eq!(
        session.extra["serverMetadata"]["serverSoftware"],
        "NextGenJmapServer/3.0"
    );

    // Primary account mapping for unknown capability works
    assert_eq!(
        session.primary_account("urn:ietf:params:jmap:mail:snooze"),
        Some(&Id::new("A_primary"))
    );
}

#[test]
fn session_quota_blob_tasks_and_sieve_capabilities_typed_accessors() {
    let raw = json!({
        "capabilities": {
            "urn:ietf:params:jmap:core": {},
            "urn:ietf:params:jmap:quota": {},
            "urn:ietf:params:jmap:blob": {
                "maxSizeSource": 50000000,
                "maxSizeTarget": 25000000
            },
            "urn:ietf:params:jmap:tasks": {
                "maxTasksPerGet": 1000,
                "maxTasksPerSet": 500,
                "maxTaskListsPerGet": 100
            },
            "urn:ietf:params:jmap:sieve": {
                "maxSizeScript": 65536,
                "maxNumberScripts": 20,
                "implementation": "ManageSieve 2.0",
                "sieveExtensions": ["fileinto", "reject"]
            }
        },
        "accounts": {},
        "primaryAccounts": {},
        "username": "user@example.com",
        "apiUrl": "https://api.example.com/jmap/",
        "downloadUrl": "https://api.example.com/download/{blobId}",
        "uploadUrl": "https://api.example.com/upload/",
        "state": "s1"
    });

    let session: Session = serde_json::from_value(raw).expect("deserializes Session");
    assert!(session.quota_capability().is_some());
    let blob_cap = session.blob_capability().expect("has blob capability");
    assert_eq!(blob_cap.max_size_source, Some(50000000));
    assert_eq!(blob_cap.max_size_target, Some(25000000));
    let tasks_cap: TasksCapability = session.tasks_capability().expect("has tasks capability");
    assert_eq!(tasks_cap.max_tasks_per_get, Some(1000));
    assert_eq!(tasks_cap.max_tasks_per_set, Some(500));
    assert_eq!(tasks_cap.max_task_lists_per_get, Some(100));
    let sieve_cap: SieveCapability = session.sieve_capability().expect("has sieve capability");
    assert_eq!(sieve_cap.max_size_script, 65536);
    assert_eq!(sieve_cap.max_number_scripts, Some(20));
    assert_eq!(sieve_cap.implementation.as_deref(), Some("ManageSieve 2.0"));
    assert_eq!(
        sieve_cap.sieve_extensions,
        vec!["fileinto".to_string(), "reject".to_string()]
    );
}

#[test]
fn response_and_invocation_forward_compatibility_with_unknown_methods() {
    let raw_response = json!({
        "methodResponses": [
            [
                "Mailbox/get",
                {
                    "accountId": "acc_1",
                    "state": "mbx_st_1",
                    "list": [
                        { "id": "m1", "name": "Inbox", "role": "inbox" }
                    ]
                },
                "c0"
            ],
            [
                "CustomVendorService/analyze",
                {
                    "status": "completed",
                    "confidenceScore": 0.98,
                    "tags": ["urgent", "actionable"]
                },
                "c1"
            ],
            [
                "error",
                {
                    "type": "urn:ietf:params:jmap:error:unknownFutureError",
                    "description": "A future error extension was encountered"
                },
                "c2"
            ]
        ],
        "sessionState": "sess_st_88",
        "createdIds": {
            "cid_1": "real_id_1"
        },
        "unknownServerHeader": "tracing-12345"
    });

    let response: Response = serde_json::from_value(raw_response).expect("deserializes Response");
    assert_eq!(response.session_state.as_str(), "sess_st_88");
    assert_eq!(response.method_responses.len(), 3);

    // Method 0: standard Mailbox/get
    let inv0 = &response.method_responses[0];
    assert_eq!(inv0.name, "Mailbox/get");
    assert!(!inv0.is_error());
    let get_resp: GetResponse<Mailbox> = inv0.parse().expect("parse GetResponse<Mailbox>");
    assert_eq!(get_resp.list.len(), 1);

    // Method 1: unknown method name parses safely into Invocation
    let inv1 = &response.method_responses[1];
    assert_eq!(inv1.name, "CustomVendorService/analyze");
    assert_eq!(inv1.call_id, "c1");
    assert!(!inv1.is_error());
    assert_eq!(inv1.arguments["status"], "completed");
    assert_eq!(inv1.arguments["confidenceScore"], 0.98);

    // Method 2: method error
    let inv2 = &response.method_responses[2];
    assert!(inv2.is_error());
    let err: MethodError = inv2.parse().expect("parse MethodError");
    assert_eq!(
        err.error_type,
        "urn:ietf:params:jmap:error:unknownFutureError"
    );
}

#[test]
fn minimal_request_and_response_payloads_per_rfc8620() {
    // Minimal Request: using and methodCalls
    let min_req_json = json!({
        "using": ["urn:ietf:params:jmap:core"],
        "methodCalls": []
    });
    let req: Request = serde_json::from_value(min_req_json).expect("minimal Request");
    assert_eq!(req.using.len(), 1);
    assert!(req.method_calls.is_empty());
    assert!(req.created_ids.is_none());

    // Minimal Response: methodResponses only (sessionState defaults)
    let min_resp_json = json!({
        "methodResponses": []
    });
    let resp: Response = serde_json::from_value(min_resp_json).expect("minimal Response");
    assert!(resp.method_responses.is_empty());
    assert!(resp.session_state.as_str().is_empty());
    assert!(resp.created_ids.is_none());

    // Minimal GetRequest: accountId only
    let min_get_req_json = json!({
        "accountId": "a1"
    });
    let get_req: GetRequest = serde_json::from_value(min_get_req_json).expect("minimal GetRequest");
    assert_eq!(get_req.account_id.as_str(), "a1");
    assert!(get_req.ids.is_none());
    assert!(get_req.properties.is_none());

    // Minimal GetResponse: accountId, state, list
    let min_get_resp_json = json!({
        "accountId": "a1",
        "state": "s1",
        "list": []
    });
    let get_resp: GetResponse<Mailbox> =
        serde_json::from_value(min_get_resp_json).expect("minimal GetResponse");
    assert_eq!(get_resp.account_id.as_str(), "a1");
    assert_eq!(get_resp.state.as_str(), "s1");
    assert!(get_resp.list.is_empty());
    assert!(get_resp.not_found.is_empty());
}

// ===========================================================================
// RFC 8621 Mail: Mailbox, Email, Roles & Snooze Draft Conformance
// ===========================================================================

#[test]
fn mailbox_conformance_required_vs_optional_fields_and_defaults() {
    // Minimal Mailbox (RFC 8621 §2): only id and name
    let minimal_json = json!({
        "id": "mbx_min",
        "name": "Archive 2026"
    });
    let mbx: Mailbox = serde_json::from_value(minimal_json).expect("minimal Mailbox");
    assert_eq!(mbx.id.as_ref().unwrap().as_str(), "mbx_min");
    assert_eq!(mbx.name, "Archive 2026");
    assert!(mbx.parent_id.is_none());
    assert!(mbx.role.is_none());
    assert!(mbx.sort_order.is_none());
    assert!(mbx.total_emails.is_none());
    assert!(mbx.unread_emails.is_none());
    assert!(mbx.total_threads.is_none());
    assert!(mbx.unread_threads.is_none());
    assert!(mbx.is_subscribed.is_none());
    assert!(mbx.my_rights.is_none());
    assert!(mbx.extra.is_empty());
}

#[test]
fn mailbox_conformance_snoozed_role_and_unknown_roles_and_extensions() {
    // draft-ietf-extra-email-snooze specifies role "snoozed"
    let payload = json!({
        "id": "mbx_snoozed_1",
        "name": "Snoozed Messages",
        "parentId": "mbx_root",
        "role": "snoozed",
        "sortOrder": 15,
        "totalEmails": 7,
        "unreadEmails": 0,
        "isSubscribed": true,
        "snoozeCapacity": 500,
        "wakeUpPolicy": "automatic"
    });

    let mbx: Mailbox = serde_json::from_value(payload).expect("Mailbox with snoozed role");
    assert_eq!(mbx.id.as_ref().unwrap().as_str(), "mbx_snoozed_1");
    assert_eq!(mbx.role.as_deref(), Some("snoozed"));
    assert_eq!(mbx.sort_order, Some(15));
    assert_eq!(mbx.total_emails, Some(7));
    assert_eq!(mbx.unread_emails, Some(0));
    assert_eq!(mbx.is_subscribed, Some(true));

    // Custom extension fields are safely retained in extra
    assert_eq!(mbx.extra["snoozeCapacity"], 500);
    assert_eq!(mbx.extra["wakeUpPolicy"], "automatic");
}

#[test]
fn email_conformance_required_vs_optional_fields_and_snooze_payload() {
    // Minimal Email (RFC 8621 §4)
    let min_email_json = json!({
        "id": "e_min_1",
        "blobId": "b_min_1",
        "threadId": "th_min_1",
        "mailboxIds": {
            "mbx_inbox": true
        },
        "size": 1024
    });

    let email: Email = serde_json::from_value(min_email_json).expect("minimal Email");
    assert_eq!(email.id.as_ref().unwrap().as_str(), "e_min_1");
    assert_eq!(email.blob_id.as_ref().unwrap().as_str(), "b_min_1");
    assert_eq!(email.thread_id.as_ref().unwrap().as_str(), "th_min_1");
    assert_eq!(email.size, Some(1024));
    assert!(email.keywords.is_none());
    assert!(email.received_at.is_none());
    assert!(email.subject.is_none());
    assert!(email.from.is_none());
    assert!(email.to.is_none());
    assert!(email.body_structure.is_none());
    assert!(email.extra.is_empty());

    // Snoozed Email per draft-ietf-extra-email-snooze:
    // Email carries an immutable `snoozed` property with SnoozeDetails
    let snoozed_email_json = json!({
        "id": "e_snoozed_1",
        "blobId": "b_snoozed_1",
        "threadId": "th_snoozed_1",
        "mailboxIds": {
            "mbx_snoozed": true
        },
        "keywords": {
            "$seen": true,
            "$snoozed": true
        },
        "size": 4096,
        "subject": "Follow up with client tomorrow",
        "snoozed": {
            "until": "2026-09-01T09:00:00Z",
            "moveToMailboxId": "mbx_inbox",
            "setKeywords": {
                "$seen": false,
                "$flagged": true
            }
        },
        "aiPriorityScore": 8.5
    });

    let snoozed_email: Email =
        serde_json::from_value(snoozed_email_json).expect("Email with snooze details");
    assert_eq!(snoozed_email.id.as_ref().unwrap().as_str(), "e_snoozed_1");
    assert_eq!(
        snoozed_email.subject.as_deref(),
        Some("Follow up with client tomorrow")
    );

    // SnoozeDetails object rides cleanly in extra["snoozed"]
    let snooze_details = &snoozed_email.extra["snoozed"];
    assert_eq!(snooze_details["until"], "2026-09-01T09:00:00Z");
    assert_eq!(snooze_details["moveToMailboxId"], "mbx_inbox");
    assert_eq!(snooze_details["setKeywords"]["$flagged"], true);
    assert_eq!(snoozed_email.extra["aiPriorityScore"], 8.5);
}

#[test]
fn email_body_part_and_value_conformance_and_forward_compatibility() {
    let minimal_part = json!({
        "partId": "part_1",
        "blobId": "blob_1",
        "size": 512,
        "type": "text/plain",
        "disposition": "inline",
        "cid": "cid_sample"
    });

    let part: EmailBodyPart = serde_json::from_value(minimal_part).expect("minimal EmailBodyPart");
    assert_eq!(part.part_id.as_deref(), Some("part_1"));
    assert_eq!(part.blob_id.as_ref().unwrap().as_str(), "blob_1");
    assert_eq!(part.size, Some(512));
    assert_eq!(part.content_type.as_deref(), Some("text/plain"));
    assert_eq!(part.disposition.as_deref(), Some("inline"));
    assert_eq!(part.cid.as_deref(), Some("cid_sample"));
    assert!(part.sub_parts.is_none());

    let minimal_val = json!({
        "value": "Hello world"
    });
    let val: EmailBodyValue = serde_json::from_value(minimal_val).expect("minimal EmailBodyValue");
    assert_eq!(val.value, "Hello world");
    assert!(!val.is_encoding_problem, "RFC 8621 default is false");
    assert!(!val.is_truncated, "RFC 8621 default is false");
}

// ===========================================================================
// RFC 9610 & RFC 9553: Contacts & JSContact Forward Conformance
// ===========================================================================

#[test]
fn contact_card_forward_compatibility_with_unknown_kinds_and_components() {
    let payload = json!({
        "@type": "Card",
        "id": "c_ai_bot_1",
        "kind": "application",
        "version": "1.0",
        "name": {
            "full": "Antigravity Assistant",
            "components": [
                { "kind": "title", "value": "AI" },
                { "kind": "given", "value": "Antigravity" },
                { "kind": "customBadge", "value": "Pro" }
            ]
        },
        "emails": {
            "e1": {
                "address": "bot@example.com",
                "contexts": {
                    "metaverse": true
                }
            }
        },
        "relatedTo": {
            "rel_dev_1": {
                "relation": {
                    "developer": true,
                    "operator": true
                }
            }
        },
        "aiModelVersion": "gemini-3.7-flash",
        "quantumSecureKey": "QSK-998877"
    });

    let card: ContactCard = serde_json::from_value(payload).expect("ContactCard deserializes");
    assert_eq!(card.id.as_ref().unwrap().as_str(), "c_ai_bot_1");
    assert_eq!(card.kind.as_deref(), Some("application"));
    assert_eq!(card.card_type.as_deref(), Some("Card"));

    let name = card.name.as_ref().unwrap();
    assert_eq!(name.full.as_deref(), Some("Antigravity Assistant"));
    assert_eq!(name.components.as_ref().unwrap().len(), 3);

    // Extra card-level fields survive in extra
    assert_eq!(card.extra["aiModelVersion"], "gemini-3.7-flash");
    assert_eq!(card.extra["quantumSecureKey"], "QSK-998877");
}

#[test]
fn address_book_conformance_required_vs_optional_fields() {
    let minimal_book = json!({
        "id": "ab_personal",
        "name": "Personal Contacts"
    });

    let book: AddressBook = serde_json::from_value(minimal_book).expect("minimal AddressBook");
    assert_eq!(book.id.as_ref().unwrap().as_str(), "ab_personal");
    assert_eq!(book.name, "Personal Contacts");
    assert!(book.description.is_none());
    assert!(book.sort_order.is_none());
    assert!(book.is_default.is_none());
    assert!(book.is_subscribed.is_none());
    assert!(book.share_with.is_none());
    assert!(book.my_rights.is_none());
    assert!(book.may_delete.is_none());
    assert!(book.extra.is_empty());
}

// ===========================================================================
// draft-ietf-jmap-calendars-28 & RFC 8984: JSCalendar Conformance
// ===========================================================================

#[test]
fn calendar_event_forward_compatibility_with_unknown_statuses_and_properties() {
    let payload = json!({
        "@type": "Event",
        "id": "ev_conference_1",
        "start": "2026-09-15T09:00:00",
        "timeZone": "Europe/Zurich",
        "duration": "PT3H",
        "title": "Quantum Computing Summit",
        "status": "tentative-rescheduled",
        "freeBusyStatus": "working-remotely",
        "privacy": "secret",
        "priority": 2,
        "alerts": {
            "a1": {
                "@type": "Alert",
                "trigger": {
                    "@type": "OffsetTrigger",
                    "offset": "-PT30M",
                    "relativeTo": "start"
                },
                "action": "webhook-notify",
                "webhookEndpoint": "https://notify.example.com/alerts"
            }
        },
        "virtualLocations": {
            "v1": {
                "@type": "VirtualLocation",
                "uri": "https://hologram.example.com/room/42",
                "features": ["3d-video", "spatial-audio", "live-transcript"]
            }
        },
        "streamingFeedUrl": "rtmp://live.example.com/summit"
    });

    let event: CalendarEvent = serde_json::from_value(payload).expect("CalendarEvent deserializes");
    assert_eq!(event.id.as_ref().unwrap().as_str(), "ev_conference_1");
    assert_eq!(event.start.as_deref(), Some("2026-09-15T09:00:00"));
    assert_eq!(event.status.as_deref(), Some("tentative-rescheduled"));
    assert_eq!(event.free_busy_status.as_deref(), Some("working-remotely"));
    assert_eq!(event.priority, Some(2));
    assert_eq!(event.privacy.as_deref(), Some("secret"));

    // Alerts and virtual locations survive as Values
    assert!(event.alerts.as_ref().unwrap().contains_key("a1"));
    assert!(event.virtual_locations.as_ref().unwrap().contains_key("v1"));

    // Unknown top-level properties ride safely in extra
    assert_eq!(
        event.extra["streamingFeedUrl"],
        "rtmp://live.example.com/summit"
    );
}

#[test]
fn calendar_conformance_required_vs_optional_fields() {
    let minimal_calendar = json!({
        "id": "cal_work",
        "name": "Work Calendar"
    });

    let cal: Calendar = serde_json::from_value(minimal_calendar).expect("minimal Calendar");
    assert_eq!(cal.id.as_ref().unwrap().as_str(), "cal_work");
    assert_eq!(cal.name, "Work Calendar");
    assert!(cal.description.is_none());
    assert!(cal.color.is_none());
    assert!(cal.sort_order.is_none());
    assert!(cal.is_default.is_none());
    assert!(cal.is_subscribed.is_none());
    assert!(cal.is_visible.is_none());
    assert!(cal.time_zone.is_none());
    assert!(cal.share_with.is_none());
    assert!(cal.my_rights.is_none());
    assert!(cal.may_delete.is_none());
    assert!(cal.extra.is_empty());
}

// ===========================================================================
// draft-ietf-jmap-tasks & RFC 8984: Task & TaskList Conformance
// ===========================================================================

#[test]
fn task_forward_compatibility_with_unknown_statuses_and_properties() {
    let payload = json!({
        "@type": "Task",
        "id": "task_future_1",
        "taskListId": "tl_projects",
        "uid": "uid-task-future-99",
        "title": "Deploy neural routing engine",
        "status": "in-review",
        "progress": "blocked-on-dependency",
        "percentComplete": 75,
        "due": "2026-10-01T12:00:00Z",
        "priority": 3,
        "customAutomatedAgent": "Gemini-Agent-42",
        "telemetryKey": "TM-889900"
    });

    let task: Task = serde_json::from_value(payload).expect("Task deserializes");
    assert_eq!(task.id.as_ref().unwrap().as_str(), "task_future_1");
    assert_eq!(task.task_list_id.as_ref().unwrap().as_str(), "tl_projects");
    assert_eq!(task.uid, "uid-task-future-99");
    assert_eq!(task.title.as_deref(), Some("Deploy neural routing engine"));
    assert_eq!(task.status.as_deref(), Some("in-review"));
    assert_eq!(task.progress.as_deref(), Some("blocked-on-dependency"));
    assert_eq!(task.percent_complete, Some(75));

    // Custom extension properties ride in extra
    assert_eq!(task.extra["customAutomatedAgent"], "Gemini-Agent-42");
    assert_eq!(task.extra["telemetryKey"], "TM-889900");
}

#[test]
fn task_list_conformance_required_vs_optional_fields() {
    let minimal_list = json!({
        "id": "tl_work",
        "name": "Work Tasks"
    });

    let list: TaskList = serde_json::from_value(minimal_list).expect("minimal TaskList");
    assert_eq!(list.id.as_ref().unwrap().as_str(), "tl_work");
    assert_eq!(list.name, "Work Tasks");
    assert!(list.color.is_none());
    assert!(list.sort_order.is_none());
    assert!(list.is_default.is_none());
    assert!(list.is_subscribed.is_none());
    assert!(list.share_with.is_none());
    assert!(list.my_rights.is_none());
    assert!(list.may_delete.is_none());
    assert!(list.extra.is_empty());
}

// ===========================================================================
// RFC 9670: Principals & Sharing Conformance
// ===========================================================================

#[test]
fn principal_forward_compatibility_with_unknown_types_and_capabilities() {
    let payload = json!({
        "id": "p_room_olympus",
        "type": "location-telepresence-room",
        "name": "Olympus Conference Center",
        "description": "Floor 4, Capacity 50, VR Enabled",
        "email": "olympus-room@example.com",
        "timeZone": "Europe/Berlin",
        "isPersonal": false,
        "capabilities": {
            "urn:ietf:params:jmap:calendars": {
                "mayGetAvailability": true,
                "maxConcurrentBookings": 1
            },
            "urn:custom:vendor:facilities": {
                "projector4k": true,
                "whiteboardRobot": true
            }
        },
        "facilitiesManagementId": "FM-4002"
    });

    let principal: Principal = serde_json::from_value(payload).expect("Principal deserializes");
    assert_eq!(principal.id.as_ref().unwrap().as_str(), "p_room_olympus");
    assert_eq!(
        principal.principal_type.as_deref(),
        Some("location-telepresence-room")
    );
    assert_eq!(principal.name, "Olympus Conference Center");
    assert_eq!(principal.is_personal, Some(false));

    // Capabilities bag retains standard and unknown capability bags
    assert!(
        principal
            .capabilities
            .contains_key("urn:ietf:params:jmap:calendars")
    );
    assert!(
        principal
            .capabilities
            .contains_key("urn:custom:vendor:facilities")
    );
    assert_eq!(
        principal.capabilities["urn:custom:vendor:facilities"]["whiteboardRobot"],
        true
    );

    // Extra fields survive
    assert_eq!(principal.extra["facilitiesManagementId"], "FM-4002");
}

// ===========================================================================
// Standard RFC Constants Coverage & Verification
// ===========================================================================

#[test]
fn standard_rfc_capability_and_role_constants_exact_values() {
    // Core & Capabilities
    assert_eq!(CAPABILITY_CORE, "urn:ietf:params:jmap:core");
    assert_eq!(CAPABILITY_MAIL, "urn:ietf:params:jmap:mail");
    assert_eq!(CAPABILITY_SUBMISSION, "urn:ietf:params:jmap:submission");
    assert_eq!(
        CAPABILITY_VACATION_RESPONSE,
        "urn:ietf:params:jmap:vacationresponse"
    );
    assert_eq!(CAPABILITY_MDN, "urn:ietf:params:jmap:mdn");
    assert_eq!(CAPABILITY_CONTACTS, "urn:ietf:params:jmap:contacts");
    assert_eq!(CAPABILITY_CALENDARS, "urn:ietf:params:jmap:calendars");
    assert_eq!(CAPABILITY_PRINCIPALS, "urn:ietf:params:jmap:principals");
    assert_eq!(CAPABILITY_WEBSOCKET, "urn:ietf:params:jmap:websocket");
    assert_eq!(CAPABILITY_QUOTA, "urn:ietf:params:jmap:quota");
    assert_eq!(CAPABILITY_BLOB, "urn:ietf:params:jmap:blob");
    assert_eq!(CAPABILITY_TASKS, "urn:ietf:params:jmap:tasks");
    assert_eq!(CAPABILITY_SIEVE, "urn:ietf:params:jmap:sieve");

    // Mailbox roles (RFC 8621 §2 / RFC 8457)
    assert_eq!(role::INBOX, "inbox");
    assert_eq!(role::DRAFTS, "drafts");
    assert_eq!(role::SENT, "sent");
    assert_eq!(role::TRASH, "trash");
    assert_eq!(role::JUNK, "junk");
    assert_eq!(role::ARCHIVE, "archive");
    assert_eq!(role::ALL, "all");
    assert_eq!(role::FLAGGED, "flagged");
    assert_eq!(role::IMPORTANT, "important");

    // Card kinds (RFC 9553 §2.1.1)
    assert_eq!(card_kind::INDIVIDUAL, "individual");
    assert_eq!(card_kind::GROUP, "group");
    assert_eq!(card_kind::ORG, "org");
    assert_eq!(card_kind::LOCATION, "location");
    assert_eq!(card_kind::DEVICE, "device");
    assert_eq!(card_kind::APPLICATION, "application");

    // Calendar statuses (RFC 8984 §4.1.1)
    assert_eq!(event_status::CONFIRMED, "confirmed");
    assert_eq!(event_status::TENTATIVE, "tentative");
    assert_eq!(event_status::CANCELLED, "cancelled");

    // Task statuses (RFC 8984 §5.1, draft-ietf-jmap-tasks §4.1)
    assert_eq!(task_status::NEEDS_ACTION, "needs-action");
    assert_eq!(task_status::COMPLETED, "completed");
    assert_eq!(task_status::IN_PROCESS, "in-process");
    assert_eq!(task_status::CANCELLED, "cancelled");
    assert_eq!(task_status::FAILED, "failed");

    // Task progress (RFC 8984 §5.1)
    assert_eq!(task_progress::NEEDS_ACTION, "needs-action");
    assert_eq!(task_progress::COMPLETED, "completed");
    assert_eq!(task_progress::IN_PROCESS, "in-process");
    assert_eq!(task_progress::FAILED, "failed");
    assert_eq!(task_progress::CANCELLED, "cancelled");

    // Task set errors (draft-ietf-jmap-tasks §4.3)
    assert_eq!(task_set_error::TOO_MANY_RECURRENCES, "tooManyRecurrences");
    assert_eq!(task_set_error::TASK_LIST_NOT_FOUND, "taskListNotFound");

    // Sieve set errors (RFC 9265 §2.3.2)
    assert_eq!(
        sieve_set_error::CANNOT_DELETE_ACTIVE_SCRIPT,
        "cannotDeleteActiveScript"
    );
    assert_eq!(
        sieve_set_error::DUPLICATE_SCRIPT_NAME,
        "duplicateScriptName"
    );
    assert_eq!(sieve_set_error::INVALID_SIEVE, "invalidSieve");
    assert_eq!(
        sieve_set_error::MAX_NUMBER_SCRIPTS_EXCEEDED,
        "maxNumberScriptsExceeded"
    );
    assert_eq!(
        sieve_set_error::MAX_SIZE_SCRIPT_EXCEEDED,
        "maxSizeScriptExceeded"
    );
    assert_eq!(
        sieve_set_error::MULTIPLE_ACTIVE_SCRIPTS,
        "multipleActiveScripts"
    );

    // Calendar free/busy (RFC 8984 §4.1.2)
    assert_eq!(free_busy_status::FREE, "free");
    assert_eq!(free_busy_status::BUSY, "busy");

    // CalendarEvent/getFreeBusy free/busy statuses (draft-ietf-jmap-calendars-28 §5.4)
    assert_eq!(calendar_free_busy_status::FREE, "free");
    assert_eq!(calendar_free_busy_status::BUSY, "busy");
    assert_eq!(calendar_free_busy_status::BUSY_TENTATIVE, "busy-tentative");
    assert_eq!(
        calendar_free_busy_status::BUSY_UNAVAILABLE,
        "busy-unavailable"
    );
}

// ===========================================================================
// RFC 9265 Sieve: Forward Compatibility & Conformance
// ===========================================================================

#[test]
fn rfc9265_sieve_script_and_validation_forward_compatibility() {
    let script_payload = json!({
        "id": "sieve_1",
        "name": "Main Rule",
        "blobId": "b_sieve_100",
        "isActive": true,
        "futureExtensionProperty": "flag-tag",
        "vendorAudit": {
            "lastChecked": "2026-08-30T12:00:00Z",
            "passedValidation": true
        }
    });

    let script: SieveScript = serde_json::from_value(script_payload)
        .expect("deserialize SieveScript with unknown fields");
    assert_eq!(script.id.as_ref().unwrap().as_str(), "sieve_1");
    assert_eq!(script.name, "Main Rule");
    assert_eq!(script.blob_id.as_str(), "b_sieve_100");
    assert!(script.is_active);
    assert_eq!(script.extra["futureExtensionProperty"], "flag-tag");
    assert_eq!(script.extra["vendorAudit"]["passedValidation"], true);

    let validate_resp_payload = json!({
        "accountId": "acc_sieve",
        "isValid": false,
        "error": {
            "description": "Invalid test command",
            "lineNumber": 15,
            "columnNumber": 3,
            "action": "custom_filter",
            "extraDiagnosticCode": "E_SYNTAX_402"
        },
        "serverWarnings": ["Script exceeds recommended execution step limit"]
    });

    let resp: SieveScriptValidateResponse = serde_json::from_value(validate_resp_payload)
        .expect("deserialize SieveScriptValidateResponse with diagnostics");
    assert_eq!(resp.account_id.as_str(), "acc_sieve");
    assert!(!resp.is_valid);
    let err = resp.error.expect("validation error present");
    assert_eq!(err.description.as_deref(), Some("Invalid test command"));
    assert_eq!(err.line_number, Some(15));
    assert_eq!(err.column_number, Some(3));
    assert_eq!(err.action.as_deref(), Some("custom_filter"));
    assert_eq!(err.extra["extraDiagnosticCode"], "E_SYNTAX_402");
    assert_eq!(
        resp.extra["serverWarnings"][0],
        "Script exceeds recommended execution step limit"
    );
}

// ===========================================================================
// RFC 8984 JSCalendar Group & Capability Conformance
// ===========================================================================

#[test]
fn rfc8984_calendar_group_and_capabilities_forward_compatibility() {
    use jmap_proto::calendars::{CalendarGroup, CalendarPreferencesCapability};
    use jmap_proto::mail::MDNCapability;

    let group_payload = json!({
        "@type": "Group",
        "id": "grp_future",
        "uid": "urn:uuid:group-9999",
        "title": "Release Team Milestones",
        "description": "Sprint milestones and tasks",
        "timeZone": "Europe/Paris",
        "updated": "2026-08-30T10:00:00Z",
        "entries": {
            "evt_release_candidate": {
                "@type": "Event",
                "title": "Release Candidate Freeze",
                "start": "2026-09-15T00:00:00"
            }
        },
        "keywords": {
            "release": true
        },
        "categories": {
            "engineering": true
        },
        "color": "#117733",
        "source": "https://calendar.example.com/groups/release",
        "links": {
            "tracker": {
                "href": "https://bugzilla.example.com/milestones"
            }
        },
        "futureGroupExtension": {
            "hierarchyLevel": 2,
            "readOnlyEntries": true
        }
    });

    let group: CalendarGroup = serde_json::from_value(group_payload)
        .expect("deserialize CalendarGroup with unknown fields");
    assert_eq!(group.id.as_ref().unwrap().as_str(), "grp_future");
    assert_eq!(group.group_type.as_deref(), Some("Group"));
    assert_eq!(group.uid.as_deref(), Some("urn:uuid:group-9999"));
    assert_eq!(group.title.as_deref(), Some("Release Team Milestones"));
    assert_eq!(
        group.description.as_deref(),
        Some("Sprint milestones and tasks")
    );
    assert_eq!(group.time_zone.as_deref(), Some("Europe/Paris"));
    assert_eq!(
        group.updated.as_ref().unwrap().as_str(),
        "2026-08-30T10:00:00Z"
    );
    assert_eq!(group.color.as_deref(), Some("#117733"));
    assert_eq!(
        group.source.as_deref(),
        Some("https://calendar.example.com/groups/release")
    );
    assert_eq!(group.keywords.as_ref().unwrap()["release"], true);
    assert!(group.categories.as_ref().unwrap()["engineering"]);
    assert_eq!(group.extra["futureGroupExtension"]["hierarchyLevel"], 2);
    assert_eq!(group.extra["futureGroupExtension"]["readOnlyEntries"], true);

    let pref_cap_payload = json!({
        "maxTimeZoneFavorites": 10,
        "vendorDefaultLocale": "en-US"
    });
    let pref_cap: CalendarPreferencesCapability = serde_json::from_value(pref_cap_payload)
        .expect("deserialize CalendarPreferencesCapability with extensions");
    assert_eq!(pref_cap.extra["maxTimeZoneFavorites"], 10);
    assert_eq!(pref_cap.extra["vendorDefaultLocale"], "en-US");

    let mdn_cap_payload = json!({
        "maxMdnsPerRequest": 50,
        "signingMethodsSupported": ["pgp", "smime"]
    });
    let mdn_cap: MDNCapability =
        serde_json::from_value(mdn_cap_payload).expect("deserialize MDNCapability with extensions");
    assert_eq!(mdn_cap.extra["maxMdnsPerRequest"], 50);
    assert_eq!(mdn_cap.extra["signingMethodsSupported"][0], "pgp");
}

// ===========================================================================
// draft-ietf-jmap-calendars-28 §7, §8, §10: Scheduling, Notifications & Replies
// ===========================================================================

#[test]
fn calendar_scheduling_and_notifications_forward_compatibility() {
    use jmap_proto::calendars::{
        CalendarEventNotification, CalendarEventSendRequest, CalendarEventSendResponse,
        ParticipantReply, calendar_event_notification_type, calendar_send_error,
        participant_participation_status, participant_problem_kind,
    };
    use jmap_proto::principals::ShareNotificationQueryFilter;

    // Constants check
    assert_eq!(calendar_event_notification_type::CREATED, "created");
    assert_eq!(calendar_event_notification_type::UPDATED, "updated");
    assert_eq!(calendar_event_notification_type::DESTROYED, "destroyed");
    assert_eq!(calendar_event_notification_type::REPLY, "reply");

    assert_eq!(
        participant_problem_kind::CANNOT_SEND_TO_SELF,
        "cannotSendToSelf"
    );
    assert_eq!(
        participant_problem_kind::CALENDAR_NOT_FOUND,
        "calendarNotFound"
    );
    assert_eq!(
        participant_problem_kind::PARTICIPANT_NOT_FOUND,
        "participantNotFound"
    );
    assert_eq!(participant_problem_kind::INVALID_EMAIL, "invalidEmail");
    assert_eq!(
        participant_problem_kind::CANNOT_SEND_TO_RESOURCE,
        "cannotSendToResource"
    );
    assert_eq!(participant_problem_kind::NOT_AUTHORIZED, "notAuthorized");

    assert_eq!(calendar_send_error::FORBIDDEN_FROM, "forbiddenFrom");
    assert_eq!(
        calendar_send_error::PARTICIPANT_NOT_FOUND,
        "participantNotFound"
    );
    assert_eq!(
        calendar_send_error::INVALID_PARTICIPANTS,
        "invalidParticipants"
    );
    assert_eq!(
        calendar_send_error::CANNOT_SEND_FOR_CALENDAR,
        "cannotSendForCalendar"
    );
    assert_eq!(calendar_send_error::EVENT_NOT_FOUND, "eventNotFound");

    assert_eq!(
        participant_participation_status::NEEDS_ACTION,
        "needs-action"
    );
    assert_eq!(participant_participation_status::ACCEPTED, "accepted");
    assert_eq!(participant_participation_status::DECLINED, "declined");
    assert_eq!(participant_participation_status::TENTATIVE, "tentative");
    assert_eq!(participant_participation_status::DELEGATED, "delegated");

    // 1. CalendarEventNotification with unknown fields and unknown type variant
    let notif_payload = json!({
        "@type": "CalendarEventNotification",
        "id": "notif_fut_1",
        "created": "2026-09-01T12:00:00Z",
        "type": "delegated-proxy-update",
        "eventId": "evt_999",
        "recurrenceId": "2026-09-01T14:00:00",
        "comment": "Delegated to assistant",
        "changedBy": {
            "name": "VP Alice",
            "email": "alice@example.com"
        },
        "event": {
            "title": "Quarterly Planning"
        },
        "customUrgencyLevel": "high",
        "auditLogSequence": 1042
    });

    let notif: CalendarEventNotification = serde_json::from_value(notif_payload)
        .expect("CalendarEventNotification deserializes with extensions");
    assert_eq!(notif.id.as_ref().unwrap().as_str(), "notif_fut_1");
    assert_eq!(notif.kind, "CalendarEventNotification");
    assert_eq!(notif.created.as_str(), "2026-09-01T12:00:00Z");
    assert_eq!(
        notif.notification_type.as_deref(),
        Some("delegated-proxy-update")
    );
    assert_eq!(notif.event_id.as_str(), "evt_999");
    assert_eq!(notif.recurrence_id.as_deref(), Some("2026-09-01T14:00:00"));
    assert_eq!(notif.comment.as_deref(), Some("Delegated to assistant"));
    assert_eq!(notif.changed_by.as_ref().unwrap().name, "VP Alice");
    assert_eq!(
        notif.event.as_ref().unwrap().title.as_deref(),
        Some("Quarterly Planning")
    );
    assert_eq!(notif.extra["customUrgencyLevel"], "high");
    assert_eq!(notif.extra["auditLogSequence"], 1042);

    // 2. CalendarEventSendRequest & Response with unknown fields
    let send_req_payload = json!({
        "accountId": "acc_main",
        "identityId": "ident_primary",
        "send": {
            "k1": {
                "recipient": "mailto:colleague@example.com",
                "calendarEvent": {
                    "title": "Strategy Session"
                },
                "includeOldProperties": false,
                "transportRouting": "direct-smtp"
            }
        },
        "onSuccessUpdateCalendarEvent": {
            "evt_999": {
                "status": "confirmed"
            }
        },
        "onSuccessDestroyCalendarEventIds": ["evt_stale"],
        "futureSendFlags": {
            "retryPolicy": "exponential-backoff"
        }
    });

    let send_req: CalendarEventSendRequest = serde_json::from_value(send_req_payload)
        .expect("CalendarEventSendRequest deserializes with extensions");
    assert_eq!(send_req.account_id.as_str(), "acc_main");
    assert_eq!(
        send_req.identity_id.as_ref().unwrap().as_str(),
        "ident_primary"
    );
    assert_eq!(
        send_req.send.as_ref().unwrap()[&Id::new("k1")]
            .recipient
            .as_deref(),
        Some("mailto:colleague@example.com")
    );
    assert_eq!(
        send_req.send.as_ref().unwrap()[&Id::new("k1")].extra["transportRouting"],
        "direct-smtp"
    );
    assert_eq!(
        send_req.extra["futureSendFlags"]["retryPolicy"],
        "exponential-backoff"
    );

    let send_resp_payload = json!({
        "accountId": "acc_main",
        "sent": {
            "k1": {
                "sendStatus": "delivered",
                "participantProblems": {
                    "mailto:external@example.org": {
                        "type": "customSpamBlockFilter",
                        "description": "External server rejected attachment",
                        "rejectCode": 554
                    }
                },
                "deliveryTimestamp": "2026-09-01T12:00:05Z"
            }
        },
        "notSent": {
            "k2": {
                "type": "cannotSendForCalendar",
                "description": "Calendar is read-only"
            }
        }
    });

    let send_resp: CalendarEventSendResponse = serde_json::from_value(send_resp_payload)
        .expect("CalendarEventSendResponse deserializes with extensions");
    assert_eq!(send_resp.account_id.as_str(), "acc_main");
    let sent_k1 = &send_resp.sent.as_ref().unwrap()[&Id::new("k1")];
    assert_eq!(sent_k1.send_status.as_deref(), Some("delivered"));
    assert_eq!(
        sent_k1.participant_problems.as_ref().unwrap()["mailto:external@example.org"]
            .kind
            .as_deref(),
        Some("customSpamBlockFilter")
    );
    assert_eq!(
        sent_k1.participant_problems.as_ref().unwrap()["mailto:external@example.org"].extra["rejectCode"],
        554
    );
    assert_eq!(
        send_resp.not_sent.as_ref().unwrap()[&Id::new("k2")].error_type,
        "cannotSendForCalendar"
    );

    // 3. ParticipantReply with unknown fields
    let reply_payload = json!({
        "calendarEventId": "evt_999",
        "recurrenceId": "2026-09-01T14:00:00",
        "participationStatus": "accepted",
        "comment": "Joining in person",
        "sendTo": {
            "imip": "mailto:organizer@example.com"
        },
        "clientAgent": "Evolution-JMAP/0.3.0"
    });

    let reply: ParticipantReply = serde_json::from_value(reply_payload)
        .expect("ParticipantReply deserializes with extensions");
    assert_eq!(reply.calendar_event_id.as_str(), "evt_999");
    assert_eq!(reply.recurrence_id.as_deref(), Some("2026-09-01T14:00:00"));
    assert_eq!(reply.participation_status, "accepted");
    assert_eq!(reply.comment.as_deref(), Some("Joining in person"));
    assert_eq!(
        reply.send_to.as_ref().unwrap()["imip"],
        "mailto:organizer@example.com"
    );
    assert_eq!(reply.extra["clientAgent"], "Evolution-JMAP/0.3.0");

    // 4. ShareNotificationQueryFilter with unknown fields
    let share_filter_payload = json!({
        "after": "2026-09-01T00:00:00Z",
        "before": "2026-09-02T00:00:00Z",
        "objectType": "Calendar",
        "includeSubscribed": true
    });

    let share_filter: ShareNotificationQueryFilter = serde_json::from_value(share_filter_payload)
        .expect("ShareNotificationQueryFilter deserializes with extensions");
    assert_eq!(
        share_filter.after.as_ref().unwrap().as_str(),
        "2026-09-01T00:00:00Z"
    );
    assert_eq!(
        share_filter.before.as_ref().unwrap().as_str(),
        "2026-09-02T00:00:00Z"
    );
    assert_eq!(share_filter.object_type.as_deref(), Some("Calendar"));
    assert_eq!(share_filter.extra["includeSubscribed"], true);
}

#[test]
fn principals_owner_and_share_notification_forward_compatibility() {
    use jmap_proto::methods::SetResponse;
    use jmap_proto::principals::{PrincipalsOwnerCapability, ShareNotification};

    // 1. PrincipalsOwnerCapability with unknown fields
    let owner_cap_payload = json!({
        "accountIdForPrincipal": "acc_principals_main",
        "principalId": "p_enterprise_root",
        "delegationAllowed": true,
        "maxDelegatedAccounts": 50
    });

    let owner_cap: PrincipalsOwnerCapability = serde_json::from_value(owner_cap_payload)
        .expect("PrincipalsOwnerCapability deserializes with extensions");
    assert_eq!(
        owner_cap.account_id_for_principal.as_str(),
        "acc_principals_main"
    );
    assert_eq!(owner_cap.principal_id.as_str(), "p_enterprise_root");
    assert_eq!(owner_cap.extra["delegationAllowed"], true);
    assert_eq!(owner_cap.extra["maxDelegatedAccounts"], 50);

    // 2. ShareNotification with name and unknown fields
    let notif_payload = json!({
        "id": "sn_calendar_shared",
        "created": "2026-09-01T15:30:00Z",
        "changedBy": {
            "id": "p_admin",
            "name": "Admin Coordinator",
            "email": "admin@example.com"
        },
        "objectType": "Calendar",
        "objectId": "cal_marketing",
        "accountId": "acc_user",
        "name": "Marketing Strategy Calendar",
        "oldRights": null,
        "newRights": {
            "mayReadItems": true,
            "mayAddItems": true
        },
        "notificationPriority": "high",
        "autoAccept": true
    });

    let notif: ShareNotification = serde_json::from_value(notif_payload)
        .expect("ShareNotification deserializes with name and extensions");
    assert_eq!(notif.id.as_ref().unwrap().as_str(), "sn_calendar_shared");
    assert_eq!(notif.created.as_str(), "2026-09-01T15:30:00Z");
    assert_eq!(notif.name.as_deref(), Some("Marketing Strategy Calendar"));
    assert_eq!(notif.object_type, "Calendar");
    assert_eq!(notif.object_id.as_str(), "cal_marketing");
    assert_eq!(notif.account_id.as_str(), "acc_user");
    assert_eq!(notif.changed_by.as_ref().unwrap().name, "Admin Coordinator");
    assert_eq!(notif.extra["notificationPriority"], "high");
    assert_eq!(notif.extra["autoAccept"], true);

    // 3. SetResponse forward compatibility
    let set_resp_payload = json!({
        "accountId": "acc_user",
        "oldState": "state_1",
        "newState": "state_2",
        "created": {
            "c1": { "id": "obj_1", "name": "Created 1" }
        },
        "updated": {
            "u1": null
        },
        "destroyed": ["d1"],
        "notCreated": {
            "c2": { "type": "invalidProperties", "description": "Invalid title" }
        },
        "serverLatencyMs": 14
    });

    let set_resp: SetResponse<serde_json::Value> =
        serde_json::from_value(set_resp_payload).expect("SetResponse deserializes cleanly");
    assert_eq!(set_resp.account_id.as_str(), "acc_user");
    assert_eq!(set_resp.old_state.as_ref().unwrap().as_str(), "state_1");
    assert_eq!(set_resp.new_state.as_str(), "state_2");
    assert!(set_resp.created.as_ref().unwrap().contains_key("c1"));
    assert!(
        set_resp
            .updated
            .as_ref()
            .unwrap()
            .contains_key(&Id::new("u1"))
    );
    assert_eq!(set_resp.destroyed.as_ref().unwrap(), &vec![Id::new("d1")]);
    assert_eq!(
        set_resp.not_created.as_ref().unwrap()["c2"].error_type,
        "invalidProperties"
    );
}

// ===========================================================================
// RFC 8620 §7 Push & EventSource: Forward Compatibility & Conformance
// ===========================================================================

#[test]
fn push_subscription_verification_and_state_change_forward_compatibility() {
    use jmap_proto::push::{PushSubscription, PushVerification, StateChange};

    // 1. PushSubscription with unknown fields and custom push encryption parameters
    let sub_payload = json!({
        "id": "sub_enterprise_99",
        "deviceClientId": "device_desktop_linux_42",
        "url": "https://push.example.com/endpoints/sub_99",
        "keys": {
            "p256dh": "BNcRdreALRFXTkOOUHK1EtK2wtaz5Ry4YfYCA_0QTpQtUbVlUls0VJXg7A8u-Ts1XbjhazAkj7I99e8QcYP7DkM=",
            "auth": "tBHItJI5svbpez7KI4CCXg==",
            "customKeyCurve": "ed25519"
        },
        "expires": "2026-12-31T23:59:59Z",
        "types": ["Mailbox", "Email", "CalendarEvent", "ContactCard", "CustomVendorTask"],
        "vendorNotificationChannel": "high-priority",
        "retryAttemptsMax": 5
    });

    let sub: PushSubscription = serde_json::from_value(sub_payload)
        .expect("PushSubscription deserializes with unknown fields");
    assert_eq!(sub.id.as_ref().unwrap().as_str(), "sub_enterprise_99");
    assert_eq!(sub.device_client_id, "device_desktop_linux_42");
    assert_eq!(sub.url, "https://push.example.com/endpoints/sub_99");
    let keys = sub.keys.as_ref().expect("keys present");
    assert_eq!(
        keys.p256dh,
        "BNcRdreALRFXTkOOUHK1EtK2wtaz5Ry4YfYCA_0QTpQtUbVlUls0VJXg7A8u-Ts1XbjhazAkj7I99e8QcYP7DkM="
    );
    assert_eq!(keys.auth, "tBHItJI5svbpez7KI4CCXg==");
    assert_eq!(
        sub.expires.as_ref().unwrap().as_str(),
        "2026-12-31T23:59:59Z"
    );
    assert_eq!(sub.types.as_ref().unwrap().len(), 5);
    assert_eq!(sub.extra["vendorNotificationChannel"], "high-priority");
    assert_eq!(sub.extra["retryAttemptsMax"], 5);

    // 2. PushVerification with unknown fields
    let ver_payload = json!({
        "@type": "PushVerification",
        "pushSubscriptionId": "sub_enterprise_99",
        "verificationCode": "verify-code-778899",
        "expiresAt": "2026-09-01T12:00:00Z",
        "serverChallengeNonce": "nonce-445566"
    });

    let ver: PushVerification = serde_json::from_value(ver_payload)
        .expect("PushVerification deserializes with unknown fields");
    assert_eq!(ver.object_type, "PushVerification");
    assert_eq!(ver.push_subscription_id.as_str(), "sub_enterprise_99");
    assert_eq!(ver.verification_code, "verify-code-778899");
    assert_eq!(ver.extra["expiresAt"], "2026-09-01T12:00:00Z");
    assert_eq!(ver.extra["serverChallengeNonce"], "nonce-445566");

    // 3. StateChange with multiple accounts and unknown fields
    let sc_payload = json!({
        "@type": "StateChange",
        "changed": {
            "acc_1": {
                "Mailbox": "state_mbx_10",
                "Email": "state_email_20"
            },
            "acc_2": {
                "CalendarEvent": "state_cal_30",
                "ContactCard": "state_card_40",
                "VendorCustomType": "state_custom_50"
            }
        },
        "eventId": "evt_push_seq_1001",
        "pushedAt": "2026-08-30T14:22:00Z"
    });

    let sc: StateChange =
        serde_json::from_value(sc_payload).expect("StateChange deserializes cleanly");
    assert_eq!(sc.kind, "StateChange");
    assert_eq!(
        sc.changed[&Id::new("acc_1")]["Mailbox"].as_str(),
        "state_mbx_10"
    );
    assert_eq!(
        sc.changed[&Id::new("acc_1")]["Email"].as_str(),
        "state_email_20"
    );
    assert_eq!(
        sc.changed[&Id::new("acc_2")]["CalendarEvent"].as_str(),
        "state_cal_30"
    );
    assert_eq!(
        sc.changed[&Id::new("acc_2")]["ContactCard"].as_str(),
        "state_card_40"
    );
    assert_eq!(
        sc.changed[&Id::new("acc_2")]["VendorCustomType"].as_str(),
        "state_custom_50"
    );
    assert_eq!(sc.extra["eventId"], "evt_push_seq_1001");
    assert_eq!(sc.extra["pushedAt"], "2026-08-30T14:22:00Z");
}

#[test]
fn rfc8620_standard_error_and_enum_constants_exact_values() {
    use jmap_proto::blob::blob_set_error;
    use jmap_proto::error::{method, request, set};
    use jmap_proto::mail::{delivered, displayed, undo_status};
    use jmap_proto::methods::filter_operator;
    use jmap_proto::principals::{
        principal_set_error, principal_type, share_notification_object_type,
    };
    use jmap_proto::push::push_subscription_set_error;
    use jmap_proto::quota::{quota_data_type, quota_resource_type, quota_scope, quota_set_error};

    // Method error types (RFC 8620 §3.6.2)
    assert_eq!(method::UNKNOWN_METHOD, "unknownMethod");
    assert_eq!(method::INVALID_ARGUMENTS, "invalidArguments");
    assert_eq!(method::INVALID_RESULT_REFERENCE, "invalidResultReference");
    assert_eq!(method::FORBIDDEN, "forbidden");
    assert_eq!(method::ACCOUNT_NOT_FOUND, "accountNotFound");
    assert_eq!(
        method::ACCOUNT_NOT_SUPPORTED_BY_METHOD,
        "accountNotSupportedByMethod"
    );
    assert_eq!(method::ACCOUNT_READ_ONLY, "accountReadOnly");
    assert_eq!(method::SERVER_FAIL, "serverFail");
    assert_eq!(method::SERVER_UNAVAILABLE, "serverUnavailable");
    assert_eq!(method::SERVER_PARTIAL_FAIL, "serverPartialFail");
    assert_eq!(method::UNKNOWN_CAPABILITY, "unknownCapability");
    assert_eq!(method::STATE_MISMATCH, "stateMismatch");
    assert_eq!(method::FROM_STATE_MISMATCH, "fromStateMismatch");
    assert_eq!(method::CANNOT_CALCULATE_CHANGES, "cannotCalculateChanges");
    assert_eq!(method::REQUEST_TOO_LARGE, "requestTooLarge");

    // Set error types (RFC 8620 §5.3, §5.4)
    assert_eq!(set::FORBIDDEN, "forbidden");
    assert_eq!(set::OVER_QUOTA, "overQuota");
    assert_eq!(set::TOO_LARGE, "tooLarge");
    assert_eq!(set::RATE_LIMIT, "rateLimit");
    assert_eq!(set::NOT_FOUND, "notFound");
    assert_eq!(set::INVALID_PATCH, "invalidPatch");
    assert_eq!(set::INVALID_PROPERTIES, "invalidProperties");
    assert_eq!(set::SINGLETON, "singleton");
    assert_eq!(set::WILL_DESTROY, "willDestroy");
    assert_eq!(set::STATE_MISMATCH, "stateMismatch");
    assert_eq!(set::REQUEST_TOO_LARGE, "requestTooLarge");
    assert_eq!(set::ALREADY_EXISTS, "alreadyExists");
    assert_eq!(set::CANNOT_DESTROY_ORIGINAL, "cannotDestroyOriginal");

    // Request problem types (RFC 8620 §3.6.1)
    assert_eq!(
        request::UNKNOWN_CAPABILITY,
        "urn:ietf:params:jmap:error:unknownCapability"
    );
    assert_eq!(request::NOT_JSON, "urn:ietf:params:jmap:error:notJSON");
    assert_eq!(
        request::NOT_REQUEST,
        "urn:ietf:params:jmap:error:notRequest"
    );
    assert_eq!(request::LIMIT, "urn:ietf:params:jmap:error:limit");

    // Filter operators (RFC 8620 §5.5)
    assert_eq!(filter_operator::AND, "AND");
    assert_eq!(filter_operator::OR, "OR");
    assert_eq!(filter_operator::NOT, "NOT");

    // Push subscription set errors (RFC 8620 §7.2.1)
    assert_eq!(push_subscription_set_error::INVALID_URL, "invalidUrl");
    assert_eq!(
        push_subscription_set_error::EXPIRES_TOO_FAR,
        "expiresTooFar"
    );

    // Undo status values (RFC 8621 §7)
    assert_eq!(undo_status::PENDING, "pending");
    assert_eq!(undo_status::FINAL, "final");
    assert_eq!(undo_status::CANCELED, "canceled");

    // Delivery and Displayed status values (RFC 8621 §7.1.1)
    assert_eq!(delivered::QUEUED, "queued");
    assert_eq!(delivered::YES, "yes");
    assert_eq!(delivered::NO, "no");
    assert_eq!(delivered::UNKNOWN, "unknown");
    assert_eq!(displayed::UNKNOWN, "unknown");
    assert_eq!(displayed::YES, "yes");

    // Principal types and set errors (RFC 9670 §2)
    assert_eq!(principal_type::INDIVIDUAL, "individual");
    assert_eq!(principal_type::GROUP, "group");
    assert_eq!(principal_type::RESOURCE, "resource");
    assert_eq!(principal_type::LOCATION, "location");
    assert_eq!(principal_type::OTHER, "other");
    assert_eq!(principal_set_error::FORBIDDEN, "forbidden");
    assert_eq!(
        principal_set_error::PRINCIPAL_ALREADY_EXISTS,
        "principalAlreadyExists"
    );
    assert_eq!(principal_set_error::INVALID_PROPERTIES, "invalidProperties");

    // Share notification object types (RFC 9670 §4)
    assert_eq!(share_notification_object_type::ADDRESS_BOOK, "AddressBook");
    assert_eq!(share_notification_object_type::CALENDAR, "Calendar");
    assert_eq!(share_notification_object_type::MAILBOX, "Mailbox");

    // Blob set errors (RFC 9404 §4)
    assert_eq!(blob_set_error::BLOB_NOT_FOUND, "blobNotFound");
    assert_eq!(blob_set_error::TOO_LARGE, "tooLarge");

    // Quota constants (RFC 9425 §2, §5)
    assert_eq!(quota_resource_type::OCTETS, "octets");
    assert_eq!(quota_resource_type::COUNT, "count");
    assert_eq!(quota_scope::ACCOUNT, "account");
    assert_eq!(quota_scope::DOMAIN, "domain");
    assert_eq!(quota_scope::GLOBAL, "global");
    assert_eq!(quota_data_type::MAIL, "Mail");
    assert_eq!(quota_data_type::CONTACTS, "Contacts");
    assert_eq!(quota_data_type::CALENDARS, "Calendars");
    assert_eq!(quota_set_error::OVER_QUOTA, "overQuota");
}

#[test]
fn rfc9219_smime_signature_verification_forward_compatibility() {
    use jmap_proto::mail::{Email, EmailQueryFilter, smime_status};
    use jmap_proto::session::{CAPABILITY_SMIME_VERIFY, Session};

    // 1. Session with smimeverify capability and vendor options
    let session_json = json!({
        "capabilities": {
            "urn:ietf:params:jmap:core": {
                "maxSizeUpload": 10000000,
                "maxConcurrentUpload": 4,
                "maxSizeRequest": 10000000,
                "maxConcurrentRequests": 4,
                "maxCallsInRequest": 16,
                "maxObjectsInGet": 500,
                "maxObjectsInSet": 500,
                "collationAlgorithms": ["i;ascii-casemap", "i;unicode-casemap"]
            },
            "urn:ietf:params:jmap:smimeverify": {
                "maxCacheTtlSeconds": 86400,
                "trustedAnchorSubjectList": ["CN=Example Root CA,O=Example Corp,C=US"],
                "vendorHardwareSecurityModule": "enabled"
            }
        },
        "accounts": {
            "acc_1": {
                "name": "Secure Mail Account",
                "isPersonal": true,
                "isReadOnly": false,
                "accountCapabilities": {
                    "urn:ietf:params:jmap:mail": {},
                    "urn:ietf:params:jmap:smimeverify": {}
                }
            }
        },
        "primaryAccounts": {
            "urn:ietf:params:jmap:mail": "acc_1"
        },
        "username": "alice@secure.example.com",
        "apiUrl": "https://secure.example.com/jmap/api",
        "downloadUrl": "https://secure.example.com/jmap/download/{blobId}",
        "uploadUrl": "https://secure.example.com/jmap/upload/{accountId}",
        "state": "stat_smime_01"
    });

    let session: Session = serde_json::from_value(session_json).expect("Session with smimeverify");
    assert!(session.capabilities.contains_key(CAPABILITY_SMIME_VERIFY));
    let smime_cap = session
        .smime_verify_capability()
        .expect("typed smime capability");
    assert_eq!(smime_cap.extra["maxCacheTtlSeconds"], 86400);
    assert_eq!(smime_cap.extra["vendorHardwareSecurityModule"], "enabled");

    // 2. Email payload with S/MIME fields, future extension status, and body parts
    let email_payload = json!({
        "id": "msg_smime_secure_01",
        "blobId": "blob_msg_01",
        "threadId": "th_01",
        "mailboxIds": {
            "mb_inbox": true
        },
        "from": [{"name": "Bob Authenticated", "email": "bob@secure.example.com"}],
        "subject": "Confidential Q3 Security Report",
        "smimeStatus": "encrypted+signed/verified",
        "smimeErrors": [
            "Warning: CRL was cached 12 hours ago",
            "Notice: Intermediate certificate stapled in payload"
        ],
        "smimeVerifiedAt": "2026-08-30T07:15:00Z",
        "smimeValidationPolicy": "RFC5280-Strict-Enterprise",
        "bodyStructure": {
            "partId": "part_1",
            "blobId": "blob_part_1",
            "size": 1024,
            "type": "text/plain",
            "smimeStatus": "signed/verified",
            "smimeErrors": [],
            "smimeVerifiedAt": "2026-08-30T07:15:00Z",
            "subParts": [
                {
                    "partId": "part_nested_1",
                    "type": "application/pdf",
                    "name": "Audit_Report.pdf",
                    "smimeStatus": "signed/verified",
                    "customSignatureDigest": "sha256:abcd1234ef5678"
                }
            ]
        }
    });

    let email: Email = serde_json::from_value(email_payload).expect("Email with smime fields");
    assert_eq!(email.id.as_ref().unwrap().as_str(), "msg_smime_secure_01");
    assert_eq!(
        email.smime_status.as_deref(),
        Some("encrypted+signed/verified")
    );
    assert_eq!(email.smime_errors.as_ref().unwrap().len(), 2);
    assert_eq!(
        email.smime_verified_at.as_ref().unwrap().as_str(),
        "2026-08-30T07:15:00Z"
    );
    assert_eq!(
        email.extra["smimeValidationPolicy"],
        "RFC5280-Strict-Enterprise"
    );

    let body = email.body_structure.as_ref().unwrap();
    assert_eq!(
        body.smime_status.as_deref(),
        Some(smime_status::SIGNED_VERIFIED)
    );
    assert_eq!(
        body.smime_verified_at.as_ref().unwrap().as_str(),
        "2026-08-30T07:15:00Z"
    );
    let nested = &body.sub_parts.as_ref().unwrap()[0];
    assert_eq!(
        nested.smime_status.as_deref(),
        Some(smime_status::SIGNED_VERIFIED)
    );
    assert_eq!(
        nested.extra["customSignatureDigest"],
        "sha256:abcd1234ef5678"
    );

    // 3. EmailQueryFilter with hasSmime and hasVerifiedSmime and forward extension
    let filter_payload = json!({
        "inMailbox": "mb_inbox",
        "hasSmime": true,
        "hasVerifiedSmime": true,
        "customExtensionFilter": "requireHardwareToken"
    });

    let filter: EmailQueryFilter =
        serde_json::from_value(filter_payload).expect("EmailQueryFilter with smime filters");
    assert_eq!(filter.has_smime, Some(true));
    assert_eq!(filter.has_verified_smime, Some(true));

    // 4. Exact wire values for smime_status constants
    assert_eq!(smime_status::UNKNOWN, "unknown");
    assert_eq!(smime_status::SIGNED, "signed");
    assert_eq!(smime_status::SIGNED_VERIFIED, "signed/verified");
    assert_eq!(smime_status::SIGNED_FAILED, "signed/failed");
}

#[test]
fn rfc8887_websocket_forward_compatibility_and_conformance() {
    use jmap_proto::session::{CAPABILITY_WEBSOCKET, Session};
    use jmap_proto::websocket::{
        SUBPROTOCOL, WebSocketPushDisable, WebSocketPushEnable, WebSocketRequest,
        WebSocketRequestError, WebSocketResponse, message_type,
    };

    // 1. Session with websocket capability and vendor options
    let session_json = json!({
        "capabilities": {
            "urn:ietf:params:jmap:core": {
                "maxSizeUpload": 10000000,
                "maxConcurrentUpload": 4,
                "maxSizeRequest": 10000000,
                "maxConcurrentRequests": 4,
                "maxCallsInRequest": 16,
                "maxObjectsInGet": 500,
                "maxObjectsInSet": 500,
                "collationAlgorithms": ["i;ascii-casemap", "i;unicode-casemap"]
            },
            "urn:ietf:params:jmap:websocket": {
                "url": "wss://mail.example.com/jmap/ws",
                "supportsPush": true,
                "pingIntervalSeconds": 30,
                "maxFrameSize": 65536
            }
        },
        "accounts": {
            "acc_ws_1": {
                "name": "WebSocket Account",
                "isPersonal": true,
                "isReadOnly": false,
                "accountCapabilities": {
                    "urn:ietf:params:jmap:mail": {}
                }
            }
        },
        "primaryAccounts": {},
        "username": "alice@example.com",
        "apiUrl": "https://mail.example.com/jmap/api",
        "downloadUrl": "https://mail.example.com/jmap/download/{blobId}",
        "uploadUrl": "https://mail.example.com/jmap/upload/{accountId}",
        "state": "stat_ws_01"
    });

    let session: Session = serde_json::from_value(session_json).expect("Session with websocket");
    assert!(session.capabilities.contains_key(CAPABILITY_WEBSOCKET));
    let ws_cap = session
        .websocket_capability()
        .expect("typed websocket capability");
    assert_eq!(ws_cap.url, "wss://mail.example.com/jmap/ws");
    assert!(ws_cap.supports_push);
    assert_eq!(ws_cap.extra["pingIntervalSeconds"], 30);
    assert_eq!(ws_cap.extra["maxFrameSize"], 65536);

    // 2. WebSocket Request frame with forward extension members
    let request_json = json!({
        "@type": "Request",
        "id": "ws-req-001",
        "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
        "methodCalls": [
            ["Mailbox/get", {"accountId": "acc_ws_1", "ids": ["mb_inbox"]}, "call_01"]
        ],
        "createdIds": {
            "tmp_k1": "real_id_1"
        },
        "clientTimestamp": 1756540000,
        "compression": "deflate"
    });

    let req: WebSocketRequest =
        serde_json::from_value(request_json).expect("WebSocketRequest deserialize");
    assert_eq!(req.message_type, "Request");
    assert_eq!(req.id.as_deref(), Some("ws-req-001"));
    assert_eq!(req.using.len(), 2);
    assert_eq!(req.method_calls.len(), 1);
    assert_eq!(req.method_calls[0].name, "Mailbox/get");
    assert_eq!(req.method_calls[0].call_id, "call_01");
    assert_eq!(
        req.created_ids
            .as_ref()
            .unwrap()
            .get(&jmap_proto::id::Id::from("tmp_k1")),
        Some(&jmap_proto::id::Id::from("real_id_1"))
    );
    assert_eq!(req.extra["clientTimestamp"], 1756540000);
    assert_eq!(req.extra["compression"], "deflate");

    // 3. WebSocket Response frame with forward extension members
    let response_json = json!({
        "@type": "Response",
        "id": "ws-req-001",
        "methodResponses": [
            ["Mailbox/get", {
                "accountId": "acc_ws_1",
                "state": "state_mb_01",
                "list": [{"id": "mb_inbox", "name": "Inbox"}]
            }, "call_01"]
        ],
        "sessionState": "session_state_updated",
        "createdIds": {
            "tmp_k1": "real_id_1"
        },
        "serverLatencyMs": 5,
        "workerNode": "worker-eu-central-1"
    });

    let resp: WebSocketResponse =
        serde_json::from_value(response_json).expect("WebSocketResponse deserialize");
    assert_eq!(resp.message_type, "Response");
    assert_eq!(resp.id.as_deref(), Some("ws-req-001"));
    assert_eq!(resp.method_responses.len(), 1);
    assert_eq!(
        resp.session_state.as_ref().map(|s| s.as_str()),
        Some("session_state_updated")
    );
    assert_eq!(resp.extra["serverLatencyMs"], 5);
    assert_eq!(resp.extra["workerNode"], "worker-eu-central-1");

    // 4. WebSocket RequestError frame with forward extension members
    let error_json = json!({
        "@type": "RequestError",
        "id": "ws-req-failed",
        "type": "urn:ietf:params:jmap:error:limit",
        "status": 400,
        "detail": "Rate limit exceeded for client",
        "retryAfterSeconds": 15,
        "vendorQuotaGroup": "tier-standard"
    });

    let err: WebSocketRequestError =
        serde_json::from_value(error_json).expect("WebSocketRequestError deserialize");
    assert_eq!(err.message_type, "RequestError");
    assert_eq!(err.id.as_deref(), Some("ws-req-failed"));
    assert_eq!(err.error_type, "urn:ietf:params:jmap:error:limit");
    assert_eq!(err.status, Some(400));
    assert_eq!(
        err.detail.as_deref(),
        Some("Rate limit exceeded for client")
    );
    assert_eq!(err.extra["retryAfterSeconds"], 15);
    assert_eq!(err.extra["vendorQuotaGroup"], "tier-standard");

    // 5. WebSocketPushEnable & WebSocketPushDisable frames
    let push_enable_json = json!({
        "@type": "WebSocketPushEnable",
        "dataTypes": ["Email", "Mailbox", "Thread"],
        "clientSubscriptionId": "sub_push_001"
    });
    let push_enable: WebSocketPushEnable =
        serde_json::from_value(push_enable_json).expect("WebSocketPushEnable deserialize");
    assert_eq!(push_enable.message_type, "WebSocketPushEnable");
    assert_eq!(
        push_enable.data_types.as_ref().unwrap(),
        &vec![
            "Email".to_string(),
            "Mailbox".to_string(),
            "Thread".to_string()
        ]
    );
    assert_eq!(push_enable.extra["clientSubscriptionId"], "sub_push_001");

    let push_disable_json = json!({
        "@type": "WebSocketPushDisable",
        "reason": "user_logout"
    });
    let push_disable: WebSocketPushDisable =
        serde_json::from_value(push_disable_json).expect("WebSocketPushDisable deserialize");
    assert_eq!(push_disable.message_type, "WebSocketPushDisable");
    assert_eq!(push_disable.extra["reason"], "user_logout");

    // 6. Exact wire constants
    assert_eq!(SUBPROTOCOL, "jmap");
    assert_eq!(message_type::REQUEST, "Request");
    assert_eq!(message_type::RESPONSE, "Response");
    assert_eq!(message_type::REQUEST_ERROR, "RequestError");
    assert_eq!(message_type::PUSH_ENABLE, "WebSocketPushEnable");
    assert_eq!(message_type::PUSH_DISABLE, "WebSocketPushDisable");
    assert_eq!(message_type::STATE_CHANGE, "StateChange");
}

#[test]
fn filenode_and_refplus_forward_compatibility_and_conformance() {
    use jmap_proto::filenode::{
        FileNode, FileNodeCapability, FileNodeQueryFilter, filenode_set_error, node_role, node_type,
    };
    use jmap_proto::request::ResultReference;
    use jmap_proto::session::{
        CAPABILITY_FILENODE, CAPABILITY_REFPLUS, RefPlusCapability, Session,
    };

    // 1. Session carrying FileNode and RefPlus capabilities with vendor extensions
    let session_json = json!({
        "capabilities": {
            "urn:ietf:params:jmap:core": {
                "maxSizeUpload": 10000000,
                "maxConcurrentUpload": 4,
                "maxSizeRequest": 10000000,
                "maxConcurrentRequests": 4,
                "maxCallsInRequest": 16,
                "maxObjectsInGet": 500,
                "maxObjectsInSet": 500,
                "collationAlgorithms": ["i;ascii-casemap", "i;unicode-casemap"]
            },
            "urn:ietf:params:jmap:filenode": {
                "maxFileNodeDepth": 32,
                "maxSizeFileNodeName": 1024,
                "fileNodeQuerySortOptions": ["name", "size", "modified", "customVendorRank"],
                "vendorCloudBackend": "ceph-s3-tier"
            },
            "urn:ietf:params:jmap:refplus": {
                "jsonPath": true,
                "filterCondition": true,
                "setProperty": true,
                "maxJsonPathLength": 4096
            }
        },
        "accounts": {
            "acc_storage_1": {
                "name": "Cloud Storage",
                "isPersonal": true,
                "isReadOnly": false,
                "accountCapabilities": {
                    "urn:ietf:params:jmap:filenode": {
                        "maxFileNodeDepth": 32,
                        "maxSizeFileNodeName": 1024
                    }
                }
            }
        },
        "primaryAccounts": {},
        "username": "alice@storage.example.com",
        "apiUrl": "https://storage.example.com/jmap/api",
        "downloadUrl": "https://storage.example.com/jmap/download/{blobId}",
        "uploadUrl": "https://storage.example.com/jmap/upload/{accountId}",
        "state": "stat_storage_01"
    });

    let session: Session =
        serde_json::from_value(session_json).expect("Session with filenode and refplus");
    assert!(session.capabilities.contains_key(CAPABILITY_FILENODE));
    assert!(session.capabilities.contains_key(CAPABILITY_REFPLUS));

    let filenode_cap: FileNodeCapability = session
        .filenode_capability()
        .expect("typed filenode capability");
    assert_eq!(filenode_cap.max_file_node_depth, Some(32));
    assert_eq!(filenode_cap.max_size_file_node_name, Some(1024));
    assert_eq!(
        filenode_cap.file_node_query_sort_options,
        vec!["name", "size", "modified", "customVendorRank"]
    );
    assert_eq!(filenode_cap.extra["vendorCloudBackend"], "ceph-s3-tier");

    let refplus_cap: RefPlusCapability = session
        .refplus_capability()
        .expect("typed refplus capability");
    assert_eq!(refplus_cap.json_path, Some(true));
    assert_eq!(refplus_cap.filter_condition, Some(true));
    assert_eq!(refplus_cap.set_property, Some(true));
    assert_eq!(refplus_cap.extra["maxJsonPathLength"], 4096);

    // 2. FileNode with unknown fields, custom node types and roles, and nested rights extensions
    let filenode_payload = json!({
        "id": "fn_backup_tar",
        "parentId": "fn_dir_backups",
        "name": "system_state_20260830.tar.gz",
        "blobId": "blob_archive_7788",
        "size": 52428800,
        "nodeType": "file",
        "nodeRole": "custom-archived-snapshot",
        "created": "2026-08-30T06:00:00Z",
        "modified": "2026-08-30T06:30:00Z",
        "executable": false,
        "myRights": {
            "mayRead": true,
            "mayWrite": false,
            "mayAdmin": false,
            "mayModifyContent": false,
            "maySharePublicly": true
        },
        "sha256": "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        "vendorEncryptionAlgorithm": "AES-GCM-256",
        "retentionPolicyDays": 90
    });

    let node: FileNode =
        serde_json::from_value(filenode_payload).expect("FileNode with unknown fields");
    assert_eq!(node.id.as_str(), "fn_backup_tar");
    assert_eq!(node.parent_id.as_ref().unwrap().as_str(), "fn_dir_backups");
    assert_eq!(node.name, "system_state_20260830.tar.gz");
    assert_eq!(node.blob_id.as_ref().unwrap().as_str(), "blob_archive_7788");
    assert_eq!(node.size, Some(52428800));
    assert_eq!(node.node_type, "file");
    assert_eq!(node.node_role.as_deref(), Some("custom-archived-snapshot"));
    assert_eq!(
        node.created.as_ref().unwrap().as_str(),
        "2026-08-30T06:00:00Z"
    );
    assert_eq!(
        node.modified.as_ref().unwrap().as_str(),
        "2026-08-30T06:30:00Z"
    );
    assert_eq!(node.executable, Some(false));
    let rights = node.my_rights.as_ref().unwrap();
    assert_eq!(rights.may_read, Some(true));
    assert_eq!(rights.may_write, Some(false));
    assert_eq!(rights.extra["maySharePublicly"], true);
    assert_eq!(node.extra["vendorEncryptionAlgorithm"], "AES-GCM-256");
    assert_eq!(node.extra["retentionPolicyDays"], 90);

    // 3. FileNodeQueryFilter with custom extension criteria
    let filter_payload = json!({
        "parentId": "fn_dir_backups",
        "descendantId": "fn_backup_tar",
        "hasParentId": true,
        "name": "system_state",
        "nodeType": "file",
        "role": "custom-archived-snapshot",
        "isExecutable": false,
        "hasBlob": true,
        "vendorTagFilter": "production-critical"
    });

    let filter: FileNodeQueryFilter =
        serde_json::from_value(filter_payload).expect("FileNodeQueryFilter with custom extensions");
    assert_eq!(
        filter.parent_id.as_ref().unwrap().as_str(),
        "fn_dir_backups"
    );
    assert_eq!(
        filter.descendant_id.as_ref().unwrap().as_str(),
        "fn_backup_tar"
    );
    assert_eq!(filter.has_parent_id, Some(true));
    assert_eq!(filter.name.as_deref(), Some("system_state"));
    assert_eq!(filter.node_type.as_deref(), Some("file"));
    assert_eq!(filter.role.as_deref(), Some("custom-archived-snapshot"));
    assert_eq!(filter.is_executable, Some(false));
    assert_eq!(filter.has_blob, Some(true));
    assert_eq!(filter.extra["vendorTagFilter"], "production-critical");

    // 4. ResultReference with JSONPath
    let res_ref_payload = json!({
        "resultOf": "FileNode/query",
        "name": "ids",
        "path": "$[?(@.size > 1048576)].id",
        "fallbackValue": []
    });
    let res_ref: ResultReference =
        serde_json::from_value(res_ref_payload).expect("ResultReference with jsonPath");
    assert_eq!(res_ref.result_of, "FileNode/query");
    assert_eq!(res_ref.name, "ids");
    assert_eq!(res_ref.path, "$[?(@.size > 1048576)].id");

    // 5. Constants exact wire values
    assert_eq!(node_type::FILE, "file");
    assert_eq!(node_type::DIRECTORY, "directory");
    assert_eq!(node_type::SYMLINK, "symlink");
    assert_eq!(node_type::OTHER, "other");

    assert_eq!(node_role::ROOT, "root");
    assert_eq!(node_role::HOME, "home");
    assert_eq!(node_role::TRASH, "trash");
    assert_eq!(node_role::DOCUMENTS, "documents");
    assert_eq!(node_role::PICTURES, "pictures");
    assert_eq!(node_role::VIDEOS, "videos");
    assert_eq!(node_role::MUSIC, "music");
    assert_eq!(node_role::DOWNLOADS, "downloads");

    assert_eq!(filenode_set_error::NODE_HAS_CHILDREN, "nodeHasChildren");
    assert_eq!(filenode_set_error::ALREADY_EXISTS, "alreadyExists");
    assert_eq!(filenode_set_error::INVALID_NODE_TYPE, "invalidNodeType");
}

#[test]
fn metadata_and_mail_sharing_forward_compatibility_and_conformance() {
    use jmap_proto::metadata::MetadataFilterCondition;

    // 1. Session with metadata and mail sharing capabilities
    let session_payload = json!({
        "capabilities": {
            "urn:ietf:params:jmap:core": {},
            "urn:ietf:params:jmap:metadata": {
                "dataTypes": {
                    "Email": {
                        "namespaces": ["urn:ietf:params:jmap:metadata:notes"],
                        "supportsVendorNamespaces": true,
                        "supportsPrivate": true,
                        "maxDepth": 4,
                        "customDataTypeParam": "active"
                    },
                    "ContactCard": {
                        "namespaces": [],
                        "supportsVendorNamespaces": false,
                        "supportsPrivate": true,
                        "maxDepth": 2
                    }
                },
                "serverGlobalMetadataLimit": 1048576
            },
            "urn:ietf:params:jmap:mail:share": {
                "administerViaAcl": true
            }
        },
        "accounts": {},
        "primaryAccounts": {},
        "username": "user@example.com",
        "apiUrl": "https://api.example.com/jmap/",
        "downloadUrl": "https://api.example.com/download/{blobId}",
        "uploadUrl": "https://api.example.com/upload/",
        "state": "s1"
    });

    let session: Session = serde_json::from_value(session_payload)
        .expect("deserializes Session with metadata and mail share");

    let meta_cap = session
        .metadata_capability()
        .expect("has metadata capability");
    assert_eq!(meta_cap.data_types.len(), 2);
    let email_info = &meta_cap.data_types["Email"];
    assert_eq!(
        email_info.namespaces,
        vec!["urn:ietf:params:jmap:metadata:notes".to_string()]
    );
    assert!(email_info.supports_vendor_namespaces);
    assert!(email_info.supports_private);
    assert_eq!(email_info.max_depth, Some(4));
    assert_eq!(email_info.extra["customDataTypeParam"], "active");
    assert_eq!(meta_cap.extra["serverGlobalMetadataLimit"], 1048576);

    let mail_share_cap = session
        .mail_share_capability()
        .expect("has mail share capability");
    assert_eq!(mail_share_cap.extra["administerViaAcl"], true);

    // 2. Mailbox with shareWith and mayShare forward-compatibility
    let mailbox_payload = json!({
        "id": "mb_shared_projects",
        "name": "Shared Projects",
        "shareWith": {
            "p_alice": {
                "mayReadItems": true,
                "mayAddItems": true,
                "mayRemoveItems": false,
                "maySetSeen": true,
                "maySetKeywords": true,
                "mayCreateChild": false,
                "mayRename": false,
                "mayDelete": false,
                "maySubmit": true,
                "mayShare": true,
                "customAclRight": "audit"
            },
            "p_auditor": {
                "mayReadItems": true,
                "mayShare": false
            }
        },
        "myRights": {
            "mayReadItems": true,
            "mayShare": true
        },
        "vendorSyncPolicy": "realtime"
    });

    let mbx: Mailbox =
        serde_json::from_value(mailbox_payload).expect("Mailbox with shareWith and rights");
    assert_eq!(mbx.id.as_ref().unwrap().as_str(), "mb_shared_projects");
    assert_eq!(mbx.name, "Shared Projects");
    let share_map = mbx.share_with.as_ref().unwrap();
    assert_eq!(share_map.len(), 2);
    let alice_rights = share_map.get(&"p_alice".into()).unwrap();
    assert_eq!(alice_rights.may_read_items, Some(true));
    assert_eq!(alice_rights.may_share, Some(true));
    assert!(alice_rights.may_share());
    assert_eq!(alice_rights.extra["customAclRight"], "audit");

    let my_rights = mbx.my_rights.as_ref().unwrap();
    assert_eq!(my_rights.may_read_items, Some(true));
    assert_eq!(my_rights.may_share, Some(true));
    assert!(my_rights.may_share());

    assert_eq!(mbx.extra["vendorSyncPolicy"], "realtime");

    // 3. MetadataFilterCondition with unknown extension fields
    let filter_payload = json!({
        "metadataExists": "com.vendor.workflow.status",
        "metadataTextContains": {
            "path": "com.vendor.workflow.summary",
            "text": "approved",
            "matchCollation": "i;ascii-casemap"
        },
        "privateMetadataTextEquals": {
            "path": "user.personalNotes",
            "text": "confidential"
        },
        "futureMetadataOperator": "regexMatch"
    });

    let filter: MetadataFilterCondition =
        serde_json::from_value(filter_payload).expect("MetadataFilterCondition");
    assert_eq!(
        filter.metadata_exists.as_deref(),
        Some("com.vendor.workflow.status")
    );
    let text_contains = filter.metadata_text_contains.as_ref().unwrap();
    assert_eq!(text_contains.path, "com.vendor.workflow.summary");
    assert_eq!(text_contains.text, "approved");
    assert_eq!(text_contains.extra["matchCollation"], "i;ascii-casemap");

    let priv_equals = filter.private_metadata_text_equals.as_ref().unwrap();
    assert_eq!(priv_equals.path, "user.personalNotes");
    assert_eq!(priv_equals.text, "confidential");

    assert_eq!(filter.extra["futureMetadataOperator"], "regexMatch");

    // 4. Capability URN constants exact match
    assert_eq!(CAPABILITY_METADATA, "urn:ietf:params:jmap:metadata");
    assert_eq!(CAPABILITY_MAIL_SHARE, "urn:ietf:params:jmap:mail:share");
}
