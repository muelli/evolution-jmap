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
    CAPABILITY_MDN, CAPABILITY_PRINCIPALS, CAPABILITY_QUOTA, CAPABILITY_SIEVE,
    CAPABILITY_SUBMISSION, CAPABILITY_TASKS, CAPABILITY_VACATION_RESPONSE, CAPABILITY_WEBSOCKET,
    Session,
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
