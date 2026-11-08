// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use libfuzzer_sys::fuzz_target;

const ALL_CAPABILITIES: &[&str] = &[
    jmap_proto::session::CAPABILITY_CORE,
    jmap_proto::session::CAPABILITY_MAIL,
    jmap_proto::session::CAPABILITY_SUBMISSION,
    jmap_proto::session::CAPABILITY_VACATION_RESPONSE,
    jmap_proto::session::CAPABILITY_MDN,
    jmap_proto::session::CAPABILITY_CONTACTS,
    jmap_proto::session::CAPABILITY_CALENDARS,
    jmap_proto::session::CAPABILITY_CALENDAR_PREFERENCES,
    jmap_proto::session::CAPABILITY_PRINCIPALS,
    jmap_proto::session::CAPABILITY_PRINCIPALS_OWNER,
    jmap_proto::session::CAPABILITY_WEBSOCKET,
    jmap_proto::session::CAPABILITY_QUOTA,
    jmap_proto::session::CAPABILITY_BLOB,
    jmap_proto::session::CAPABILITY_TASKS,
    jmap_proto::session::CAPABILITY_SIEVE,
    jmap_proto::session::CAPABILITY_SMIME_VERIFY,
    jmap_proto::session::CAPABILITY_FILENODE,
    jmap_proto::session::CAPABILITY_REFPLUS,
    jmap_proto::session::CAPABILITY_METADATA,
    jmap_proto::session::CAPABILITY_MAIL_SHARE,
    jmap_proto::session::CAPABILITY_PRINCIPALS_AVAILABILITY,
    jmap_proto::session::CAPABILITY_WEBPUSH_VAPID,
    jmap_proto::session::CAPABILITY_CYRUS_MAIL,
];

fuzz_target!(|data: &[u8]| {
    if let Ok(session) = serde_json::from_slice::<jmap_proto::session::Session>(data) {
        for &cap in ALL_CAPABILITIES {
            let _ = session.primary_account(cap);
            let _ = session.resolve_primary_account(cap);
        }

        if let Ok(s) = std::str::from_utf8(data) {
            let _ = session.primary_account(s);
            let _ = session.resolve_primary_account(s);
        }

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

        let built_session = jmap_proto::session::Session::new(
            &session.username,
            &session.api_url,
            &session.download_url,
            &session.upload_url,
            session.state.clone(),
        )
        .with_capabilities(session.capabilities.clone())
        .with_accounts(session.accounts.clone())
        .with_primary_accounts(session.primary_accounts.clone())
        .with_event_source_url(&session.event_source_url)
        .with_extra(session.extra.clone());
        let _ = built_session.primary_account(jmap_proto::session::CAPABILITY_CORE);

        for account in session.accounts.values() {
            let _ = account.max_delayed_send();
            for &cap in ALL_CAPABILITIES {
                let _ = account.has_capability(cap);
            }
            if let Ok(s) = std::str::from_utf8(data) {
                let _ = account.has_capability(s);
            }
            let _ = account.submission_capability();

            let built_account = jmap_proto::session::Account::new(&account.name)
                .is_personal(account.is_personal)
                .is_read_only(account.is_read_only)
                .with_capabilities(account.account_capabilities.clone())
                .with_extra(account.extra.clone());
            let _ = built_account.max_delayed_send();
        }

        if let Ok(encoded) = serde_json::to_vec(&session) {
            let _ = serde_json::from_slice::<jmap_proto::session::Session>(&encoded);
        }
    }

    if let Ok(account) = serde_json::from_slice::<jmap_proto::session::Account>(data) {
        let _ = account.max_delayed_send();
        for &cap in ALL_CAPABILITIES {
            let _ = account.has_capability(cap);
        }
        if let Ok(s) = std::str::from_utf8(data) {
            let _ = account.has_capability(s);
        }
        let _ = account.submission_capability();

        let built_account = jmap_proto::session::Account::new(&account.name)
            .is_personal(account.is_personal)
            .is_read_only(account.is_read_only)
            .with_capabilities(account.account_capabilities.clone())
            .with_extra(account.extra.clone());
        let _ = built_account.max_delayed_send();

        if let Ok(encoded) = serde_json::to_vec(&account) {
            let _ = serde_json::from_slice::<jmap_proto::session::Account>(&encoded);
        }
    }

    if let Ok(cap) = serde_json::from_slice::<jmap_proto::session::CoreCapability>(data) {
        let built = jmap_proto::session::CoreCapability::new()
            .with_max_size_upload(cap.max_size_upload)
            .with_max_concurrent_upload(cap.max_concurrent_upload)
            .with_max_size_request(cap.max_size_request)
            .with_max_concurrent_requests(cap.max_concurrent_requests)
            .with_max_calls_in_request(cap.max_calls_in_request)
            .with_max_objects_in_get(cap.max_objects_in_get)
            .with_max_objects_in_set(cap.max_objects_in_set)
            .with_collation_algorithms(cap.collation_algorithms.clone())
            .with_extra(cap.extra.clone());
        let _ = built.max_size_upload;
        if let Ok(enc) = serde_json::to_vec(&cap) {
            let _ = serde_json::from_slice::<jmap_proto::session::CoreCapability>(&enc);
        }
    }

    if let Ok(ws) = serde_json::from_slice::<jmap_proto::session::WebSocketCapability>(data) {
        let built = jmap_proto::session::WebSocketCapability::new(&ws.url)
            .supports_push(ws.supports_push)
            .with_extra(ws.extra.clone());
        let _ = built.supports_push;
        if let Ok(enc) = serde_json::to_vec(&ws) {
            let _ = serde_json::from_slice::<jmap_proto::session::WebSocketCapability>(&enc);
        }
    }

    if let Ok(rp) = serde_json::from_slice::<jmap_proto::session::RefPlusCapability>(data) {
        let mut built = jmap_proto::session::RefPlusCapability::new().with_extra(rp.extra.clone());
        if let Some(jp) = rp.json_path {
            built = built.with_json_path(jp);
        }
        if let Some(fc) = rp.filter_condition {
            built = built.with_filter_condition(fc);
        }
        if let Some(sp) = rp.set_property {
            built = built.with_set_property(sp);
        }
        let _ = built.json_path;
        if let Ok(enc) = serde_json::to_vec(&rp) {
            let _ = serde_json::from_slice::<jmap_proto::session::RefPlusCapability>(&enc);
        }
    }

    if let Ok(mail) = serde_json::from_slice::<jmap_proto::mail::MailCapability>(data) {
        let built = jmap_proto::mail::MailCapability::new()
            .with_max_size_attachments_per_email(mail.max_size_attachments_per_email)
            .with_max_size_email_in_bytes(mail.max_size_email_in_bytes)
            .with_max_size_body_value_bytes(mail.max_size_body_value_bytes)
            .with_max_number_of_attachments_per_email(mail.max_number_of_attachments_per_email)
            .with_max_number_of_recipients_per_email(mail.max_number_of_recipients_per_email)
            .may_create_top_level_mailbox(mail.may_create_top_level_mailbox)
            .with_extra(mail.extra.clone());
        let _ = built.max_size_email_in_bytes;
        if let Ok(enc) = serde_json::to_vec(&mail) {
            let _ = serde_json::from_slice::<jmap_proto::mail::MailCapability>(&enc);
        }
    }

    if let Ok(sub) = serde_json::from_slice::<jmap_proto::mail::SubmissionCapability>(data) {
        let built = jmap_proto::mail::SubmissionCapability::new()
            .with_max_delayed_send(sub.max_delayed_send)
            .with_submission_extensions(sub.submission_extensions.clone())
            .with_extra(sub.extra.clone());
        let _ = built.max_delayed_send;
        if let Ok(enc) = serde_json::to_vec(&sub) {
            let _ = serde_json::from_slice::<jmap_proto::mail::SubmissionCapability>(&enc);
        }
    }

    if let Ok(vac) = serde_json::from_slice::<jmap_proto::mail::VacationResponseCapability>(data) {
        if let Ok(enc) = serde_json::to_vec(&vac) {
            let _ = serde_json::from_slice::<jmap_proto::mail::VacationResponseCapability>(&enc);
        }
    }

    if let Ok(mdn) = serde_json::from_slice::<jmap_proto::mail::MDNCapability>(data) {
        if let Ok(enc) = serde_json::to_vec(&mdn) {
            let _ = serde_json::from_slice::<jmap_proto::mail::MDNCapability>(&enc);
        }
    }

    if let Ok(contacts) = serde_json::from_slice::<jmap_proto::contacts::ContactsCapability>(data) {
        let built = jmap_proto::contacts::ContactsCapability::new()
            .with_max_size_attachments_per_card(contacts.max_size_attachments_per_card)
            .with_max_number_of_cards_in_set(contacts.max_number_of_cards_in_set)
            .with_extra(contacts.extra.clone());
        let _ = built.max_size_attachments_per_card;
        if let Ok(enc) = serde_json::to_vec(&contacts) {
            let _ = serde_json::from_slice::<jmap_proto::contacts::ContactsCapability>(&enc);
        }
    }

    if let Ok(cals) = serde_json::from_slice::<jmap_proto::calendars::CalendarsCapability>(data) {
        let mut built =
            jmap_proto::calendars::CalendarsCapability::new().with_extra(cals.extra.clone());
        if let Some(m) = cals.max_calendars_per_event {
            built = built.with_max_calendars_per_event(m);
        }
        if let Some(ref dt) = cals.min_date_time {
            built = built.with_min_date_time(dt);
        }
        if let Some(ref dt) = cals.max_date_time {
            built = built.with_max_date_time(dt);
        }
        if let Some(ref dur) = cals.max_expanded_query_duration {
            built = built.with_max_expanded_query_duration(dur);
        }
        if let Some(p) = cals.max_participants_per_event {
            built = built.with_max_participants_per_event(p);
        }
        if let Some(mc) = cals.may_create_calendar {
            built = built.with_may_create_calendar(mc);
        }
        let _ = built.max_calendars_per_event;
        if let Ok(enc) = serde_json::to_vec(&cals) {
            let _ = serde_json::from_slice::<jmap_proto::calendars::CalendarsCapability>(&enc);
        }
    }

    if let Ok(cal_pref) =
        serde_json::from_slice::<jmap_proto::calendars::CalendarPreferencesCapability>(data)
    {
        if let Ok(enc) = serde_json::to_vec(&cal_pref) {
            let _ = serde_json::from_slice::<jmap_proto::calendars::CalendarPreferencesCapability>(
                &enc,
            );
        }
    }

    if let Ok(pr) = serde_json::from_slice::<jmap_proto::principals::PrincipalsCapability>(data) {
        let mut built =
            jmap_proto::principals::PrincipalsCapability::new().with_extra(pr.extra.clone());
        if let Some(ref id) = pr.current_user_principal_id {
            built = built.with_current_user_principal_id(id.clone());
        }
        let _ = built.current_user_principal_id;
        if let Ok(enc) = serde_json::to_vec(&pr) {
            let _ = serde_json::from_slice::<jmap_proto::principals::PrincipalsCapability>(&enc);
        }
    }

    if let Ok(pr_owner) =
        serde_json::from_slice::<jmap_proto::principals::PrincipalsOwnerCapability>(data)
    {
        if let Ok(enc) = serde_json::to_vec(&pr_owner) {
            let _ =
                serde_json::from_slice::<jmap_proto::principals::PrincipalsOwnerCapability>(&enc);
        }
    }

    if let Ok(pr_avail) =
        serde_json::from_slice::<jmap_proto::principals::PrincipalAvailabilityCapability>(data)
    {
        if let Ok(enc) = serde_json::to_vec(&pr_avail) {
            let _ = serde_json::from_slice::<jmap_proto::principals::PrincipalAvailabilityCapability>(
                &enc,
            );
        }
    }

    if let Ok(q) = serde_json::from_slice::<jmap_proto::quota::QuotaCapability>(data) {
        let built = jmap_proto::quota::QuotaCapability::new().with_extra(q.extra.clone());
        let _ = built.extra.len();
        if let Ok(enc) = serde_json::to_vec(&q) {
            let _ = serde_json::from_slice::<jmap_proto::quota::QuotaCapability>(&enc);
        }
    }

    if let Ok(b) = serde_json::from_slice::<jmap_proto::blob::BlobCapability>(data) {
        let mut built = jmap_proto::blob::BlobCapability::new().with_extra(b.extra.clone());
        if let Some(s) = b.max_size_blob_set {
            built = built.with_max_size_blob_set(s);
        }
        if let Some(d) = b.max_data_sources {
            built = built.with_max_data_sources(d);
        }
        if let Some(ref types) = b.supported_type_names {
            built = built.with_supported_type_names(types.clone());
        }
        if let Some(ref algs) = b.supported_digest_algorithms {
            built = built.with_supported_digest_algorithms(algs.clone());
        }
        let _ = built.max_size_blob_set;
        if let Ok(enc) = serde_json::to_vec(&b) {
            let _ = serde_json::from_slice::<jmap_proto::blob::BlobCapability>(&enc);
        }
    }

    if let Ok(t) = serde_json::from_slice::<jmap_proto::tasks::TasksCapability>(data) {
        let mut built = jmap_proto::tasks::TasksCapability::new().with_extra(t.extra.clone());
        if let Some(g) = t.max_tasks_per_get {
            built = built.with_max_tasks_per_get(g);
        }
        if let Some(s) = t.max_tasks_per_set {
            built = built.with_max_tasks_per_set(s);
        }
        if let Some(l) = t.max_task_lists_per_get {
            built = built.with_max_task_lists_per_get(l);
        }
        let _ = built.max_tasks_per_get;
        if let Ok(enc) = serde_json::to_vec(&t) {
            let _ = serde_json::from_slice::<jmap_proto::tasks::TasksCapability>(&enc);
        }
    }

    if let Ok(s) = serde_json::from_slice::<jmap_proto::sieve::SieveCapability>(data) {
        let mut built = jmap_proto::sieve::SieveCapability::new(s.max_size_script)
            .with_sieve_extensions(s.sieve_extensions.clone())
            .with_extra(s.extra.clone());
        if let Some(m) = s.max_size_script_name {
            built = built.with_max_size_script_name(m);
        }
        if let Some(n) = s.max_number_scripts {
            built = built.with_max_number_scripts(n);
        }
        if let Some(ref imp) = s.implementation {
            built = built.with_implementation(imp);
        }
        let _ = built.max_size_script;
        if let Ok(enc) = serde_json::to_vec(&s) {
            let _ = serde_json::from_slice::<jmap_proto::sieve::SieveCapability>(&enc);
        }
    }

    if let Ok(sm) = serde_json::from_slice::<jmap_proto::mail::SmimeVerifyCapability>(data) {
        if let Ok(enc) = serde_json::to_vec(&sm) {
            let _ = serde_json::from_slice::<jmap_proto::mail::SmimeVerifyCapability>(&enc);
        }
    }

    if let Ok(fn_cap) = serde_json::from_slice::<jmap_proto::filenode::FileNodeCapability>(data) {
        let mut built =
            jmap_proto::filenode::FileNodeCapability::new().with_extra(fn_cap.extra.clone());
        if let Some(d) = fn_cap.max_file_node_depth {
            built = built.with_max_file_node_depth(d);
        }
        if let Some(s) = fn_cap.max_size_file_node_name {
            built = built.with_max_size_file_node_name(s);
        }
        let _ = built.max_file_node_depth;
        if let Ok(enc) = serde_json::to_vec(&fn_cap) {
            let _ = serde_json::from_slice::<jmap_proto::filenode::FileNodeCapability>(&enc);
        }
    }

    if let Ok(meta) = serde_json::from_slice::<jmap_proto::metadata::MetadataCapability>(data) {
        let built = jmap_proto::metadata::MetadataCapability::new().with_extra(meta.extra.clone());
        let _ = built.data_types.len();
        if let Ok(enc) = serde_json::to_vec(&meta) {
            let _ = serde_json::from_slice::<jmap_proto::metadata::MetadataCapability>(&enc);
        }
    }

    if let Ok(ms) = serde_json::from_slice::<jmap_proto::mail::MailShareCapability>(data) {
        if let Ok(enc) = serde_json::to_vec(&ms) {
            let _ = serde_json::from_slice::<jmap_proto::mail::MailShareCapability>(&enc);
        }
    }

    if let Ok(vapid) = serde_json::from_slice::<jmap_proto::push::WebPushVapidCapability>(data) {
        let built = jmap_proto::push::WebPushVapidCapability::new(&vapid.application_server_key)
            .with_application_server_key(&vapid.application_server_key)
            .with_extra(vapid.extra.clone());
        let _ = built.application_server_key;
        if let Ok(enc) = serde_json::to_vec(&vapid) {
            let _ = serde_json::from_slice::<jmap_proto::push::WebPushVapidCapability>(&enc);
        }
    }
});
