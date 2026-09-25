//! The host loop: emit `host.ready`, then validate and route one request per
//! frame until the extension closes the stream, recording the session's
//! lifecycle as diagnostics.

use std::borrow::Cow;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

use crate::HOST_VERSION;
use crate::diagnostics::{Diagnostics, LifecycleEvent, LoggedError, Record, loggable_id, millis};
use crate::framing::{self, FrameError};
use crate::protocol::events::{
    self, Authentication, Availability, Capabilities, Capability, ConversationCreated, ErrorBody,
    ErrorCode, Event, EventError, ProviderState, ProviderStatus, ResponseCompleted, ResponseDelta,
    ResponseStarted,
};
use crate::protocol::json::JsonStr;
use crate::protocol::request::{self, Method, Request, RequestFailure, RequestId};
use crate::protocol::router::{self, End, Handlers, Outcome};

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

    /// Why the host stopped, as its `host.stopped` record says.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Io => "io_error",
            Self::FrameTruncated => "frame_truncated",
            Self::FrameTooLarge => "frame_too_large",
            Self::AllocationFailed => "allocation_failed",
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

/// Runs the host until `input` reaches a clean end of stream, recording its
/// lifecycle in `log` (OBS-01).
///
/// Invalid requests are answered with `response.failed` and do not stop the
/// host; only framing and I/O failures do.
pub fn run<R: Read + ?Sized, W: Write + ?Sized, L: Write>(
    input: &mut R,
    output: &mut W,
    log: &mut Diagnostics<L>,
) -> Result<(), HostError> {
    let started = Instant::now();
    let mut record = Record::new(LifecycleEvent::HostStarted);
    record.host_version = Some(HOST_VERSION);
    record.pid = Some(std::process::id());
    log.record(&record);

    let mut counts = Counts::default();
    let result = serve(&mut ScaffoldHandlers, input, output, log, &mut counts);

    let mut record = Record::new(LifecycleEvent::HostStopped);
    record.reason = Some(result.map_or_else(HostError::reason, |()| "end_of_input"));
    record.exit_code = Some(result.map_or_else(HostError::exit_code, |()| 0));
    record.duration_ms = Some(millis(started.elapsed()));
    record.requests = Some(counts.requests);
    record.rejected = Some(counts.rejected);
    log.record(&record);
    result
}

/// What the loop has handled, for the `host.stopped` record.
#[derive(Default)]
struct Counts {
    /// Requests that passed validation and reached a handler.
    requests: u64,
    /// Requests that failed validation.
    rejected: u64,
}

/// The request loop behind [`run`]. A request reaches `handlers` only after it
/// has passed validation, so provider work never starts for an invalid one
/// (SEC-01).
fn serve<H: Handlers, R: Read + ?Sized, W: Write + ?Sized, L: Write>(
    handlers: &mut H,
    input: &mut R,
    output: &mut W,
    log: &mut Diagnostics<L>,
    counts: &mut Counts,
) -> Result<(), HostError> {
    events::write_host_ready(output).map_err(|_| HostError::Io)?;

    // Every request read gets one record, even when the host stops while
    // answering it.
    while let Some(frame) = framing::read_frame(input)? {
        match request::parse_request(&frame) {
            Ok(request) => {
                let started = Instant::now();
                let (record, result) = match router::dispatch(handlers, output, &request) {
                    Ok(outcome) => (finished(&request, outcome, started.elapsed()), Ok(())),
                    Err(_) => (
                        aborted(&request, HostError::Io, started.elapsed()),
                        Err(HostError::Io),
                    ),
                };
                counts.requests += 1;
                log.record(&record);
                result?;
            }
            Err(failure) => {
                let written = events::write_request_failure(output, &failure);
                counts.rejected += 1;
                log.record(&rejected(&failure));
                written.map_err(|_| HostError::Io)?;
            }
        }
    }
    Ok(())
}

/// The record of a request that reached a handler, before its outcome is
/// known. It names the request's identifiers, never its input, context, or
/// other payload members.
fn handled<'a>(event: LifecycleEvent, request: &Request<'a>, elapsed: Duration) -> Record<'a> {
    let mut record = Record::new(event);
    record.request_id = Some(loggable_id(request.request_id.decode()));
    record.method = Some(request.method.name());
    match request.method {
        Method::ConversationSend {
            provider_id,
            conversation_id,
        } => {
            record.provider_id = Some(loggable_id(provider_id.decode()));
            record.conversation_id = conversation_id.map(|id| loggable_id(id.decode()));
        }
        Method::ProviderStatus { provider_id } => {
            record.provider_id = provider_id.map(|id| loggable_id(id.decode()));
        }
        Method::RequestCancel { target_request_id } => {
            record.target_request_id = Some(loggable_id(target_request_id.decode()));
        }
    }
    record.duration_ms = Some(millis(elapsed));
    record
}

/// The record of a request a handler finished. Its conversation is the one the
/// request created, if it created one, or else the one it continued.
fn finished<'a>(request: &Request<'a>, outcome: Outcome, elapsed: Duration) -> Record<'a> {
    let (event, error) = match outcome.end {
        End::Completed => (LifecycleEvent::RequestCompleted, None),
        End::Failed { code, reason } => (
            LifecycleEvent::RequestFailed,
            Some(LoggedError { code, reason }),
        ),
    };
    let mut record = handled(event, request, elapsed);
    if let Some(created) = outcome.created_conversation_id {
        record.conversation_id = Some(loggable_id(created));
    }
    record.error = error;
    record
}

/// The record of a request whose handler stopped with `error` before the
/// request ended, such as when stdout closed mid-answer.
fn aborted<'a>(request: &Request<'a>, error: HostError, elapsed: Duration) -> Record<'a> {
    let mut record = handled(LifecycleEvent::RequestAborted, request, elapsed);
    record.reason = Some(error.reason());
    record
}

/// The record of a request that failed validation. The frame itself is never
/// copied into it.
fn rejected<'a>(failure: &RequestFailure<'a>) -> Record<'a> {
    let mut record = Record::new(LifecycleEvent::RequestRejected);
    record.request_id = failure.request_id.map(|id| loggable_id(id.decode()));
    record.error = Some(LoggedError {
        code: ErrorCode::InvalidRequest,
        reason: failure.kind.reason(),
    });
    record
}

const FAKE_PROVIDER_ID: &str = "fake";
const FAKE_CONVERSATION_ID: &str = "fake-conversation";

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
    ) -> Result<Outcome, EventError> {
        if !provider_id.equals_ascii(FAKE_PROVIDER_ID) {
            return fail(output, request_id, PROVIDER_NOT_INSTALLED);
        }

        let created_conversation_id = conversation_id.is_none().then_some(FAKE_CONVERSATION_ID);
        if let Some(conversation_id) = created_conversation_id {
            let created = ConversationCreated { conversation_id };
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
        )?;
        Ok(Outcome {
            created_conversation_id: created_conversation_id.map(Cow::Borrowed),
            ..Outcome::COMPLETED
        })
    }

    fn provider_status<W: Write + ?Sized>(
        &mut self,
        output: &mut W,
        request_id: RequestId<'_>,
        provider_id: Option<JsonStr<'_>>,
    ) -> Result<Outcome, EventError> {
        if provider_id.is_some_and(|id| !id.equals_ascii(FAKE_PROVIDER_ID)) {
            return fail(output, request_id, PROVIDER_NOT_INSTALLED);
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
        )?;
        Ok(Outcome::COMPLETED)
    }

    fn request_cancel<W: Write + ?Sized>(
        &mut self,
        output: &mut W,
        request_id: RequestId<'_>,
        _target_request_id: RequestId<'_>,
    ) -> Result<Outcome, EventError> {
        let error = ErrorBody {
            code: ErrorCode::InvalidRequest,
            reason: "UNKNOWN_TARGET_REQUEST",
            message: "The target request is not in flight.",
            retryable: false,
        };
        fail(output, request_id, error)
    }
}

/// Ends the request with `response.failed` carrying `error`.
fn fail<W: Write + ?Sized>(
    output: &mut W,
    request_id: RequestId<'_>,
    error: ErrorBody<'static>,
) -> Result<Outcome, EventError> {
    events::write_failure(output, Some(request_id), error)?;
    Ok(Outcome::failed(&error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HOST_VERSION;
    use crate::framing::PREFIX_SIZE;
    use crate::limits::MAX_FRAME_SIZE;

    fn framed(payloads: &[&str]) -> Vec<u8> {
        let mut wire = Vec::new();
        for payload in payloads {
            framing::write_frame(&mut wire, payload.as_bytes()).unwrap();
        }
        wire
    }

    fn run_host(input: &[u8]) -> (Result<(), HostError>, Vec<String>) {
        let (result, frames, _) = run_logged(input);
        (result, frames)
    }

    /// Runs the host and returns its frames and its diagnostics records.
    fn run_logged(input: &[u8]) -> (Result<(), HostError>, Vec<String>, Vec<serde_json::Value>) {
        let mut output = Vec::new();
        let mut log = Diagnostics::new(Vec::new());
        let result = run(&mut &input[..], &mut output, &mut log);

        let mut wire = output.as_slice();
        let mut frames = Vec::new();
        while let Some(frame) = framing::read_frame(&mut wire).unwrap() {
            frames.push(String::from_utf8(frame).unwrap());
        }
        let records = String::from_utf8(log.into_inner())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        (result, frames, records)
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

        let mut log = Diagnostics::new(std::io::sink());
        assert_eq!(
            run(&mut &[][..], &mut ClosedPipe, &mut log),
            Err(HostError::Io)
        );
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

    /// Records the ID of every request that reaches a handler.
    #[derive(Default)]
    struct Recorder {
        request_ids: Vec<String>,
    }

    impl Recorder {
        fn record(&mut self, request_id: RequestId<'_>) -> Result<Outcome, EventError> {
            self.request_ids
                .push(String::from_utf8_lossy(request_id.raw()).into_owned());
            Ok(Outcome::COMPLETED)
        }
    }

    impl Handlers for Recorder {
        fn conversation_send<W: Write + ?Sized>(
            &mut self,
            _output: &mut W,
            request_id: RequestId<'_>,
            _provider_id: JsonStr<'_>,
            _conversation_id: Option<JsonStr<'_>>,
        ) -> Result<Outcome, EventError> {
            self.record(request_id)
        }

        fn provider_status<W: Write + ?Sized>(
            &mut self,
            _output: &mut W,
            request_id: RequestId<'_>,
            _provider_id: Option<JsonStr<'_>>,
        ) -> Result<Outcome, EventError> {
            self.record(request_id)
        }

        fn request_cancel<W: Write + ?Sized>(
            &mut self,
            _output: &mut W,
            request_id: RequestId<'_>,
            _target_request_id: RequestId<'_>,
        ) -> Result<Outcome, EventError> {
            self.record(request_id)
        }
    }

    fn conversation_request(request_id: &str, payload: &str) -> String {
        format!(
            r#"{{"version":1,"type":"request","request_id":"{request_id}","method":"conversation.send","payload":{payload}}}"#
        )
    }

    #[test]
    fn invalid_requests_never_reach_a_handler() {
        // Every validation failure is answered before routing, so no handler,
        // and therefore no provider work, sees an invalid request (SEC-01).
        let too_deep = format!(
            r#"{{"provider_id":"fake","input":{{"text":"hi"}},"extra":{}{}}}"#,
            "[".repeat(200),
            "]".repeat(200)
        );
        let invalid = [
            conversation_request("req_syntax", r#"{"provider_id":"fake""#),
            r#"{"version":1,"request_id":"req_envelope","method":"conversation.send","payload":{"provider_id":"fake","input":{"text":"hi"}}}"#.to_owned(),
            r#"{"version":1,"type":"request","request_id":"req_extra","method":"conversation.send","payload":{"provider_id":"fake","input":{"text":"hi"}},"extra":1}"#.to_owned(),
            r#"{"version":2,"type":"request","request_id":"req_version","method":"conversation.send","payload":{"provider_id":"fake","input":{"text":"hi"}}}"#.to_owned(),
            r#"{"version":1,"type":"request","request_id":"req_method","method":"provider.spawn","payload":{}}"#.to_owned(),
            conversation_request("req_payload", r#"{"provider_id":"fake","input":{"text":""}}"#),
            conversation_request(
                "req_duplicate",
                r#"{"provider_id":"fake","provider_id":"fake","input":{"text":"hi"}}"#,
            ),
            conversation_request("req_deep", &too_deep),
            conversation_request(
                &"a".repeat(crate::limits::MAX_REQUEST_ID_LENGTH + 1),
                r#"{"provider_id":"fake","input":{"text":"hi"}}"#,
            ),
            r#"{"version":1,"type":"request","request_id":"req_cancel","method":"request.cancel","payload":{"target_request_id":"bad id"}}"#.to_owned(),
        ];
        let valid = conversation_request(
            "req_valid",
            r#"{"provider_id":"fake","input":{"text":"hi"}}"#,
        );

        let mut frames: Vec<&str> = invalid.iter().map(String::as_str).collect();
        frames.push(&valid);
        let input = framed(&frames);

        let mut recorder = Recorder::default();
        let mut output = Vec::new();
        let mut log = Diagnostics::new(std::io::sink());
        assert_eq!(
            serve(
                &mut recorder,
                &mut input.as_slice(),
                &mut output,
                &mut log,
                &mut Counts::default()
            ),
            Ok(())
        );
        assert_eq!(recorder.request_ids, ["req_valid"]);

        let mut wire = output.as_slice();
        let mut events = Vec::new();
        while let Some(frame) = framing::read_frame(&mut wire).unwrap() {
            events.push(serde_json::from_slice::<serde_json::Value>(&frame).unwrap());
        }
        assert_eq!(events.len(), 1 + invalid.len());
        for failure in &events[1..] {
            assert_eq!(failure["event"], "response.failed", "{failure}");
            assert_eq!(
                failure["payload"]["error"]["code"], "INVALID_REQUEST",
                "{failure}"
            );
        }
    }

    #[test]
    fn provider_ids_are_names_not_paths_or_commands() {
        // A provider ID is only ever compared with the registry's names. One
        // that looks like a path or a shell command is just an unknown ID.
        for provider_id in [
            "/bin/sh",
            "../../../bin/sh",
            r"C:\Windows\System32\cmd.exe",
            "fake; rm -rf ~",
            "fake && id",
            "$(id)",
            "`id`",
            "fake\0",
            "fake/",
            " fake",
            "FAKE",
        ] {
            let id_json = serde_json::to_string(provider_id).unwrap();
            for request in [
                format!(
                    r#"{{"version":1,"type":"request","request_id":"req_send","method":"conversation.send","payload":{{"provider_id":{id_json},"input":{{"text":"hi"}}}}}}"#
                ),
                format!(
                    r#"{{"version":1,"type":"request","request_id":"req_status","method":"provider.status","payload":{{"provider_id":{id_json}}}}}"#
                ),
            ] {
                let (result, frames) = run_host(&framed(&[&request]));
                assert_eq!(result, Ok(()));
                assert_eq!(frames.len(), 2, "{provider_id:?}");
                assert!(
                    frames[1].contains(r#""reason":"PROVIDER_NOT_INSTALLED""#),
                    "{provider_id:?}: {}",
                    frames[1]
                );
            }
        }
    }

    fn record_events(records: &[serde_json::Value]) -> Vec<&str> {
        records
            .iter()
            .map(|record| record["event"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn diagnostics_record_the_session_lifecycle() {
        // The request ID is written with an escape; the record holds it decoded.
        let escaped_id = format!("r{}u0065q_ok", '\\');
        let input = framed(&[
            r#"{"version":1,"type":"request","request_id":"req_bad","method":"provider.spawn","payload":{}}"#,
            "{not json",
            &format!(
                r#"{{"version":1,"type":"request","request_id":"{escaped_id}","method":"conversation.send","payload":{{"provider_id":"fake","conversation_id":"conv_1","input":{{"text":"hi"}}}}}}"#
            ),
            r#"{"version":1,"type":"request","request_id":"req_status","method":"provider.status","payload":{"provider_id":"codex"}}"#,
            r#"{"version":1,"type":"request","request_id":"req_cancel","method":"request.cancel","payload":{"target_request_id":"req_ok"}}"#,
        ]);
        let (result, _, records) = run_logged(&input);
        assert_eq!(result, Ok(()));

        assert_eq!(
            record_events(&records),
            [
                "host.started",
                "request.rejected",
                "request.rejected",
                "request.completed",
                "request.failed",
                "request.failed",
                "host.stopped"
            ]
        );
        for record in &records {
            let ts = record["ts"].as_str().unwrap();
            assert_eq!(ts.len(), "2025-09-25T01:23:45.678Z".len(), "{ts}");
            assert!(ts.ends_with('Z') && ts.as_bytes()[10] == b'T', "{ts}");
        }

        assert_eq!(records[0]["host_version"], HOST_VERSION);
        assert_eq!(records[0]["pid"], std::process::id());

        assert_eq!(records[1]["request_id"], "req_bad");
        assert_eq!(
            records[1]["error"],
            serde_json::json!({"code": "INVALID_REQUEST", "reason": "UNKNOWN_METHOD"})
        );
        assert!(records[2].get("request_id").is_none());
        assert_eq!(records[2]["error"]["reason"], "MALFORMED_MESSAGE");

        assert_eq!(records[3]["request_id"], "req_ok");
        assert_eq!(records[3]["method"], "conversation.send");
        assert_eq!(records[3]["provider_id"], "fake");
        assert_eq!(records[3]["conversation_id"], "conv_1");
        assert!(records[3]["duration_ms"].is_u64());
        assert!(records[3].get("error").is_none());

        assert_eq!(records[4]["request_id"], "req_status");
        assert_eq!(records[4]["method"], "provider.status");
        assert_eq!(records[4]["provider_id"], "codex");
        assert_eq!(
            records[4]["error"],
            serde_json::json!({"code": "PROVIDER_NOT_FOUND", "reason": "PROVIDER_NOT_INSTALLED"})
        );
        assert_eq!(records[5]["method"], "request.cancel");
        assert_eq!(records[5]["target_request_id"], "req_ok");
        assert_eq!(records[5]["error"]["reason"], "UNKNOWN_TARGET_REQUEST");

        let stopped = &records[6];
        assert_eq!(stopped["reason"], "end_of_input");
        assert_eq!(stopped["exit_code"], 0);
        assert_eq!(stopped["requests"], 3);
        assert_eq!(stopped["rejected"], 2);
        assert!(stopped["duration_ms"].is_u64());
    }

    #[test]
    fn request_records_agree_with_each_terminal_event() {
        // Across every handler path, a request is recorded as failed exactly
        // when its last frame is response.failed, with the same code and reason.
        // Its conversation is the one conversation.created announced, or else
        // the one the request continued.
        let requests = [
            r#"{"version":1,"type":"request","request_id":"send_new","method":"conversation.send","payload":{"provider_id":"fake","input":{"text":"hi"}}}"#,
            r#"{"version":1,"type":"request","request_id":"send_existing","method":"conversation.send","payload":{"provider_id":"fake","conversation_id":"c1","input":{"text":"hi"}}}"#,
            r#"{"version":1,"type":"request","request_id":"send_unknown","method":"conversation.send","payload":{"provider_id":"codex","input":{"text":"hi"}}}"#,
            r#"{"version":1,"type":"request","request_id":"status_all","method":"provider.status","payload":{}}"#,
            r#"{"version":1,"type":"request","request_id":"status_fake","method":"provider.status","payload":{"provider_id":"fake"}}"#,
            r#"{"version":1,"type":"request","request_id":"status_unknown","method":"provider.status","payload":{"provider_id":"codex"}}"#,
            r#"{"version":1,"type":"request","request_id":"cancel","method":"request.cancel","payload":{"target_request_id":"send_new"}}"#,
        ];
        let (result, frames, records) = run_logged(&framed(&requests));
        assert_eq!(result, Ok(()));

        let mut conversations = std::collections::HashMap::new();
        for request in requests {
            let request: serde_json::Value = serde_json::from_str(request).unwrap();
            let conversation_id = request["payload"].get("conversation_id").cloned();
            conversations.insert(
                request["request_id"].as_str().unwrap().to_owned(),
                conversation_id,
            );
        }
        let mut last_frames = std::collections::HashMap::new();
        for frame in &frames[1..] {
            let frame: serde_json::Value = serde_json::from_str(frame).unwrap();
            let request_id = frame["request_id"].as_str().unwrap().to_owned();
            if frame["event"] == "conversation.created" {
                let created = frame["payload"]["conversation_id"].clone();
                conversations.insert(request_id.clone(), Some(created));
            }
            last_frames.insert(request_id, frame);
        }
        let request_records = &records[1..records.len() - 1];
        assert_eq!(request_records.len(), requests.len());

        let mut failures = 0;
        for record in request_records {
            let request_id = record["request_id"].as_str().unwrap();
            assert_eq!(
                record.get("conversation_id"),
                conversations[request_id].as_ref(),
                "{record}"
            );
            let last = &last_frames[request_id];
            if last["event"] == "response.failed" {
                failures += 1;
                assert_eq!(record["event"], "request.failed", "{record}");
                let error = &last["payload"]["error"];
                assert_eq!(record["error"]["code"], error["code"], "{record}");
                assert_eq!(record["error"]["reason"], error["reason"], "{record}");
            } else {
                assert_eq!(record["event"], "request.completed", "{record}");
            }
        }
        assert_eq!(failures, 3);
        assert_eq!(request_records[0]["conversation_id"], "fake-conversation");
        assert_eq!(request_records[1]["conversation_id"], "c1");
    }

    #[test]
    fn a_request_the_host_stops_answering_is_still_recorded() {
        /// Accepts `capacity` bytes, then fails like a closed pipe.
        struct BreakingPipe {
            capacity: usize,
        }

        impl Write for BreakingPipe {
            fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
                if self.capacity == 0 {
                    return Err(std::io::ErrorKind::BrokenPipe.into());
                }
                let written = buffer.len().min(self.capacity);
                self.capacity -= written;
                Ok(written)
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        // stdout closes right after host.ready, while the request is answered.
        let run_until_stdout_closes = |request: &str| {
            let mut ready = Vec::new();
            events::write_host_ready(&mut ready).unwrap();
            let mut output = BreakingPipe {
                capacity: ready.len(),
            };
            let mut log = Diagnostics::new(Vec::new());
            let result = run(&mut framed(&[request]).as_slice(), &mut output, &mut log);
            assert_eq!(result, Err(HostError::Io));
            let records: Vec<serde_json::Value> = String::from_utf8(log.into_inner())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let stopped = &records[records.len() - 1];
            assert_eq!(stopped["reason"], "io_error");
            assert_eq!(stopped["exit_code"], 2);
            records
        };

        let records = run_until_stdout_closes(
            r#"{"version":1,"type":"request","request_id":"req_cut","method":"conversation.send","payload":{"provider_id":"fake","conversation_id":"conv_1","input":{"text":"hi"}}}"#,
        );
        assert_eq!(
            record_events(&records),
            ["host.started", "request.aborted", "host.stopped"]
        );
        let aborted = &records[1];
        assert_eq!(aborted["request_id"], "req_cut");
        assert_eq!(aborted["method"], "conversation.send");
        assert_eq!(aborted["provider_id"], "fake");
        assert_eq!(aborted["conversation_id"], "conv_1");
        assert_eq!(aborted["reason"], "io_error");
        assert!(aborted["duration_ms"].is_u64());
        assert!(aborted.get("error").is_none());
        assert_eq!(
            (&records[2]["requests"], &records[2]["rejected"]),
            (&1.into(), &0.into())
        );

        let records = run_until_stdout_closes(
            r#"{"version":1,"type":"request","request_id":"req_bad","method":"provider.spawn","payload":{}}"#,
        );
        assert_eq!(
            record_events(&records),
            ["host.started", "request.rejected", "host.stopped"]
        );
        assert_eq!(records[1]["request_id"], "req_bad");
        assert_eq!(
            (&records[2]["requests"], &records[2]["rejected"]),
            (&0.into(), &1.into())
        );
    }

    #[test]
    fn diagnostics_record_why_the_host_stopped() {
        let oversized = u32::try_from(MAX_FRAME_SIZE + 1).unwrap().to_ne_bytes();
        for (input, reason, exit_code) in [
            (&oversized[..], "frame_too_large", 4),
            (&[0x01, 0x00][..], "frame_truncated", 3),
        ] {
            let (_, _, records) = run_logged(input);
            let stopped = records.last().unwrap();
            assert_eq!(stopped["event"], "host.stopped");
            assert_eq!(stopped["reason"], reason);
            assert_eq!(stopped["exit_code"], exit_code);
        }
    }

    #[test]
    fn diagnostics_never_copy_request_content() {
        // Prompt text, page context, unknown members, and malformed frames all
        // carry the marker; only identifiers may appear in a record.
        const MARKER: &str = "SECRET-PROMPT-7f3a";
        let input = framed(&[
            &format!(
                r#"{{"version":1,"type":"request","request_id":"req_1","method":"conversation.send","payload":{{"provider_id":"fake","input":{{"text":"{MARKER}"}},"context":{{"page":"{MARKER}"}},"extra":"{MARKER}"}}}}"#
            ),
            &format!(
                r#"{{"version":1,"type":"request","request_id":"req_2","method":"conversation.send","payload":{{"provider_id":"none","input":{{"text":"{MARKER}"}}}}}}"#
            ),
            &format!(
                r#"{{"version":1,"type":"request","request_id":"req_3","method":"conversation.send","payload":{{"input":{{"text":"{MARKER}"}}}}}}"#
            ),
            &format!(
                r#"{{"version":1,"type":"request","request_id":"req_4","method":"conversation.send","payload":{{}},"{MARKER}":1}}"#
            ),
            &format!(r#"{{"request_id":"req_5","{MARKER}"#),
            MARKER,
        ]);
        let (result, _, records) = run_logged(&input);
        assert_eq!(result, Ok(()));
        assert_eq!(records.len(), 8);
        for record in &records {
            assert!(!record.to_string().contains(MARKER), "{record}");
        }
    }

    #[test]
    fn diagnostics_do_not_change_the_frames() {
        // A sink that fails, like a closed stderr, leaves stdout byte-for-byte
        // the same as a working one.
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }

        let input = framed(&[
            r#"{"version":1,"type":"request","request_id":"req_a","method":"conversation.send","payload":{"provider_id":"fake","input":{"text":"hi"}}}"#,
            "{not json",
            r#"{"version":1,"type":"request","request_id":"req_b","method":"provider.status","payload":{}}"#,
        ]);

        let mut logged = Vec::new();
        let working = run(
            &mut input.as_slice(),
            &mut logged,
            &mut Diagnostics::new(Vec::new()),
        );
        let mut unlogged = Vec::new();
        let broken = run(
            &mut input.as_slice(),
            &mut unlogged,
            &mut Diagnostics::new(Broken),
        );

        assert_eq!(working, Ok(()));
        assert_eq!(broken, Ok(()));
        assert_eq!(logged, unlogged);
    }
}
