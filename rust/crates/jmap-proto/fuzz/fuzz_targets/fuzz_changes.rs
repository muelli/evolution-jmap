// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use jmap_proto::methods::{Comparator, Filter, QueryChangesRequest};
use libfuzzer_sys::fuzz_target;

macro_rules! roundtrip_query_changes_request {
    ($data:expr, $filter_ty:ty) => {
        if let Ok(qreq) = serde_json::from_slice::<QueryChangesRequest<Filter<$filter_ty>>>($data) {
            let mut built: QueryChangesRequest<Filter<$filter_ty>> =
                QueryChangesRequest::new(qreq.account_id.clone(), qreq.since_query_state.clone());
            if let Some(ref filter) = qreq.filter {
                let wrapped = match filter {
                    Filter::Condition(c) => Filter::and([
                        Filter::condition(c.clone()),
                        Filter::or([
                            Filter::from(c.clone()),
                            Filter::not([Filter::from(c.clone())]),
                        ]),
                    ]),
                    Filter::Operator(op) => Filter::Operator(op.clone()),
                };
                built = built.filter(wrapped);
            }
            if let Some(ref sort) = qreq.sort {
                let rebuilt_sort = sort.iter().map(|c| {
                    let comp = if c.is_ascending {
                        Comparator::ascending(c.property.clone())
                    } else {
                        Comparator::descending(c.property.clone())
                    };
                    if let Some(ref coll) = c.collation {
                        comp.with_collation(coll.clone())
                    } else {
                        comp
                    }
                });
                built = built.sort(rebuilt_sort);
            }
            if let Some(max) = qreq.max_changes {
                built = built.max_changes(max);
            }
            if let Some(ref up_to) = qreq.up_to_id {
                built = built.up_to_id(up_to.clone());
            }
            if qreq.calculate_total {
                built = built.calculate_total();
            }
            if qreq.collapse_threads {
                built = built.collapse_threads();
            }

            if let Ok(encoded) = serde_json::to_vec(&qreq) {
                let _ = serde_json::from_slice::<QueryChangesRequest<Filter<$filter_ty>>>(&encoded);
            }
            if let Ok(encoded) = serde_json::to_vec(&built) {
                let _ = serde_json::from_slice::<QueryChangesRequest<Filter<$filter_ty>>>(&encoded);
            }
        }
    };
}

fuzz_target!(|data: &[u8]| {
    if let Ok(changes) = serde_json::from_slice::<jmap_proto::methods::ChangesResponse>(data) {
        let built = jmap_proto::methods::ChangesResponse::new(
            changes.account_id.clone(),
            changes.old_state.clone(),
            changes.new_state.clone(),
        )
        .with_created(changes.created.clone())
        .with_updated(changes.updated.clone())
        .with_destroyed(changes.destroyed.clone())
        .has_more_changes(changes.has_more_changes);

        if let Ok(encoded) = serde_json::to_vec(&changes) {
            let _ = serde_json::from_slice::<jmap_proto::methods::ChangesResponse>(&encoded);
        }
        if let Ok(encoded) = serde_json::to_vec(&built) {
            let _ = serde_json::from_slice::<jmap_proto::methods::ChangesResponse>(&encoded);
        }
    }

    if let Ok(query_changes) =
        serde_json::from_slice::<jmap_proto::methods::QueryChangesResponse>(data)
    {
        let mut qc_built = jmap_proto::methods::QueryChangesResponse::new(
            query_changes.account_id.clone(),
            query_changes.old_query_state.clone(),
            query_changes.new_query_state.clone(),
        )
        .with_added(query_changes.added.clone())
        .with_removed(query_changes.removed.clone());

        if let Some(total) = query_changes.total {
            qc_built = qc_built.with_total(total);
        }

        if let Ok(encoded) = serde_json::to_vec(&query_changes) {
            let _ = serde_json::from_slice::<jmap_proto::methods::QueryChangesResponse>(&encoded);
        }
        if let Ok(encoded) = serde_json::to_vec(&qc_built) {
            let _ = serde_json::from_slice::<jmap_proto::methods::QueryChangesResponse>(&encoded);
        }
    }

    if let Ok(req) = serde_json::from_slice::<jmap_proto::methods::ChangesRequest>(data) {
        let mut cr_built = jmap_proto::methods::ChangesRequest::new(
            req.account_id.clone(),
            req.since_state.clone(),
        );
        if let Some(max) = req.max_changes {
            cr_built = cr_built.max_changes(max);
        }

        if let Ok(encoded) = serde_json::to_vec(&req) {
            let _ = serde_json::from_slice::<jmap_proto::methods::ChangesRequest>(&encoded);
        }
        if let Ok(encoded) = serde_json::to_vec(&cr_built) {
            let _ = serde_json::from_slice::<jmap_proto::methods::ChangesRequest>(&encoded);
        }
    }

    if let Ok(qreq) =
        serde_json::from_slice::<jmap_proto::methods::QueryChangesRequest<serde_json::Value>>(data)
    {
        let mut qcr_built: jmap_proto::methods::QueryChangesRequest<serde_json::Value> =
            jmap_proto::methods::QueryChangesRequest::new(
                qreq.account_id.clone(),
                qreq.since_query_state.clone(),
            );
        if let Some(ref filter) = qreq.filter {
            qcr_built = qcr_built.filter(filter.clone());
        }
        if let Some(ref sort) = qreq.sort {
            qcr_built = qcr_built.sort(sort.clone());
        }
        if let Some(max) = qreq.max_changes {
            qcr_built = qcr_built.max_changes(max);
        }
        if let Some(ref up_to) = qreq.up_to_id {
            qcr_built = qcr_built.up_to_id(up_to.clone());
        }
        if qreq.calculate_total {
            qcr_built = qcr_built.calculate_total();
        }
        if qreq.collapse_threads {
            qcr_built = qcr_built.collapse_threads();
        }

        if let Ok(encoded) = serde_json::to_vec(&qreq) {
            let _ = serde_json::from_slice::<
                jmap_proto::methods::QueryChangesRequest<serde_json::Value>,
            >(&encoded);
        }
        if let Ok(encoded) = serde_json::to_vec(&qcr_built) {
            let _ = serde_json::from_slice::<
                jmap_proto::methods::QueryChangesRequest<serde_json::Value>,
            >(&encoded);
        }
    }

    roundtrip_query_changes_request!(data, jmap_proto::mail::EmailQueryFilter);
    roundtrip_query_changes_request!(data, jmap_proto::mail::EmailSubmissionQueryFilter);
    roundtrip_query_changes_request!(data, jmap_proto::contacts::ContactCardQueryFilter);
    roundtrip_query_changes_request!(data, jmap_proto::calendars::CalendarEventQueryFilter);
    roundtrip_query_changes_request!(
        data,
        jmap_proto::calendars::CalendarEventNotificationQueryFilter
    );
    roundtrip_query_changes_request!(data, jmap_proto::tasks::TaskQueryFilter);
    roundtrip_query_changes_request!(data, jmap_proto::principals::PrincipalQueryFilter);
    roundtrip_query_changes_request!(data, jmap_proto::principals::ShareNotificationQueryFilter);
    roundtrip_query_changes_request!(data, jmap_proto::sieve::SieveScriptQueryFilter);
    roundtrip_query_changes_request!(data, jmap_proto::quota::QuotaQueryFilter);
    roundtrip_query_changes_request!(data, jmap_proto::filenode::FileNodeQueryFilter);
    roundtrip_query_changes_request!(data, jmap_proto::metadata::MetadataFilterCondition);

    if let Ok(ef) = serde_json::from_slice::<jmap_proto::mail::EmailQueryFilter>(data) {
        let mut built = jmap_proto::mail::EmailQueryFilter::default();
        if let Some(ref mb) = ef.in_mailbox {
            built = jmap_proto::mail::EmailQueryFilter::in_mailbox(mb.clone());
        }
        if let Some(ref mbs) = ef.in_mailbox_other_than {
            built = built.in_mailbox_other_than(mbs.clone());
        }
        if let Some(min) = ef.min_size {
            built = built.min_size(min);
        }
        if let Some(max) = ef.max_size {
            built = built.max_size(max);
        }
        if let Some(ref kw) = ef.has_keyword {
            built = built.has_keyword(kw.clone());
        }
        if let Some(ref kw) = ef.not_keyword {
            built = built.not_keyword(kw.clone());
        }
        if let Some(ref kw) = ef.all_in_thread_have_keyword {
            built = built.all_in_thread_have_keyword(kw.clone());
        }
        if let Some(ref kw) = ef.some_in_thread_have_keyword {
            built = built.some_in_thread_have_keyword(kw.clone());
        }
        if let Some(ref kw) = ef.none_in_thread_have_keyword {
            built = built.none_in_thread_have_keyword(kw.clone());
        }
        if let Some(att) = ef.has_attachment {
            built = built.has_attachment(att);
        }
        if let Some(ref t) = ef.text {
            built = built.text(t.clone());
        }
        if let Some(ref b) = ef.body {
            built = built.body(b.clone());
        }
        if let Some(ref f) = ef.from {
            built = built.from(f.clone());
        }
        if let Some(ref t) = ef.to {
            built = built.to(t.clone());
        }
        if let Some(ref c) = ef.cc {
            built = built.cc(c.clone());
        }
        if let Some(ref b) = ef.bcc {
            built = built.bcc(b.clone());
        }
        if let Some(ref s) = ef.subject {
            built = built.subject(s.clone());
        }
        if let Some(ref h) = ef.header
            && h.len() == 2
        {
            built = built.header(h[0].clone(), h[1].clone());
        }
        built = built.time_range(ef.after.clone(), ef.before.clone());
        if let Some(ref b) = ef.before {
            built = built.before(b.clone());
        }
        if let Some(ref a) = ef.after {
            built = built.after(a.clone());
        }
        if let Some(sm) = ef.has_smime {
            built = built.with_has_smime(sm);
        }
        if let Some(vsm) = ef.has_verified_smime {
            built = built.with_has_verified_smime(vsm);
        }
        if let Ok(encoded) = serde_json::to_vec(&built) {
            let _ = serde_json::from_slice::<jmap_proto::mail::EmailQueryFilter>(&encoded);
        }
    }

    if let Ok(esf) = serde_json::from_slice::<jmap_proto::mail::EmailSubmissionQueryFilter>(data) {
        let mut built = jmap_proto::mail::EmailSubmissionQueryFilter::new();
        if let Some(ref ids) = esf.identity_ids {
            built = built.with_identity_ids(ids.clone());
        }
        if let Some(ref ids) = esf.email_ids {
            built = built.with_email_ids(ids.clone());
        }
        if let Some(ref ids) = esf.thread_ids {
            built = built.with_thread_ids(ids.clone());
        }
        if let Some(ref u) = esf.undo_status {
            built = built.with_undo_status(u.clone());
        }
        built = built.time_range(esf.after.clone(), esf.before.clone());
        if let Some(ref b) = esf.before {
            built = built.before(b.clone());
        }
        if let Some(ref a) = esf.after {
            built = built.after(a.clone());
        }
        if let Ok(encoded) = serde_json::to_vec(&built) {
            let _ =
                serde_json::from_slice::<jmap_proto::mail::EmailSubmissionQueryFilter>(&encoded);
        }
    }

    if let Ok(cf) = serde_json::from_slice::<jmap_proto::contacts::ContactCardQueryFilter>(data) {
        let mut built = if let Some(ref ab) = cf.in_address_book {
            jmap_proto::contacts::ContactCardQueryFilter::in_address_book(ab.clone())
        } else {
            jmap_proto::contacts::ContactCardQueryFilter::default()
        };
        if let Some(ref u) = cf.uid {
            built = built.uid(u.clone());
        }
        if let Some(ref m) = cf.has_member {
            built = built.has_member(m.clone());
        }
        if let Some(ref cb) = cf.created_before {
            built = built.created_before(cb.clone());
        }
        if let Some(ref ca) = cf.created_after {
            built = built.created_after(ca.clone());
        }
        if let Some(ref ub) = cf.updated_before {
            built = built.updated_before(ub.clone());
        }
        if let Some(ref ua) = cf.updated_after {
            built = built.updated_after(ua.clone());
        }
        if let Some(ref t) = cf.text {
            built = built.text(t.clone());
        }
        if let Some(ref n) = cf.name {
            built = built.name(n.clone());
        }
        if let Some(ref ng) = cf.name_given {
            built = built.name_given(ng.clone());
        }
        if let Some(ref ns) = cf.name_surname {
            built = built.name_surname(ns.clone());
        }
        if let Some(ref ns2) = cf.name_surname2 {
            built = built.name_surname2(ns2.clone());
        }
        if let Some(ref nick) = cf.nickname {
            built = built.nickname(nick.clone());
        }
        if let Some(ref org) = cf.organization {
            built = built.organization(org.clone());
        }
        if let Some(ref em) = cf.email {
            built = built.email(em.clone());
        }
        if let Some(ref ph) = cf.phone {
            built = built.phone(ph.clone());
        }
        if let Some(ref os) = cf.online_service {
            built = built.online_service(os.clone());
        }
        if let Some(ref addr) = cf.address {
            built = built.address(addr.clone());
        }
        if let Some(ref note) = cf.note {
            built = built.note(note.clone());
        }
        if let Some(ref k) = cf.kind {
            built = built.kind(k.clone());
        }
        if let Ok(encoded) = serde_json::to_vec(&built) {
            let _ =
                serde_json::from_slice::<jmap_proto::contacts::ContactCardQueryFilter>(&encoded);
        }
    }

    if let Ok(cef) = serde_json::from_slice::<jmap_proto::calendars::CalendarEventQueryFilter>(data)
    {
        let mut built = if let Some(ref cal) = cef.in_calendar {
            jmap_proto::calendars::CalendarEventQueryFilter::in_calendar(cal.clone())
        } else if let Some(ref cals) = cef.in_calendar_list {
            jmap_proto::calendars::CalendarEventQueryFilter::in_calendar_list(cals.clone())
        } else {
            jmap_proto::calendars::CalendarEventQueryFilter::default()
        };
        if let Some(ref cals) = cef.in_calendar_list {
            built = built.with_in_calendar_list(cals.clone());
        }
        if let (Some(a), Some(b)) = (&cef.after, &cef.before) {
            let _ =
                jmap_proto::calendars::CalendarEventQueryFilter::time_range(a.clone(), b.clone());
            built = built.with_time_range(a.clone(), b.clone());
        }
        if let Some(ref a) = cef.after {
            built = built.after(a.clone());
        }
        if let Some(ref b) = cef.before {
            built = built.before(b.clone());
        }
        if let Some(ref t) = cef.text {
            built = built.text(t.clone());
        }
        if let Some(ref ti) = cef.title {
            built = built.title(ti.clone());
        }
        if let Some(ref d) = cef.description {
            built = built.description(d.clone());
        }
        if let Some(ref l) = cef.location {
            built = built.location(l.clone());
        }
        if let Some(ref u) = cef.uid {
            built = built.uid(u.clone());
        }
        if let Some(ref ex) = cef.excluded_from_calendar {
            built = built
                .excluded_from_calendar(ex.clone())
                .with_excluded_from_calendar(ex.clone());
        }
        if let Some(ref tz) = cef.time_zone {
            built = built.time_zone(tz.clone()).with_time_zone(tz.clone());
        }
        if let Some(ref ps) = cef.participation_status {
            built = built.participation_status(ps.clone());
        }
        if let Ok(encoded) = serde_json::to_vec(&built) {
            let _ =
                serde_json::from_slice::<jmap_proto::calendars::CalendarEventQueryFilter>(&encoded);
        }
    }

    if let Ok(cenf) =
        serde_json::from_slice::<jmap_proto::calendars::CalendarEventNotificationQueryFilter>(data)
    {
        let mut built = jmap_proto::calendars::CalendarEventNotificationQueryFilter::new();
        if let Some(ref a) = cenf.after {
            built = built.with_after(a.clone());
        }
        if let Some(ref b) = cenf.before {
            built = built.with_before(b.clone());
        }
        if let Some(ref t) = cenf.types {
            built = built.with_types(t.clone());
        }
        if let Ok(encoded) = serde_json::to_vec(&built) {
            let _ = serde_json::from_slice::<
                jmap_proto::calendars::CalendarEventNotificationQueryFilter,
            >(&encoded);
        }
    }

    if let Ok(tf) = serde_json::from_slice::<jmap_proto::tasks::TaskQueryFilter>(data) {
        let mut built = jmap_proto::tasks::TaskQueryFilter::new().with_extra(tf.extra.clone());
        if let Some(ref id) = tf.task_list_id {
            built = built.with_task_list_id(id.clone());
        }
        if let Some(ref t) = tf.text {
            built = built.with_text(t.clone());
        }
        if let Some(ref s) = tf.status {
            built = built.with_status(s.clone());
        }
        if let Some(ref db) = tf.due_before {
            built = built.with_due_before(db.clone());
        }
        if let Some(hr) = tf.has_recurrence {
            built = built.with_has_recurrence(hr);
        }
        if let Ok(encoded) = serde_json::to_vec(&built) {
            let _ = serde_json::from_slice::<jmap_proto::tasks::TaskQueryFilter>(&encoded);
        }
    }

    if let Ok(pf) = serde_json::from_slice::<jmap_proto::principals::PrincipalQueryFilter>(data) {
        let mut built = if let Some(ref em) = pf.email {
            jmap_proto::principals::PrincipalQueryFilter::email(em.clone())
        } else {
            jmap_proto::principals::PrincipalQueryFilter::default()
        }
        .with_extra(pf.extra.clone());
        if let Some(ref ids) = pf.account_ids {
            built = built.account_ids(ids.clone());
        }
        if let Some(ref n) = pf.name {
            built = built.name(n.clone());
        }
        if let Some(ref t) = pf.text {
            built = built.text(t.clone());
        }
        if let Some(ref pt) = pf.principal_type {
            built = built.principal_type(pt.clone());
        }
        if let Some(ref tz) = pf.time_zone {
            built = built.time_zone(tz.clone());
        }
        if let Ok(encoded) = serde_json::to_vec(&built) {
            let _ =
                serde_json::from_slice::<jmap_proto::principals::PrincipalQueryFilter>(&encoded);
        }
    }

    if let Ok(snf) =
        serde_json::from_slice::<jmap_proto::principals::ShareNotificationQueryFilter>(data)
    {
        let mut built = jmap_proto::principals::ShareNotificationQueryFilter::new()
            .with_extra(snf.extra.clone());
        if let Some(ref a) = snf.after {
            built = built.with_after(a.clone());
        }
        if let Some(ref b) = snf.before {
            built = built.with_before(b.clone());
        }
        if let Some(ref ot) = snf.object_type {
            built = built.with_object_type(ot.clone());
        }
        if let Some(ref oa) = snf.object_account_id {
            built = built.with_object_account_id(oa.clone());
        }
        if let Ok(encoded) = serde_json::to_vec(&built) {
            let _ = serde_json::from_slice::<jmap_proto::principals::ShareNotificationQueryFilter>(
                &encoded,
            );
        }
    }

    if let Ok(sf) = serde_json::from_slice::<jmap_proto::sieve::SieveScriptQueryFilter>(data) {
        let mut built =
            jmap_proto::sieve::SieveScriptQueryFilter::new().with_extra(sf.extra.clone());
        if let Some(ref n) = sf.name {
            built = built.with_name(n.clone());
        }
        if let Some(act) = sf.is_active {
            built = built.with_is_active(act);
        }
        if let Ok(encoded) = serde_json::to_vec(&built) {
            let _ = serde_json::from_slice::<jmap_proto::sieve::SieveScriptQueryFilter>(&encoded);
        }
    }

    if let Ok(qf) = serde_json::from_slice::<jmap_proto::quota::QuotaQueryFilter>(data) {
        let mut built = jmap_proto::quota::QuotaQueryFilter::new().with_extra(qf.extra.clone());
        if let Some(ref n) = qf.name {
            built = built.with_name(n.clone());
        }
        if let Some(ref rt) = qf.resource_type {
            built = built.with_resource_type(rt.clone());
        }
        if let Some(ref sc) = qf.scope {
            built = built.with_scope(sc.clone());
        }
        if let Some(ref t) = qf.r#type {
            built = built.with_type(t.clone());
        }
        if let Ok(encoded) = serde_json::to_vec(&built) {
            let _ = serde_json::from_slice::<jmap_proto::quota::QuotaQueryFilter>(&encoded);
        }
    }

    if let Ok(fnf) = serde_json::from_slice::<jmap_proto::filenode::FileNodeQueryFilter>(data) {
        let mut built =
            jmap_proto::filenode::FileNodeQueryFilter::new().with_extra(fnf.extra.clone());
        if let Some(ref p) = fnf.parent_id {
            built = built.with_parent_id(p.clone());
        }
        if let Some(ref d) = fnf.descendant_id {
            built = built.with_descendant_id(d.clone());
        }
        if let Some(hp) = fnf.has_parent_id {
            built = built.with_has_parent_id(hp);
        }
        if let Some(ref n) = fnf.name {
            built = built.with_name(n.clone());
        }
        if let Some(ref nt) = fnf.node_type {
            built = built.with_node_type(nt.clone());
        }
        if let Some(ref r) = fnf.role {
            built = built.with_role(r.clone());
        }
        if let Some(ex) = fnf.is_executable {
            built = built.with_is_executable(ex);
        }
        if let Some(hb) = fnf.has_blob {
            built = built.with_has_blob(hb);
        }
        if let Ok(encoded) = serde_json::to_vec(&built) {
            let _ = serde_json::from_slice::<jmap_proto::filenode::FileNodeQueryFilter>(&encoded);
        }
    }

    if let Ok(mf) = serde_json::from_slice::<jmap_proto::metadata::MetadataFilterCondition>(data) {
        let mut built =
            jmap_proto::metadata::MetadataFilterCondition::new().with_extra(mf.extra.clone());
        if let Some(ref me) = mf.metadata_exists {
            built = built.with_metadata_exists(me.clone());
        }
        if let Some(ref mtc) = mf.metadata_text_contains {
            let _ =
                jmap_proto::metadata::MetadataTextFilter::new(mtc.path.clone(), mtc.text.clone())
                    .with_extra(mtc.extra.clone());
            built = built.with_metadata_text_contains(mtc.path.clone(), mtc.text.clone());
        }
        if let Some(ref mte) = mf.metadata_text_equals {
            built = built.with_metadata_text_equals(mte.path.clone(), mte.text.clone());
        }
        if let Some(ref pme) = mf.private_metadata_exists {
            built = built.with_private_metadata_exists(pme.clone());
        }
        if let Some(ref pmtc) = mf.private_metadata_text_contains {
            built = built.with_private_metadata_text_contains(pmtc.path.clone(), pmtc.text.clone());
        }
        if let Some(ref pmte) = mf.private_metadata_text_equals {
            built = built.with_private_metadata_text_equals(pmte.path.clone(), pmte.text.clone());
        }
        if let Ok(encoded) = serde_json::to_vec(&built) {
            let _ =
                serde_json::from_slice::<jmap_proto::metadata::MetadataFilterCondition>(&encoded);
        }
    }

    if let Ok(sc) = serde_json::from_slice::<jmap_proto::push::StateChange>(data) {
        let mut built = jmap_proto::push::StateChange::new(std::collections::BTreeMap::new())
            .with_changed(sc.changed.clone())
            .with_extra(sc.extra.clone());
        for (acc, map) in &sc.changed {
            for (ty, st) in map {
                built = built.with_account_change(acc.clone(), ty.clone(), st.clone());
            }
        }
        if let Ok(encoded) = serde_json::to_vec(&sc) {
            let _ = serde_json::from_slice::<jmap_proto::push::StateChange>(&encoded);
        }
        if let Ok(encoded) = serde_json::to_vec(&built) {
            let _ = serde_json::from_slice::<jmap_proto::push::StateChange>(&encoded);
        }
    }

    if let Ok(ps) = serde_json::from_slice::<jmap_proto::push::PushSubscription>(data) {
        let mut built = jmap_proto::push::PushSubscription::new(
            ps.device_client_id.clone(),
            ps.url.clone().unwrap_or_default(),
        )
        .with_device_client_id(ps.device_client_id.clone())
        .with_extra(ps.extra.clone());
        if let Some(ref id) = ps.id {
            built = built.with_id(id.clone());
        }
        if let Some(ref url) = ps.url {
            built = built.with_url(url.clone());
        }
        if let Some(ref keys) = ps.keys {
            let _ =
                jmap_proto::push::PushSubscriptionKeys::new(keys.p256dh.clone(), keys.auth.clone())
                    .with_p256dh(keys.p256dh.clone())
                    .with_auth(keys.auth.clone());
            built = built.with_keys(keys.p256dh.clone(), keys.auth.clone());
        }
        if let Some(ref exp) = ps.expires {
            built = built.with_expires(exp.clone());
        }
        if let Some(ref t) = ps.types {
            built = built.with_types(t.clone());
        }
        if let Some(ref vc) = ps.verification_code {
            built = built.with_verification_code(vc.clone());
        }
        if let Ok(encoded) = serde_json::to_vec(&built) {
            let _ = serde_json::from_slice::<jmap_proto::push::PushSubscription>(&encoded);
        }
    }

    if let Ok(pv) = serde_json::from_slice::<jmap_proto::push::PushVerification>(data) {
        let built = jmap_proto::push::PushVerification::new(
            pv.push_subscription_id.clone(),
            pv.verification_code.clone(),
        )
        .with_push_subscription_id(pv.push_subscription_id.clone())
        .with_verification_code(pv.verification_code.clone())
        .with_extra(pv.extra.clone());
        if let Ok(encoded) = serde_json::to_vec(&built) {
            let _ = serde_json::from_slice::<jmap_proto::push::PushVerification>(&encoded);
        }
    }

    if let Ok(item) = serde_json::from_slice::<jmap_proto::methods::AddedItem>(data) {
        let item_built = jmap_proto::methods::AddedItem::new(item.id.clone(), item.index);
        if let Ok(encoded) = serde_json::to_vec(&item) {
            let _ = serde_json::from_slice::<jmap_proto::methods::AddedItem>(&encoded);
        }
        if let Ok(encoded) = serde_json::to_vec(&item_built) {
            let _ = serde_json::from_slice::<jmap_proto::methods::AddedItem>(&encoded);
        }
    }

    if let Ok(s) = std::str::from_utf8(data) {
        let id = jmap_proto::id::Id::new(s);
        let _ = id.as_str();
        let _ = id.is_empty();
        let _ = id.as_ref();
        let _ = id.to_string();
        let _ = jmap_proto::id::split_shared_identity(s);

        let st = jmap_proto::state::State::new(s);
        let _ = st.as_str();
        let _ = st.as_ref();
        let _ = st.to_string();

        let dt = jmap_proto::state::UtcDate::new(s);
        let _ = dt.as_str();
        let _ = dt.as_ref();
        let _ = dt.to_string();
    }

    if data.len() >= 8 {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&data[..8]);
        let val = i64::from_le_bytes(bytes);
        if let Some((year, month, day)) = jmap_proto::date::civil_from_days(val) {
            let _ = jmap_proto::date::is_leap_year(year);
            let _ = jmap_proto::date::days_in_month(year, month);
            if (-1_000_000..=1_000_000).contains(&year) {
                let _ = jmap_proto::date::days_from_civil(year, month, day);
            }
        }
    }
});
