// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use std::collections::HashMap;
use std::time::Duration;

use libfuzzer_sys::fuzz_target;

use jmap_client::websocket::{
    DEFAULT_MAX_FRAME_SIZE, DEFAULT_MAX_MESSAGE_SIZE, InboundFrameAction, WebSocketLimits,
    WebSocketPushTimeouts, WebSocketTimeouts, parse_inbound_message, parse_inbound_text,
};
use jmap_proto::websocket::{
    WebSocketPushDisable, WebSocketPushEnable, WebSocketRequest, WebSocketRequestError,
    WebSocketResponse,
};
use tungstenite::Message;
use tungstenite::protocol::CloseFrame;
use tungstenite::protocol::frame::coding::CloseCode;

fuzz_target!(|data: &[u8]| {
    // 1. Text parsing with raw UTF-8 and lossy UTF-8
    if let Ok(s) = std::str::from_utf8(data) {
        let action = parse_inbound_text(s);
        let debug = format!("{action:?}");
        let cloned = action.clone();
        assert_eq!(action, cloned);
        assert!(!debug.is_empty());
        inspect_action(&action);

        // Attempt direct deserialization of standard RFC 8887 frame types
        let _ = serde_json::from_str::<WebSocketResponse>(s);
        let _ = serde_json::from_str::<WebSocketRequestError>(s);
        let _ = serde_json::from_str::<WebSocketRequest>(s);
        let _ = serde_json::from_str::<WebSocketPushEnable>(s);
        let _ = serde_json::from_str::<WebSocketPushDisable>(s);
    }

    let lossy = String::from_utf8_lossy(data);
    let action_lossy = parse_inbound_text(&lossy);
    inspect_action(&action_lossy);

    // 2. Inbound tungstenite message parsing across all frame types
    let text_msg = Message::Text(lossy.into_owned().into());
    let act_text = parse_inbound_message(&text_msg);
    inspect_action(&act_text);

    let bin_msg = Message::Binary(data.to_vec().into());
    let act_bin = parse_inbound_message(&bin_msg);
    assert_eq!(act_bin, InboundFrameAction::Ignored);

    let ping_msg = Message::Ping(data.to_vec().into());
    let act_ping = parse_inbound_message(&ping_msg);
    assert_eq!(act_ping, InboundFrameAction::Ping(data.to_vec()));

    let pong_msg = Message::Pong(data.to_vec().into());
    let act_pong = parse_inbound_message(&pong_msg);
    assert_eq!(act_pong, InboundFrameAction::Pong(data.to_vec()));

    let close_msg_some = Message::Close(Some(CloseFrame {
        code: CloseCode::Normal,
        reason: String::from_utf8_lossy(&data[..data.len().min(125)])
            .into_owned()
            .into(),
    }));
    let act_close_some = parse_inbound_message(&close_msg_some);
    assert!(matches!(act_close_some, InboundFrameAction::Close(Some(_))));

    let close_msg_none = Message::Close(None);
    let act_close_none = parse_inbound_message(&close_msg_none);
    assert_eq!(act_close_none, InboundFrameAction::Close(None));

    // 3. Simulated interleaved response and push stream dispatch
    let mut pending: HashMap<String, u64> = HashMap::new();
    pending.insert("req-1".to_string(), 1);
    pending.insert("req-2".to_string(), 2);
    pending.insert("req-3".to_string(), 3);
    pending.insert("custom-id".to_string(), 4);

    let mut latest_push_state: Option<String> = None;
    let mut state_changes_count: usize = 0;

    // Chunk input by newlines or arbitrary slices
    for chunk in data.split(|&b| b == b'\n' || b == b'\r') {
        if chunk.is_empty() {
            continue;
        }
        let chunk_str = String::from_utf8_lossy(chunk);
        let action = parse_inbound_text(&chunk_str);
        match action {
            InboundFrameAction::Response {
                id: Some(id),
                response,
            } => {
                let _ = pending.remove(&id);
                let _ = response.responses_for("c1").count();
                let _ = response.responses_for(&id).count();
            }
            InboundFrameAction::RequestError {
                id: Some(id),
                error,
            } => {
                let _ = pending.remove(&id);
                let _ = error.status;
            }
            InboundFrameAction::StateChange {
                push_state,
                state_change,
            } => {
                state_changes_count = state_changes_count.saturating_add(1);
                if let Some(ps) = push_state {
                    latest_push_state = Some(ps);
                }
                for (account_id, changes) in &state_change.changed {
                    let _ = account_id.as_str();
                    for (typ, state) in changes {
                        let _ = typ.as_str();
                        let _ = state.as_str();
                    }
                }
            }
            _ => {}
        }
    }
    // Pending map remains bounded to initial count
    assert!(pending.len() <= 4);
    let _ = latest_push_state.as_deref();
    let _ = state_changes_count;

    // 4. WebSocketLimits configuration and formatting
    let default_limits = WebSocketLimits::default();
    assert_eq!(default_limits.max_message_size, DEFAULT_MAX_MESSAGE_SIZE);
    assert_eq!(default_limits.max_frame_size, DEFAULT_MAX_FRAME_SIZE);

    let custom_limits = WebSocketLimits {
        max_message_size: data.len(),
        max_frame_size: data.len().saturating_div(2),
    };
    let _ = format!("{custom_limits:?}");
    let cloned_limits = custom_limits;
    assert_eq!(custom_limits, cloned_limits);

    let zero_limits = WebSocketLimits {
        max_message_size: 0,
        max_frame_size: 0,
    };
    let _ = format!("{zero_limits:?}");

    // 5. Timeouts configuration and formatting
    let ws_timeouts = WebSocketTimeouts {
        connect: Duration::from_millis(data.len() as u64),
        request: Duration::from_millis(data.len() as u64 * 2),
    };
    let _ = format!("{ws_timeouts:?}");
    let cloned_ws = ws_timeouts;
    assert_eq!(ws_timeouts, cloned_ws);

    let push_timeouts = WebSocketPushTimeouts {
        connect: Duration::from_millis(data.len() as u64),
        ping_interval: Duration::from_millis(300),
        ping_timeout: Duration::from_millis(100),
        initial_backoff: Duration::from_millis(50),
        max_backoff: Duration::from_millis(1000),
    };
    let _ = format!("{push_timeouts:?}");
    let cloned_push = push_timeouts;
    assert_eq!(push_timeouts, cloned_push);

    let default_push = WebSocketPushTimeouts::default();
    let _ = format!("{default_push:?}");
    let cloned_default = default_push;
    assert_eq!(default_push, cloned_default);
});

fn inspect_action(action: &InboundFrameAction) {
    match action {
        InboundFrameAction::Response { id, response } => {
            let _ = id.as_deref();
            let _ = response.id.as_deref();
            let _ = response.session_state.as_ref();
            let _ = response.method_responses.len();
            let _ = response.extra.len();
            let _ = response.created_ids.as_ref();
            for resp in &response.method_responses {
                let _ = resp.name.as_str();
                let _ = resp.call_id.as_str();
            }
            let _ = response.responses_for("c1").count();
        }
        InboundFrameAction::RequestError { id, error } => {
            let _ = id.as_deref();
            let _ = error.id.as_deref();
            let _ = &error.error_type;
            let _ = error.status;
            let _ = error.detail.as_deref();
            let _ = error.extra.len();
        }
        InboundFrameAction::StateChange {
            state_change,
            push_state,
        } => {
            let _ = push_state.as_deref();
            let _ = state_change.changed.len();
            let _ = &state_change.kind;
            let _ = state_change.extra.len();
            for (acc, map) in &state_change.changed {
                let _ = acc.as_str();
                for (type_name, st) in map {
                    let _ = type_name.as_str();
                    let _ = st.as_str();
                }
            }
        }
        InboundFrameAction::Close(reason) => {
            let _ = reason.as_deref();
        }
        InboundFrameAction::Ping(payload) | InboundFrameAction::Pong(payload) => {
            let _ = payload.len();
        }
        InboundFrameAction::Ignored => {}
    }
}
