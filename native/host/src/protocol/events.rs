//! Protocol-v1 event emission (`docs/protocol/v1.md` §3, §6, §8) and the
//! normalized error and provider-status vocabulary
//! (`docs/protocol/errors-and-capabilities-v1.md`).

use std::io::Write;

use serde::{Serialize, Serializer};

use super::PROTOCOL_VERSION;
use super::request::{FailureKind, RequestFailure, RequestId};
use crate::HOST_VERSION;
use crate::framing::{self, FrameError};

/// v1 event names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    HostReady,
    ProviderStatus,
    ConversationCreated,
    ResponseStarted,
    ResponseDelta,
    ResponseSource,
    ResponseCompleted,
    ResponseFailed,
    RequestCancelled,
}

impl Event {
    pub const fn name(self) -> &'static str {
        match self {
            Self::HostReady => "host.ready",
            Self::ProviderStatus => "provider.status",
            Self::ConversationCreated => "conversation.created",
            Self::ResponseStarted => "response.started",
            Self::ResponseDelta => "response.delta",
            Self::ResponseSource => "response.source",
            Self::ResponseCompleted => "response.completed",
            Self::ResponseFailed => "response.failed",
            Self::RequestCancelled => "request.cancelled",
        }
    }
}

/// Why an event could not be written.
#[derive(Debug)]
pub enum EventError {
    Encode(serde_json::Error),
    Frame(FrameError),
}

/// Normalized error categories (DOC-02 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    HostNotInstalled,
    HostUnavailable,
    ProviderNotFound,
    ProviderNotAuthenticated,
    ProviderFailed,
    RequestCancelled,
    RequestTimeout,
    ContextUnavailable,
    InvalidRequest,
    InternalError,
}

/// The `error` object of a `response.failed` event (DOC-02 §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ErrorBody<'a> {
    pub code: ErrorCode,
    pub reason: &'a str,
    pub message: &'a str,
    pub retryable: bool,
}

/// Provider availability (DOC-02 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    Unavailable,
    NotFound,
    Unknown,
}

/// Provider authentication state (DOC-02 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Authentication {
    Authenticated,
    Unauthenticated,
    Unknown,
}

/// A capability value, serialized as `true`, `false`, or `"unknown"` (DOC-02 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Supported,
    Unsupported,
    Unknown,
}

impl Serialize for Capability {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Supported => serializer.serialize_bool(true),
            Self::Unsupported => serializer.serialize_bool(false),
            Self::Unknown => serializer.serialize_str("unknown"),
        }
    }
}

/// The v1 capability set (DOC-02 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    pub streaming: Capability,
    pub continuation: Capability,
    pub web_search: Capability,
    pub page_context: Capability,
    pub attachments: Capability,
    pub model_selection: Capability,
    pub cancellation: Capability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderState {
    pub availability: Availability,
    pub authentication: Authentication,
    pub capabilities: Capabilities,
}

/// Payload of a `provider.status` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderStatus<'a> {
    pub provider_id: &'a str,
    pub status: ProviderState,
}

/// Payload of a `conversation.created` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ConversationCreated<'a> {
    pub conversation_id: &'a str,
}

/// Payload of a `response.started` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ResponseStarted<'a> {
    pub provider_id: &'a str,
}

/// Payload of a `response.delta` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ResponseDelta<'a> {
    pub text: &'a str,
}

/// Payload of a `response.completed` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ResponseCompleted {}

#[derive(Serialize)]
struct HostReady {
    host_version: &'static str,
    protocol_versions: [i64; 1],
}

#[derive(Serialize)]
struct ResponseFailed<'a> {
    error: ErrorBody<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    protocol: Option<ProtocolVersions>,
}

#[derive(Serialize)]
struct ProtocolVersions {
    received_version: i64,
    supported_versions: [i64; 1],
}

/// Writes the `host.ready` event.
pub fn write_host_ready<W: Write + ?Sized>(output: &mut W) -> Result<(), EventError> {
    let payload = HostReady {
        host_version: HOST_VERSION,
        protocol_versions: [PROTOCOL_VERSION],
    };
    write_envelope(output, None, Event::HostReady, &payload)
}

/// Writes one event correlated to `request_id`.
pub fn write_event<W: Write + ?Sized, P: Serialize + ?Sized>(
    output: &mut W,
    request_id: RequestId<'_>,
    event: Event,
    payload: &P,
) -> Result<(), EventError> {
    write_envelope(output, Some(request_id), event, payload)
}

/// Writes a `response.failed` event. `request_id` is `None` only when no valid
/// ID could be recovered from the request (v1 §3).
pub fn write_failure<W: Write + ?Sized>(
    output: &mut W,
    request_id: Option<RequestId<'_>>,
    error: ErrorBody<'_>,
) -> Result<(), EventError> {
    let payload = ResponseFailed {
        error,
        protocol: None,
    };
    write_envelope(output, request_id, Event::ResponseFailed, &payload)
}

/// Writes the `response.failed` event for a rejected request (v1 §8).
pub fn write_request_failure<W: Write + ?Sized>(
    output: &mut W,
    failure: &RequestFailure<'_>,
) -> Result<(), EventError> {
    let (reason, message) = match failure.kind {
        FailureKind::Malformed => ("MALFORMED_MESSAGE", "Malformed request."),
        FailureKind::InvalidEnvelope => ("INVALID_ENVELOPE", "Invalid request envelope."),
        FailureKind::InvalidPayload => ("INVALID_PAYLOAD", "Invalid request payload."),
        FailureKind::UnknownMethod => ("UNKNOWN_METHOD", "Unsupported method."),
        FailureKind::UnsupportedVersion => (
            "UNSUPPORTED_PROTOCOL_VERSION",
            "Unsupported protocol version.",
        ),
    };

    // v1 §8.1: a correlated version failure also reports the versions involved.
    let protocol = match (failure.kind, failure.request_id, failure.received_version) {
        (FailureKind::UnsupportedVersion, Some(_), Some(received_version)) => {
            Some(ProtocolVersions {
                received_version,
                supported_versions: [PROTOCOL_VERSION],
            })
        }
        _ => None,
    };

    let payload = ResponseFailed {
        error: ErrorBody {
            code: ErrorCode::InvalidRequest,
            reason,
            message,
            retryable: false,
        },
        protocol,
    };
    write_envelope(output, failure.request_id, Event::ResponseFailed, &payload)
}

fn write_envelope<W: Write + ?Sized, P: Serialize + ?Sized>(
    output: &mut W,
    request_id: Option<RequestId<'_>>,
    event: Event,
    payload: &P,
) -> Result<(), EventError> {
    let mut frame = Vec::with_capacity(256);
    frame.extend_from_slice(br#"{"version":1,"type":"event","request_id":"#);
    match request_id {
        Some(request_id) => {
            frame.push(b'"');
            frame.extend_from_slice(request_id.raw());
            frame.push(b'"');
        }
        None => frame.extend_from_slice(b"null"),
    }
    frame.extend_from_slice(br#","event":""#);
    frame.extend_from_slice(event.name().as_bytes());
    frame.extend_from_slice(br#"","payload":"#);
    serde_json::to_writer(&mut frame, payload).map_err(EventError::Encode)?;
    frame.push(b'}');
    framing::write_frame(output, &frame).map_err(EventError::Frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::request::parse_request;

    fn frames(mut wire: &[u8]) -> Vec<String> {
        let mut frames = Vec::new();
        while let Some(frame) = framing::read_frame(&mut wire).unwrap() {
            frames.push(String::from_utf8(frame).unwrap());
        }
        frames
    }

    fn failure_event(input: &str) -> String {
        let failure = parse_request(input.as_bytes()).unwrap_err();
        let mut wire = Vec::new();
        write_request_failure(&mut wire, &failure).unwrap();
        frames(&wire).remove(0)
    }

    #[test]
    fn host_ready_is_uncorrelated() {
        let mut wire = Vec::new();
        write_host_ready(&mut wire).unwrap();
        assert_eq!(
            frames(&wire),
            [format!(
                r#"{{"version":1,"type":"event","request_id":null,"event":"host.ready","payload":{{"host_version":"{HOST_VERSION}","protocol_versions":[1]}}}}"#
            )]
        );
    }

    #[test]
    fn uncorrelated_failures_carry_a_null_request_id() {
        assert_eq!(
            failure_event("{not-json"),
            r#"{"version":1,"type":"event","request_id":null,"event":"response.failed","payload":{"error":{"code":"INVALID_REQUEST","reason":"MALFORMED_MESSAGE","message":"Malformed request.","retryable":false}}}"#
        );
    }

    #[test]
    fn request_ids_are_echoed_byte_for_byte() {
        assert_eq!(
            failure_event(
                r#"{"version":2,"type":"request","request_id":"r\u0065q_v2","method":"provider.status","payload":{}}"#
            ),
            r#"{"version":1,"type":"event","request_id":"r\u0065q_v2","event":"response.failed","payload":{"error":{"code":"INVALID_REQUEST","reason":"UNSUPPORTED_PROTOCOL_VERSION","message":"Unsupported protocol version.","retryable":false},"protocol":{"received_version":2,"supported_versions":[1]}}}"#
        );
    }

    #[test]
    fn every_failure_kind_has_a_stable_reason() {
        for (input, reason) in [
            (
                r#"{"version":1,"type":"request","request_id":"a","method":"x","payload":{},"y":1}"#,
                "INVALID_ENVELOPE",
            ),
            (
                r#"{"version":1,"type":"request","request_id":"a","method":"request.cancel","payload":{}}"#,
                "INVALID_PAYLOAD",
            ),
            (
                r#"{"version":1,"type":"request","request_id":"a","method":"x","payload":{}}"#,
                "UNKNOWN_METHOD",
            ),
        ] {
            let event = failure_event(input);
            assert!(event.contains(r#""request_id":"a""#), "{event}");
            assert!(
                event.contains(&format!(r#""reason":"{reason}""#)),
                "{event}"
            );
        }
    }

    #[test]
    fn provider_status_uses_the_normalized_vocabulary() {
        let request = parse_request(
            br#"{"version":1,"type":"request","request_id":"req","method":"provider.status","payload":{}}"#,
        )
        .unwrap();
        let status = ProviderStatus {
            provider_id: "codex",
            status: ProviderState {
                availability: Availability::NotFound,
                authentication: Authentication::Unknown,
                capabilities: Capabilities {
                    streaming: Capability::Supported,
                    continuation: Capability::Unsupported,
                    web_search: Capability::Unknown,
                    page_context: Capability::Supported,
                    attachments: Capability::Unsupported,
                    model_selection: Capability::Unknown,
                    cancellation: Capability::Supported,
                },
            },
        };

        let mut wire = Vec::new();
        write_event(
            &mut wire,
            request.request_id,
            Event::ProviderStatus,
            &status,
        )
        .unwrap();
        assert_eq!(
            frames(&wire),
            [
                r#"{"version":1,"type":"event","request_id":"req","event":"provider.status","payload":{"provider_id":"codex","status":{"availability":"not_found","authentication":"unknown","capabilities":{"streaming":true,"continuation":false,"web_search":"unknown","page_context":true,"attachments":false,"model_selection":"unknown","cancellation":true}}}}"#
            ]
        );
    }

    #[test]
    fn oversized_events_are_not_written() {
        let request = parse_request(
            br#"{"version":1,"type":"request","request_id":"req","method":"provider.status","payload":{}}"#,
        )
        .unwrap();
        let text = "x".repeat(crate::limits::MAX_FRAME_SIZE);
        let mut wire = Vec::new();

        let result = write_event(
            &mut wire,
            request.request_id,
            Event::ResponseDelta,
            &ResponseDelta { text: &text },
        );
        assert!(matches!(
            result,
            Err(EventError::Frame(FrameError::TooLarge))
        ));
        assert!(wire.is_empty());
    }
}
