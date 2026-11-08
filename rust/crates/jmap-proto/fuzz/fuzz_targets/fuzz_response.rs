// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]
#![allow(clippy::collapsible_if)]

use libfuzzer_sys::fuzz_target;
use serde_json::Value;

use jmap_proto::error::{MethodError, RequestError, SetError};
use jmap_proto::methods::{
    BlobCopyRequest, BlobCopyResponse, ChangesRequest, ChangesResponse, Comparator, CopyRequest,
    CopyResponse, Echo, Filter, FilterOperator, GetRequest, GetResponse, QueryChangesRequest,
    QueryChangesResponse, QueryRequest, QueryResponse, SetRequest, SetResponse, UploadResponse,
};
use jmap_proto::request::{Invocation, Request, ResultReference};
use jmap_proto::response::Response;

fuzz_target!(|data: &[u8]| {
    if let Ok(response) = serde_json::from_slice::<Response>(data) {
        let _ = response.responses_for("");
        let _ = response.responses_for("0");
        let _ = response.responses_for("c1");
        let _ = response.responses_for("c2");
        let _ = response.responses_for("c3");

        let mut rebuilt = Response::new(response.session_state.clone())
            .with_method_responses(response.method_responses.clone());
        if let Some(created_ids) = &response.created_ids {
            rebuilt = rebuilt.with_created_ids(created_ids.clone());
        }

        for invocation in &response.method_responses {
            rebuilt = rebuilt.with_method_response(invocation.clone());
            let _ = invocation.is_error();
            if invocation.is_error() {
                let _ = invocation.parse::<MethodError>();
            }
            let _ = invocation.parse::<Value>();
            let _ = invocation.parse::<GetResponse<Value>>();
            let _ = invocation.parse::<SetResponse<Value>>();
            let _ = invocation.parse::<QueryResponse>();
            let _ = invocation.parse::<ChangesResponse>();
            let _ = invocation.parse::<QueryChangesResponse>();
            let _ = invocation.parse::<CopyResponse<Value>>();
            let _ = invocation.parse::<BlobCopyResponse>();
            let _ = invocation.parse::<Echo<Value>>();
        }

        if let Ok(encoded) = serde_json::to_vec(&response) {
            let _ = serde_json::from_slice::<Response>(&encoded);
        }
        let _ = serde_json::to_vec(&rebuilt);
    }

    if let Ok(invocation) = serde_json::from_slice::<Invocation>(data) {
        let _ = invocation.is_error();
        if invocation.is_error() {
            let _ = invocation.parse::<MethodError>();
        }
        let _ = invocation.parse::<Value>();
        let _ = invocation.parse::<GetResponse<Value>>();
        let _ = invocation.parse::<SetResponse<Value>>();
        let _ = invocation.parse::<QueryResponse>();
        let _ = invocation.parse::<CopyResponse<Value>>();
        let _ = invocation.parse::<BlobCopyResponse>();
        let _ = invocation.parse::<Echo<Value>>();

        let from_val = Invocation::from_value(
            invocation.name.clone(),
            invocation.arguments.clone(),
            invocation.call_id.clone(),
        );
        let _ = serde_json::to_vec(&from_val);
        if let Ok(built) = Invocation::new(
            invocation.name.clone(),
            &invocation.arguments,
            invocation.call_id.clone(),
        ) {
            let _ = serde_json::to_vec(&built);
        }

        if let Ok(encoded) = serde_json::to_vec(&invocation) {
            let _ = serde_json::from_slice::<Invocation>(&encoded);
        }
    }

    if let Ok(request) = serde_json::from_slice::<Request>(data) {
        let mut rebuilt = Request::new(request.using.clone());
        if let Some(created_ids) = &request.created_ids {
            rebuilt = rebuilt.with_created_ids(created_ids.clone());
        }
        for invocation in &request.method_calls {
            let _ = invocation.is_error();
            let _ = invocation.parse::<Value>();
            let _ = invocation.parse::<GetRequest>();
            let _ = invocation.parse::<SetRequest<Value>>();
            let _ = invocation.parse::<QueryRequest<Value>>();
            let _ = invocation.parse::<ChangesRequest>();
            let _ = invocation.parse::<QueryChangesRequest<Value>>();
            let _ = invocation.parse::<CopyRequest<Value>>();
            let _ = invocation.parse::<BlobCopyRequest>();
            let _ = invocation.parse::<Echo<Value>>();
            if let Ok(next) = rebuilt.clone().call(
                invocation.name.clone(),
                &invocation.arguments,
                invocation.call_id.clone(),
            ) {
                rebuilt = next;
            }
        }

        if let Ok(encoded) = serde_json::to_vec(&request) {
            let _ = serde_json::from_slice::<Request>(&encoded);
        }
        let _ = serde_json::to_vec(&rebuilt);
    }

    if let Ok(result_ref) = serde_json::from_slice::<ResultReference>(data) {
        let built = ResultReference::new(
            result_ref.result_of.clone(),
            result_ref.name.clone(),
            result_ref.path.clone(),
        );
        let _ = serde_json::to_vec(&built);
        if let Ok(encoded) = serde_json::to_vec(&result_ref) {
            let _ = serde_json::from_slice::<ResultReference>(&encoded);
        }
    }

    if let Ok(req_err) = serde_json::from_slice::<RequestError>(data) {
        let mut built =
            RequestError::new(req_err.error_type.clone()).with_extra(req_err.extra.clone());
        if let Some(status) = req_err.status {
            built = built.with_status(status);
        }
        if let Some(detail) = &req_err.detail {
            built = built.with_detail(detail.clone());
        }
        let _ = serde_json::to_vec(&built);
        if let Ok(encoded) = serde_json::to_vec(&req_err) {
            let _ = serde_json::from_slice::<RequestError>(&encoded);
        }
    }

    if let Ok(meth_err) = serde_json::from_slice::<MethodError>(data) {
        let mut built =
            MethodError::new(meth_err.error_type.clone()).with_extra(meth_err.extra.clone());
        if let Some(desc) = &meth_err.description {
            built = built.with_description(desc.clone());
        }
        let _ = serde_json::to_vec(&built);
        if let Ok(encoded) = serde_json::to_vec(&meth_err) {
            let _ = serde_json::from_slice::<MethodError>(&encoded);
        }
    }

    if let Ok(set_err) = serde_json::from_slice::<SetError>(data) {
        let mut built = SetError::new(set_err.error_type.clone()).with_extra(set_err.extra.clone());
        if let Some(desc) = &set_err.description {
            built = built.with_description(desc.clone());
        }
        if let Some(props) = &set_err.properties {
            built = built.with_properties(props.clone());
        }
        let _ = serde_json::to_vec(&built);
        if let Ok(encoded) = serde_json::to_vec(&set_err) {
            let _ = serde_json::from_slice::<SetError>(&encoded);
        }
    }

    if let Ok(get_req) = serde_json::from_slice::<GetRequest>(data) {
        let mut built = match &get_req.ids {
            Some(ids) => GetRequest::ids(get_req.account_id.clone(), ids.clone()),
            None => GetRequest::all(get_req.account_id.clone()),
        };
        if let Some(props) = &get_req.properties {
            built = built.properties(props.clone());
        }
        if let Some(ids_ref) = &get_req.ids_ref {
            built = built.ids_ref(ids_ref.clone());
        }
        let _ = serde_json::to_vec(&built);
        if let Ok(encoded) = serde_json::to_vec(&get_req) {
            let _ = serde_json::from_slice::<GetRequest>(&encoded);
        }
    }

    if let Ok(get_resp) = serde_json::from_slice::<GetResponse<Value>>(data) {
        let built = GetResponse::new(
            get_resp.account_id.clone(),
            get_resp.state.clone(),
            get_resp.list.clone(),
        )
        .with_not_found(get_resp.not_found.clone());
        let _ = serde_json::to_vec(&built);
        if let Ok(encoded) = serde_json::to_vec(&get_resp) {
            let _ = serde_json::from_slice::<GetResponse<Value>>(&encoded);
        }
    }

    if let Ok(set_req) = serde_json::from_slice::<SetRequest<Value>>(data) {
        let mut built = SetRequest::new(set_req.account_id.clone());
        if let Some(state) = &set_req.if_in_state {
            built = built.if_in_state(state.clone());
        }
        if let Some(create) = &set_req.create {
            for (cid, obj) in create {
                built = built.create(cid.clone(), obj.clone());
            }
            built = built.with_create(create.clone());
        }
        if let Some(update) = &set_req.update {
            for (id, patch) in update {
                built = built.update(id.clone(), patch.clone());
            }
            built = built.with_update(update.clone());
        }
        if let Some(destroy) = &set_req.destroy {
            for id in destroy {
                built = built.destroy(id.clone());
            }
            built = built.with_destroy(destroy.clone());
        }
        let _ = serde_json::to_vec(&built);
        if let Ok(encoded) = serde_json::to_vec(&set_req) {
            let _ = serde_json::from_slice::<SetRequest<Value>>(&encoded);
        }
    }

    if let Ok(set_resp) = serde_json::from_slice::<SetResponse<Value>>(data) {
        let new_state = set_resp
            .new_state
            .clone()
            .unwrap_or_else(|| jmap_proto::state::State::from("s0"));
        let mut built = SetResponse::new(set_resp.account_id.clone(), new_state);
        if let Some(old) = &set_resp.old_state {
            built = built.with_old_state(old.clone());
        }
        if let Some(created) = &set_resp.created {
            built = built.with_created(created.clone());
        }
        if let Some(updated) = &set_resp.updated {
            built = built.with_updated(updated.clone());
        }
        if let Some(destroyed) = &set_resp.destroyed {
            built = built.with_destroyed(destroyed.clone());
        }
        if let Some(nc) = &set_resp.not_created {
            built = built.with_not_created(nc.clone());
        }
        if let Some(nu) = &set_resp.not_updated {
            built = built.with_not_updated(nu.clone());
        }
        if let Some(nd) = &set_resp.not_destroyed {
            built = built.with_not_destroyed(nd.clone());
        }
        let _ = serde_json::to_vec(&built);
        if let Ok(encoded) = serde_json::to_vec(&set_resp) {
            let _ = serde_json::from_slice::<SetResponse<Value>>(&encoded);
        }
    }

    if let Ok(q_req) = serde_json::from_slice::<QueryRequest<Value>>(data) {
        let mut built = QueryRequest::new(q_req.account_id.clone()).position(q_req.position);
        if let Some(filter) = &q_req.filter {
            built = built.filter(filter.clone());
        }
        if let Some(sort) = &q_req.sort {
            built = built.sort(sort.clone());
        }
        if let Some(anchor) = &q_req.anchor {
            built = built.anchor(anchor.clone());
        }
        if let Some(offset) = q_req.anchor_offset {
            built = built.anchor_offset(offset);
        }
        if let Some(limit) = q_req.limit {
            built = built.limit(limit);
        }
        if q_req.calculate_total {
            built = built.calculate_total();
        }
        if q_req.collapse_threads {
            built = built.collapse_threads();
        }
        let _ = serde_json::to_vec(&built);
        if let Ok(encoded) = serde_json::to_vec(&q_req) {
            let _ = serde_json::from_slice::<QueryRequest<Value>>(&encoded);
        }
    }

    if let Ok(q_req_filter) = serde_json::from_slice::<QueryRequest<Filter<Value>>>(data) {
        let mut built =
            QueryRequest::new(q_req_filter.account_id.clone()).position(q_req_filter.position);
        if let Some(filter) = &q_req_filter.filter {
            built = built.filter(filter.clone());
        }
        if let Some(sort) = &q_req_filter.sort {
            built = built.sort(sort.clone());
        }
        if let Ok(encoded) = serde_json::to_vec(&built) {
            let _ = serde_json::from_slice::<QueryRequest<Filter<Value>>>(&encoded);
        }
    }

    if let Ok(comp) = serde_json::from_slice::<Comparator>(data) {
        let mut built = if comp.is_ascending {
            Comparator::ascending(comp.property.clone())
        } else {
            Comparator::descending(comp.property.clone())
        };
        if let Some(coll) = &comp.collation {
            built = built.with_collation(coll.clone());
        }
        let _ = serde_json::to_vec(&built);
        if let Ok(encoded) = serde_json::to_vec(&comp) {
            let _ = serde_json::from_slice::<Comparator>(&encoded);
        }
    }

    if let Ok(filter) = serde_json::from_slice::<Filter<Value>>(data) {
        let _ = Filter::and([filter.clone()]);
        let _ = Filter::or([filter.clone()]);
        let _ = Filter::not([filter.clone()]);
        if let Ok(encoded) = serde_json::to_vec(&filter) {
            let _ = serde_json::from_slice::<Filter<Value>>(&encoded);
        }
    }

    if let Ok(fop) = serde_json::from_slice::<FilterOperator<Value>>(data) {
        if let Ok(encoded) = serde_json::to_vec(&fop) {
            let _ = serde_json::from_slice::<FilterOperator<Value>>(&encoded);
        }
    }

    if let Ok(q_resp) = serde_json::from_slice::<QueryResponse>(data) {
        let mut built = QueryResponse::new(
            q_resp.account_id.clone(),
            q_resp.query_state.clone(),
            q_resp.ids.clone(),
        )
        .with_position(q_resp.position)
        .can_calculate_changes(q_resp.can_calculate_changes);
        if let Some(total) = q_resp.total {
            built = built.with_total(total);
        }
        if let Some(limit) = q_resp.limit {
            built = built.with_limit(limit);
        }
        let _ = serde_json::to_vec(&built);
        if let Ok(encoded) = serde_json::to_vec(&q_resp) {
            let _ = serde_json::from_slice::<QueryResponse>(&encoded);
        }
    }

    if let Ok(up_resp) = serde_json::from_slice::<UploadResponse>(data) {
        let mut built = UploadResponse::new(
            up_resp.account_id.clone(),
            up_resp.blob_id.clone(),
            up_resp.size,
        );
        if let Some(ct) = &up_resp.content_type {
            built = built.with_content_type(ct.clone());
        }
        let _ = serde_json::to_vec(&built);
        if let Ok(encoded) = serde_json::to_vec(&up_resp) {
            let _ = serde_json::from_slice::<UploadResponse>(&encoded);
        }
    }

    if let Ok(echo) = serde_json::from_slice::<Echo<Value>>(data) {
        let built = Echo::new(echo.payload.clone());
        let _ = serde_json::to_vec(&built);
        if let Ok(encoded) = serde_json::to_vec(&echo) {
            let _ = serde_json::from_slice::<Echo<Value>>(&encoded);
        }
    }

    if let Ok(copy_req) = serde_json::from_slice::<CopyRequest<Value>>(data) {
        let mut built = CopyRequest::new(
            copy_req.from_account_id.clone(),
            copy_req.account_id.clone(),
        );
        if let Some(s) = &copy_req.if_from_in_state {
            built = built.if_from_in_state(s.clone());
        }
        if let Some(s) = &copy_req.if_in_state {
            built = built.if_in_state(s.clone());
        }
        for (cid, obj) in &copy_req.create {
            built = built.copy_object(cid.clone(), obj.clone());
        }
        if copy_req.on_success_destroy_original {
            built = built.on_success_destroy_original();
        }
        if let Some(s) = &copy_req.destroy_from_if_in_state {
            built = built.destroy_from_if_in_state(s.clone());
        }
        let _ = serde_json::to_vec(&built);
        if let Ok(encoded) = serde_json::to_vec(&copy_req) {
            let _ = serde_json::from_slice::<CopyRequest<Value>>(&encoded);
        }
    }

    if let Ok(copy_resp) = serde_json::from_slice::<CopyResponse<Value>>(data) {
        let mut built = CopyResponse::new(
            copy_resp.from_account_id.clone(),
            copy_resp.account_id.clone(),
            copy_resp.new_state.clone(),
        );
        if let Some(old) = &copy_resp.old_state {
            built = built.with_old_state(old.clone());
        }
        if let Some(created) = &copy_resp.created {
            built = built.with_created(created.clone());
        }
        if let Some(nc) = &copy_resp.not_created {
            built = built.with_not_created(nc.clone());
        }
        let _ = serde_json::to_vec(&built);
        if let Ok(encoded) = serde_json::to_vec(&copy_resp) {
            let _ = serde_json::from_slice::<CopyResponse<Value>>(&encoded);
        }
    }

    if let Ok(bcopy_req) = serde_json::from_slice::<BlobCopyRequest>(data) {
        let built = BlobCopyRequest::new(
            bcopy_req.from_account_id.clone(),
            bcopy_req.account_id.clone(),
            bcopy_req.blob_ids.clone(),
        );
        let _ = serde_json::to_vec(&built);
        if let Ok(encoded) = serde_json::to_vec(&bcopy_req) {
            let _ = serde_json::from_slice::<BlobCopyRequest>(&encoded);
        }
    }

    if let Ok(bcopy_resp) = serde_json::from_slice::<BlobCopyResponse>(data) {
        let mut built = BlobCopyResponse::new(
            bcopy_resp.from_account_id.clone(),
            bcopy_resp.account_id.clone(),
        );
        if let Some(copied) = &bcopy_resp.copied {
            built = built.with_copied(copied.clone());
        }
        if let Some(nc) = &bcopy_resp.not_copied {
            built = built.with_not_copied(nc.clone());
        }
        let _ = serde_json::to_vec(&built);
        if let Ok(encoded) = serde_json::to_vec(&bcopy_resp) {
            let _ = serde_json::from_slice::<BlobCopyResponse>(&encoded);
        }
    }
});
