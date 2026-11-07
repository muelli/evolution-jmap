// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(response) = serde_json::from_slice::<jmap_proto::response::Response>(data) {
        let _ = response.responses_for("");
        let _ = response.responses_for("0");
        let _ = response.responses_for("c1");
        let _ = response.responses_for("c2");
        let _ = response.responses_for("c3");

        for invocation in &response.method_responses {
            let _ = invocation.is_error();
            if invocation.is_error() {
                let _ = invocation.parse::<jmap_proto::error::MethodError>();
            }
            let _ = invocation.parse::<serde_json::Value>();
        }

        if let Ok(encoded) = serde_json::to_vec(&response) {
            let _ = serde_json::from_slice::<jmap_proto::response::Response>(&encoded);
        }
    }

    if let Ok(invocation) = serde_json::from_slice::<jmap_proto::request::Invocation>(data) {
        let _ = invocation.is_error();
        if invocation.is_error() {
            let _ = invocation.parse::<jmap_proto::error::MethodError>();
        }
        let _ = invocation.parse::<serde_json::Value>();

        if let Ok(encoded) = serde_json::to_vec(&invocation) {
            let _ = serde_json::from_slice::<jmap_proto::request::Invocation>(&encoded);
        }
    }

    if let Ok(request) = serde_json::from_slice::<jmap_proto::request::Request>(data) {
        for invocation in &request.method_calls {
            let _ = invocation.is_error();
            let _ = invocation.parse::<serde_json::Value>();
        }

        if let Ok(encoded) = serde_json::to_vec(&request) {
            let _ = serde_json::from_slice::<jmap_proto::request::Request>(&encoded);
        }
    }

    if let Ok(result_ref) = serde_json::from_slice::<jmap_proto::request::ResultReference>(data) {
        if let Ok(encoded) = serde_json::to_vec(&result_ref) {
            let _ = serde_json::from_slice::<jmap_proto::request::ResultReference>(&encoded);
        }
    }

    if let Ok(req_err) = serde_json::from_slice::<jmap_proto::error::RequestError>(data) {
        if let Ok(encoded) = serde_json::to_vec(&req_err) {
            let _ = serde_json::from_slice::<jmap_proto::error::RequestError>(&encoded);
        }
    }

    if let Ok(meth_err) = serde_json::from_slice::<jmap_proto::error::MethodError>(data) {
        if let Ok(encoded) = serde_json::to_vec(&meth_err) {
            let _ = serde_json::from_slice::<jmap_proto::error::MethodError>(&encoded);
        }
    }

    if let Ok(set_err) = serde_json::from_slice::<jmap_proto::error::SetError>(data) {
        if let Ok(encoded) = serde_json::to_vec(&set_err) {
            let _ = serde_json::from_slice::<jmap_proto::error::SetError>(&encoded);
        }
    }
});
