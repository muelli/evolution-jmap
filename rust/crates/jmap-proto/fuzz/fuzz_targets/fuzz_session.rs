// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(session) = serde_json::from_slice::<jmap_proto::session::Session>(data) {
        let _ = session.primary_account(jmap_proto::session::CAPABILITY_CORE);
        let _ = session.primary_account(jmap_proto::session::CAPABILITY_MAIL);
        let _ = session.primary_account(jmap_proto::session::CAPABILITY_CONTACTS);
        let _ = session.primary_account(jmap_proto::session::CAPABILITY_CALENDARS);
        let _ = session.primary_account(jmap_proto::session::CAPABILITY_PRINCIPALS);

        let _ = session.resolve_primary_account(jmap_proto::session::CAPABILITY_CORE);
        let _ = session.resolve_primary_account(jmap_proto::session::CAPABILITY_MAIL);
        let _ = session.resolve_primary_account(jmap_proto::session::CAPABILITY_CONTACTS);
        let _ = session.resolve_primary_account(jmap_proto::session::CAPABILITY_CALENDARS);
        let _ = session.resolve_primary_account(jmap_proto::session::CAPABILITY_PRINCIPALS);

        let _ = session.max_objects_in_get();
        let _ = session.max_calls_in_request();
        let _ = session.max_size_request();
        let _ = session.max_size_upload();
        let _ = session.max_concurrent_upload();
        let _ = session.max_concurrent_requests();
        let _ = session.max_objects_in_set();
        let _ = session.collation_algorithms();

        let _ = session.core_capability();
        let _ = session.mail_capability();
        let _ = session.submission_capability();
        let _ = session.contacts_capability();
        let _ = session.calendars_capability();
        let _ = session.principals_capability();
        let _ = session.principals_owner_capability();
        let _ = session.websocket_capability();
        let _ = session.quota_capability();
        let _ = session.blob_capability();
        let _ = session.tasks_capability();
        let _ = session.sieve_capability();
        let _ = session.mdn_capability();
        let _ = session.vacation_response_capability();
        let _ = session.calendar_preferences_capability();
        let _ = session.smime_verify_capability();
        let _ = session.filenode_capability();
        let _ = session.refplus_capability();
        let _ = session.metadata_capability();
        let _ = session.mail_share_capability();
        let _ = session.principals_availability_capability();
        let _ = session.webpush_vapid_capability();

        for account in session.accounts.values() {
            let _ = account.max_delayed_send();
            let _ = account.has_capability(jmap_proto::session::CAPABILITY_CORE);
            let _ = account.has_capability(jmap_proto::session::CAPABILITY_MAIL);
            let _ = account.has_capability(jmap_proto::session::CAPABILITY_CONTACTS);
            let _ = account.has_capability(jmap_proto::session::CAPABILITY_CALENDARS);
            let _ = account.has_capability(jmap_proto::session::CAPABILITY_PRINCIPALS);
            let _ = account.submission_capability();
        }

        if let Ok(encoded) = serde_json::to_vec(&session) {
            let _ = serde_json::from_slice::<jmap_proto::session::Session>(&encoded);
        }
    }

    if let Ok(account) = serde_json::from_slice::<jmap_proto::session::Account>(data) {
        let _ = account.max_delayed_send();
        let _ = account.has_capability(jmap_proto::session::CAPABILITY_CORE);
        let _ = account.has_capability(jmap_proto::session::CAPABILITY_MAIL);
        let _ = account.has_capability(jmap_proto::session::CAPABILITY_CONTACTS);
        let _ = account.has_capability(jmap_proto::session::CAPABILITY_CALENDARS);
        let _ = account.has_capability(jmap_proto::session::CAPABILITY_PRINCIPALS);
        let _ = account.submission_capability();

        if let Ok(encoded) = serde_json::to_vec(&account) {
            let _ = serde_json::from_slice::<jmap_proto::session::Account>(&encoded);
        }
    }
});
