// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use libfuzzer_sys::fuzz_target;

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

    if let Ok(item) = serde_json::from_slice::<jmap_proto::methods::AddedItem>(data) {
        let item_built = jmap_proto::methods::AddedItem::new(item.id.clone(), item.index);
        if let Ok(encoded) = serde_json::to_vec(&item) {
            let _ = serde_json::from_slice::<jmap_proto::methods::AddedItem>(&encoded);
        }
        if let Ok(encoded) = serde_json::to_vec(&item_built) {
            let _ = serde_json::from_slice::<jmap_proto::methods::AddedItem>(&encoded);
        }
    }
});
