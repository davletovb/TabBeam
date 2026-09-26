//! Whole-host Claude fairness regression: ignored provider events must yield
//! so provider.status and request.cancel remain responsive.

mod support;

use std::time::Duration;

use serde_json::Value;
use support::{
    FakeClaude, PacedInput, request_id_for, serve_timed_claude,
};

fn request(method: &str, request_id: &str, payload: Value) -> String {
    serde_json::json!({
        "version": 1,
        "type": "request",
        "request_id": request_id_for(request_id),
        "method": method,
        "payload": payload
    })
    .to_string()
}

#[test]
fn ignored_claude_flood_does_not_starve_status_or_cancel() {
    let claude = FakeClaude::install("flooding", "signed-in");
    let ask = request(
        "conversation.send",
        "flood",
        serde_json::json!({
            "provider_id": "claude",
            "input": {"text": "flood forever"}
        }),
    );
    let status = request(
        "provider.status",
        "status",
        serde_json::json!({"provider_id": "claude"}),
    );
    let cancel = request(
        "request.cancel",
        "cancel",
        serde_json::json!({"target_request_id": request_id_for("flood")}),
    );
    let input = PacedInput::new(
        &[
            (Duration::ZERO, ask.as_str()),
            (Duration::from_millis(150), status.as_str()),
            (Duration::from_millis(150), cancel.as_str()),
        ],
        Duration::from_secs(10),
    );

    let flood_id = request_id_for("flood");
    let status_id = request_id_for("status");
    let cancel_id = request_id_for("cancel");
    let session = serve_timed_claude(
        claude.adapter(),
        input,
        &[flood_id.as_str(), status_id.as_str(), cancel_id.as_str()],
    );

    let status_events = session.of(&status_id);
    assert!(
        status_events
            .iter()
            .any(|event| event.event["event"] == "provider.status"),
        "status request was starved"
    );
    assert_eq!(
        status_events.last().map(|event| event.event["event"].as_str()),
        Some(Some("response.completed"))
    );

    let cancel_events = session.of(&cancel_id);
    assert!(
        cancel_events
            .iter()
            .any(|event| event.event["event"] == "request.cancelled"),
        "cancel request was starved"
    );

    let flood_events = session.of(&flood_id);
    assert!(
        flood_events.iter().any(|event| {
            event.event["event"] == "response.failed"
                && event.event["payload"]["error"]["code"] == "REQUEST_CANCELLED"
        }),
        "flooding request was not cancelled"
    );

    // Both side requests should finish promptly after their input frames rather
    // than waiting for the fake Claude flood to end.
    assert!(
        status_events
            .last()
            .is_some_and(|event| event.at.duration_since(session.sent[1]) < Duration::from_secs(2))
    );
    assert!(
        cancel_events
            .last()
            .is_some_and(|event| event.at.duration_since(session.sent[2]) < Duration::from_secs(2))
    );
}
