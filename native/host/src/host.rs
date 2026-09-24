//! The host loop: emit `host.ready`, then validate and route one request per
//! frame until the extension closes the stream.

use std::io::{Read, Write};

use crate::framing::{self, FrameError};
use crate::protocol::events::{
    self, Authentication, Availability, Capabilities, Capability, ConversationCreated, ErrorBody,
    ErrorCode, Event, EventError, ProviderState, ProviderStatus, ResponseCompleted, ResponseDelta,
    ResponseStarted,
};
use crate::protocol::json::JsonStr;
use crate::protocol::request::{self, RequestId};
use crate::protocol::router::{self, Handlers};

/// Why the host stopped before a clean end of stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostError {
    /// Reading a request or writing an event failed.
    Io,
    /// The stream ended inside a frame.
    FrameTruncated,
    /// A frame declared a length above the frame cap.
    FrameTooLarge,
    /// A frame buffer could not be allocated.
    AllocationFailed,
}

impl HostError {
    /// Process exit status for this failure; a clean end of stream exits 0.
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::Io => 2,
            Self::FrameTruncated => 3,
            Self::FrameTooLarge => 4,
            Self::AllocationFailed => 5,
        }
    }
}

impl From<FrameError> for HostError {
    fn from(error: FrameError) -> Self {
        match error {
            FrameError::Io => Self::Io,
            FrameError::Truncated => Self::FrameTruncated,
            FrameError::TooLarge => Self::FrameTooLarge,
            FrameError::AllocationFailed => Self::AllocationFailed,
        }
    }
}

/// Runs the host until `input` reaches a clean end of stream.
///
/// Invalid requests are answered with `response.failed` and do not stop the
/// host; only framing and I/O failures do.
pub fn run<R: Read + ?Sized, W: Write + ?Sized>(
    input: &mut R,
    output: &mut W,
) -> Result<(), HostError> {
    events::write_host_ready(output).map_err(|_| HostError::Io)?;

    let mut handlers = ScaffoldHandlers;
    while let Some(frame) = framing::read_frame(input)? {
        let written = match request::parse_request(&frame) {
            Ok(request) => router::dispatch(&mut handlers, output, &request),
            Err(failure) => events::write_request_failure(output, &failure),
        };
        written.map_err(|_| HostError::Io)?;
    }
    Ok(())
}

const FAKE_PROVIDER_ID: &str = "fake";

const PROVIDER_NOT_INSTALLED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderNotFound,
    reason: "PROVIDER_NOT_INSTALLED",
    message: "The selected provider runtime is not installed.",
    retryable: false,
};

const FAKE_PROVIDER_STATUS: ProviderStatus<'static> = ProviderStatus {
    provider_id: FAKE_PROVIDER_ID,
    status: ProviderState {
        availability: Availability::Available,
        authentication: Authentication::Authenticated,
        capabilities: Capabilities {
            streaming: Capability::Supported,
            continuation: Capability::Supported,
            web_search: Capability::Unsupported,
            page_context: Capability::Supported,
            attachments: Capability::Unsupported,
            model_selection: Capability::Unsupported,
            cancellation: Capability::Unsupported,
        },
    },
};

/// Local scaffold routes: `provider_id: "fake"` answers with a deterministic
/// event sequence and every other provider is reported as not installed. Real
/// provider execution begins with NAT-04/NAT-05.
struct ScaffoldHandlers;

impl Handlers for ScaffoldHandlers {
    fn conversation_send<W: Write + ?Sized>(
        &mut self,
        output: &mut W,
        request_id: RequestId<'_>,
        provider_id: JsonStr<'_>,
        conversation_id: Option<JsonStr<'_>>,
    ) -> Result<(), EventError> {
        if !provider_id.equals_ascii(FAKE_PROVIDER_ID) {
            return events::write_failure(output, Some(request_id), PROVIDER_NOT_INSTALLED);
        }

        if conversation_id.is_none() {
            let created = ConversationCreated {
                conversation_id: "fake-conversation",
            };
            events::write_event(output, request_id, Event::ConversationCreated, &created)?;
        }
        let started = ResponseStarted {
            provider_id: FAKE_PROVIDER_ID,
        };
        events::write_event(output, request_id, Event::ResponseStarted, &started)?;
        let delta = ResponseDelta {
            text: "Fake provider response.",
        };
        events::write_event(output, request_id, Event::ResponseDelta, &delta)?;
        events::write_event(
            output,
            request_id,
            Event::ResponseCompleted,
            &ResponseCompleted {},
        )
    }

    fn provider_status<W: Write + ?Sized>(
        &mut self,
        output: &mut W,
        request_id: RequestId<'_>,
        provider_id: Option<JsonStr<'_>>,
    ) -> Result<(), EventError> {
        if provider_id.is_some_and(|id| !id.equals_ascii(FAKE_PROVIDER_ID)) {
            return events::write_failure(output, Some(request_id), PROVIDER_NOT_INSTALLED);
        }

        events::write_event(
            output,
            request_id,
            Event::ProviderStatus,
            &FAKE_PROVIDER_STATUS,
        )?;
        events::write_event(
            output,
            request_id,
            Event::ResponseCompleted,
            &ResponseCompleted {},
        )
    }

    fn request_cancel<W: Write + ?Sized>(
        &mut self,
        output: &mut W,
        request_id: RequestId<'_>,
        _target_request_id: RequestId<'_>,
    ) -> Result<(), EventError> {
        let error = ErrorBody {
            code: ErrorCode::InvalidRequest,
            reason: "UNKNOWN_TARGET_REQUEST",
            message: "The target request is not in flight.",
            retryable: false,
        };
        events::write_failure(output, Some(request_id), error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HOST_VERSION;
    use crate::framing::{MAX_FRAME_SIZE, PREFIX_SIZE};

    fn framed(payloads: &[&str]) -> Vec<u8> {
        let mut wire = Vec::new();
        for payload in payloads {
            framing::write_frame(&mut wire, payload.as_bytes()).unwrap();
        }
        wire
    }

    fn run_host(input: &[u8]) -> (Result<(), HostError>, Vec<String>) {
        let mut output = Vec::new();
        let result = run(&mut &input[..], &mut output);
        let mut wire = output.as_slice();
        let mut frames = Vec::new();
        while let Some(frame) = framing::read_frame(&mut wire).unwrap() {
            frames.push(String::from_utf8(frame).unwrap());
        }
        (result, frames)
    }

    fn event(request_id: &str, event: &str, payload: &str) -> String {
        format!(
            r#"{{"version":1,"type":"event","request_id":{request_id},"event":"{event}","payload":{payload}}}"#
        )
    }

    fn host_ready() -> String {
        event(
            "null",
            "host.ready",
            &format!(r#"{{"host_version":"{HOST_VERSION}","protocol_versions":[1]}}"#),
        )
    }

    #[test]
    fn empty_input_emits_only_host_ready() {
        assert_eq!(run_host(&[]), (Ok(()), vec![host_ready()]));
    }

    #[test]
    fn host_version_is_the_package_version() {
        assert_eq!(HOST_VERSION, "0.1.0-dev");
    }

    #[test]
    fn framing_failures_stop_the_host() {
        let (result, frames) = run_host(&[0x01, 0x00]);
        assert_eq!(result, Err(HostError::FrameTruncated));
        assert_eq!(frames, [host_ready()]);

        let oversized = u32::try_from(MAX_FRAME_SIZE + 1).unwrap().to_ne_bytes();
        let (result, _) = run_host(&oversized);
        assert_eq!(result, Err(HostError::FrameTooLarge));

        let mut truncated_payload = framed(&["{}"]);
        truncated_payload.truncate(PREFIX_SIZE + 1);
        let (result, _) = run_host(&truncated_payload);
        assert_eq!(result, Err(HostError::FrameTruncated));
    }

    #[test]
    fn exit_codes_are_stable() {
        let codes = [
            HostError::Io,
            HostError::FrameTruncated,
            HostError::FrameTooLarge,
            HostError::AllocationFailed,
        ]
        .map(HostError::exit_code);
        assert_eq!(codes, [2, 3, 4, 5]);
    }

    #[test]
    fn output_failures_are_io_errors() {
        struct ClosedPipe;

        impl Write for ClosedPipe {
            fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        assert_eq!(run(&mut &[][..], &mut ClosedPipe), Err(HostError::Io));
    }

    #[test]
    fn malformed_requests_do_not_stop_the_host() {
        let input = framed(&[
            "{not-json",
            r#"{"version":1,"type":"request","request_id":"req_malformed_flow","method":"provider.status","payload":{}}x"#,
            r#"{"version":1,"type":"request","request_id":"req_flow","method":"conversation.send","payload":{"provider_id":"fake","input":{"text":"Hello"}}}"#,
            r#"{"version":2,"type":"request","request_id":"r\u0065q_v2","method":"provider.status","payload":{}}"#,
            r#"{"version":1,"type":"request","request_id":"req_unknown","method":"unknown.method","payload":{}}"#,
        ]);
        let malformed = r#"{"error":{"code":"INVALID_REQUEST","reason":"MALFORMED_MESSAGE","message":"Malformed request.","retryable":false}}"#;

        let (result, frames) = run_host(&input);
        assert_eq!(result, Ok(()));
        assert_eq!(
            frames,
            [
                host_ready(),
                event("null", "response.failed", malformed),
                event(r#""req_malformed_flow""#, "response.failed", malformed),
                event(
                    r#""req_flow""#,
                    "conversation.created",
                    r#"{"conversation_id":"fake-conversation"}"#
                ),
                event(
                    r#""req_flow""#,
                    "response.started",
                    r#"{"provider_id":"fake"}"#
                ),
                event(
                    r#""req_flow""#,
                    "response.delta",
                    r#"{"text":"Fake provider response."}"#
                ),
                event(r#""req_flow""#, "response.completed", "{}"),
                event(
                    r#""r\u0065q_v2""#,
                    "response.failed",
                    r#"{"error":{"code":"INVALID_REQUEST","reason":"UNSUPPORTED_PROTOCOL_VERSION","message":"Unsupported protocol version.","retryable":false},"protocol":{"received_version":2,"supported_versions":[1]}}"#
                ),
                event(
                    r#""req_unknown""#,
                    "response.failed",
                    r#"{"error":{"code":"INVALID_REQUEST","reason":"UNKNOWN_METHOD","message":"Unsupported method.","retryable":false}}"#
                ),
            ]
        );
    }

    #[test]
    fn scaffold_routes_answer_fake_and_reject_other_providers() {
        let input = framed(&[
            r#"{"version":1,"type":"request","request_id":"req_continue","method":"conversation.send","payload":{"provider_id":"fake","conversation_id":"conv_1","input":{"text":"Hi"}}}"#,
            r#"{"version":1,"type":"request","request_id":"req_codex","method":"conversation.send","payload":{"provider_id":"codex","input":{"text":"Hi"}}}"#,
            r#"{"version":1,"type":"request","request_id":"req_status","method":"provider.status","payload":{}}"#,
            r#"{"version":1,"type":"request","request_id":"req_status_codex","method":"provider.status","payload":{"provider_id":"codex"}}"#,
            r#"{"version":1,"type":"request","request_id":"req_cancel","method":"request.cancel","payload":{"target_request_id":"req_continue"}}"#,
        ]);
        let not_installed = r#"{"error":{"code":"PROVIDER_NOT_FOUND","reason":"PROVIDER_NOT_INSTALLED","message":"The selected provider runtime is not installed.","retryable":false}}"#;

        let (result, frames) = run_host(&input);
        assert_eq!(result, Ok(()));
        assert_eq!(
            frames,
            [
                host_ready(),
                event(
                    r#""req_continue""#,
                    "response.started",
                    r#"{"provider_id":"fake"}"#
                ),
                event(
                    r#""req_continue""#,
                    "response.delta",
                    r#"{"text":"Fake provider response."}"#
                ),
                event(r#""req_continue""#, "response.completed", "{}"),
                event(r#""req_codex""#, "response.failed", not_installed),
                event(
                    r#""req_status""#,
                    "provider.status",
                    r#"{"provider_id":"fake","status":{"availability":"available","authentication":"authenticated","capabilities":{"streaming":true,"continuation":true,"web_search":false,"page_context":true,"attachments":false,"model_selection":false,"cancellation":false}}}"#
                ),
                event(r#""req_status""#, "response.completed", "{}"),
                event(r#""req_status_codex""#, "response.failed", not_installed),
                event(
                    r#""req_cancel""#,
                    "response.failed",
                    r#"{"error":{"code":"INVALID_REQUEST","reason":"UNKNOWN_TARGET_REQUEST","message":"The target request is not in flight.","retryable":false}}"#
                ),
            ]
        );
    }
}
