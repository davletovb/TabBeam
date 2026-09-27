//! The host loop: emit `host.ready`, then serve requests until the extension
//! closes the stream, recording the session's lifecycle as diagnostics.
//!
//! A reader thread delivers Chrome's frames, and this loop owns everything
//! else. Each valid request becomes an [`Exchange`] with a provider, which the
//! loop pumps without ever blocking on it, so requests run side by side and a
//! `request.cancel` is read while its target is still streaming. A new request
//! is pumped before the next frame is read, so a request that is answered at
//! once, such as the fake scaffold's, completes in order.
//!
//! The loop enforces the request lifecycle: request IDs are unique among the
//! requests in flight; a cancelled request ends with `REQUEST_CANCELLED` before
//! its cancellation is confirmed; a provider that takes too long ends with
//! `REQUEST_TIMEOUT`; and nothing is sent for a request after its terminal
//! event.

use std::borrow::Cow;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::HOST_VERSION;
use crate::diagnostics::{
    self, Diagnostics, LifecycleEvent, LoggedError, Record, issued_id, millis,
};
use crate::limits::MAX_FRAME_SIZE;
use crate::protocol::events::{
    self, Capability, ConversationCreated, ErrorBody, ErrorCode, Event, ProviderStatus,
    RequestCancelled, ResponseCompleted, ResponseDelta, ResponseSource, ResponseStarted,
};
use crate::protocol::request::{self, Method, RequestFailure, RequestId};
use crate::providers::{Exchange, Providers, Scripted, SendRequest, StatusOfAll, Timeouts, Update};
use crate::search::{SearchProviders, SearchRequest, SynthesisExchange};
use pervue_core::framing::{self, FrameError};
use pervue_core::stream::split_text;

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

/// How often the loop checks running requests while it waits for a frame.
const TICK: Duration = Duration::from_millis(10);

/// How long the loop delivers one request's updates before it turns to the
/// other requests and to new frames, so a provider that floods its output
/// can't hold up the rest, or its own cancellation.
const SLICE: Duration = Duration::from_millis(5);

/// Largest `response.delta` text. JSON escaping at most sixfolds text, so a
/// delta this size always fits in one frame.
const MAX_DELTA_BYTES: usize = MAX_FRAME_SIZE / 16;

/// How long requests still running at the end of input get to stop. Chrome
/// kills a host soon after closing its stdin, so this is short.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(250);

/// How long a cancelled `provider.status` check gets to stop.
const STATUS_STOP_GRACE: Duration = Duration::from_millis(250);

/// How long past its grace period a stopping request may take before the host
/// drops it, which kills its processes. Adapters stop within the grace
/// period; this bounds a broken one.
const STOP_SLACK: Duration = Duration::from_secs(1);

const PROVIDER_NOT_INSTALLED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderNotFound,
    reason: "PROVIDER_NOT_INSTALLED",
    message: "Pervue's companion app doesn't support this AI provider yet. Update it, then try again.",
    retryable: false,
};

const SEARCH_BACKEND_NOT_FOUND: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::SearchFailed,
    reason: "SEARCH_BACKEND_NOT_FOUND",
    message: "Pervue's companion app doesn't support this search backend yet.",
    retryable: false,
};

const SEARCH_TIMEOUT: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::SearchFailed,
    reason: "SEARCH_TIMEOUT",
    message: "Web search took too long. Try again.",
    retryable: true,
};

const PAGE_CONTEXT_UNSUPPORTED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InvalidRequest,
    reason: "PAGE_CONTEXT_UNSUPPORTED",
    message: "This AI provider can't use browser context. Choose No context, then ask again.",
    retryable: false,
};

const MODEL_SELECTION_UNSUPPORTED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InvalidRequest,
    reason: "MODEL_SELECTION_UNSUPPORTED",
    message: "This AI provider can't switch models. Choose its default model, then ask again.",
    retryable: false,
};

const UNKNOWN_TARGET_REQUEST: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InvalidRequest,
    reason: "UNKNOWN_TARGET_REQUEST",
    message: "The target request is not in flight.",
    retryable: false,
};

const DUPLICATE_REQUEST_ID: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InvalidRequest,
    reason: "DUPLICATE_REQUEST_ID",
    message: "Another request with this ID is still in flight.",
    retryable: false,
};

const CANCELLED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::RequestCancelled,
    reason: "USER_CANCELLED",
    message: "Stopped. You can ask again.",
    retryable: true,
};

const INPUT_CLOSED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::RequestCancelled,
    reason: "INPUT_CLOSED",
    message: "Stopped because Pervue closed. You can ask again.",
    retryable: true,
};

const START_TIMEOUT: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::RequestTimeout,
    reason: "PROVIDER_START_TIMEOUT",
    message: "The answer took too long to start. Try again.",
    retryable: true,
};

const RESPONSE_TIMEOUT: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::RequestTimeout,
    reason: "PROVIDER_RESPONSE_TIMEOUT",
    message: "The answer stopped arriving. Try again.",
    retryable: true,
};

/// An exchange ended as if cancelled when nothing cancelled it: a bug in
/// the adapter.
const STOPPED_UNASKED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InternalError,
    reason: "INTERNAL_STATE_ERROR",
    message: "The answer stopped unexpectedly. Try again.",
    retryable: false,
};

/// Runs the host with the providers of an installed host until `input`
/// reaches a clean end of stream, recording its lifecycle in `log` (OBS-01).
///
/// Invalid requests are answered with `response.failed` and do not stop the
/// host; only framing and I/O failures do.
pub fn run<R, W, L>(
    input: &mut R,
    output: &mut W,
    log: &mut Diagnostics<L>,
) -> Result<(), HostError>
where
    R: Read + Send + ?Sized,
    W: Write + ?Sized,
    L: Write,
{
    let providers = Providers::installed();
    let searches = SearchProviders::installed();
    run_with_services(&providers, &searches, input, output, log)
}

/// Runs the host as [`run`] does, serving `providers`.
pub fn run_with<R, W, L>(
    providers: &Providers,
    input: &mut R,
    output: &mut W,
    log: &mut Diagnostics<L>,
) -> Result<(), HostError>
where
    R: Read + Send + ?Sized,
    W: Write + ?Sized,
    L: Write,
{
    // Explicit-provider runs are used by tests and fuzzing and must never
    // discover credentials or start network search processes implicitly.
    let searches = SearchProviders::new(Vec::new());
    run_with_services(providers, &searches, input, output, log)
}

/// Runs the host with explicit model and search registries. Tests use this to
/// prove the search -> synthesis path without network access.
pub fn run_with_services<R, W, L>(
    providers: &Providers,
    searches: &SearchProviders,
    input: &mut R,
    output: &mut W,
    log: &mut Diagnostics<L>,
) -> Result<(), HostError>
where
    R: Read + Send + ?Sized,
    W: Write + ?Sized,
    L: Write,
{
    let started = Instant::now();
    let mut record = Record::new(LifecycleEvent::HostStarted);
    record.host_version = Some(HOST_VERSION);
    record.pid = Some(std::process::id());
    log.record(&record);

    let mut counts = Counts::default();
    let result = thread::scope(|scope| {
        // The reader hands over one frame at a time, so a flood of requests
        // waits in the pipe rather than in memory.
        let (sender, frames) = mpsc::sync_channel(0);
        scope.spawn(move || read_frames(input, &sender));
        let mut session = Session {
            providers,
            searches,
            output,
            log: &mut *log,
            counts: &mut counts,
            running: Vec::new(),
            conversations: HashSet::new(),
        };
        session.serve(&frames)
    });

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
    /// Requests that passed validation and were served.
    requests: u64,
    /// Requests that failed validation or reused an ID in flight.
    rejected: u64,
}

/// What the reader thread delivers.
enum Inbound {
    Frame(Vec<u8>),
    End,
    Broken(FrameError),
}

fn read_frames<R: Read + ?Sized>(input: &mut R, frames: &SyncSender<Inbound>) {
    loop {
        let (inbound, last) = match framing::read_frame(input) {
            Ok(Some(frame)) => (Inbound::Frame(frame), false),
            Ok(None) => (Inbound::End, true),
            Err(error) => (Inbound::Broken(error), true),
        };
        if frames.send(inbound).is_err() || last {
            return;
        }
    }
}

/// An event could not be written: the extension can no longer be reached.
#[derive(Debug)]
struct Undeliverable;

/// A request ID kept after its frame is released: the raw token to echo byte
/// for byte, and the decoded value that identifies the request.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Id {
    raw: Vec<u8>,
    value: String,
}

impl Id {
    fn new(request_id: RequestId<'_>) -> Self {
        Self {
            raw: request_id.raw().to_vec(),
            value: request_id.decode().into_owned(),
        }
    }
}

/// A `request.cancel` waiting for its target to stop.
struct Canceller {
    id: Id,
    target: String,
    started_at: Instant,
}

impl Canceller {
    fn record(&self, event: LifecycleEvent, error: Option<ErrorBody<'static>>) -> Record<'_> {
        let mut record = Record::new(event);
        record.request_id = Some(diagnostics::request_id(Cow::Borrowed(&self.id.value)));
        record.method = Some("request.cancel");
        record.target_request_id = Some(diagnostics::request_id(Cow::Borrowed(&self.target)));
        record.duration_ms = Some(millis(self.started_at.elapsed()));
        record.error = error.as_ref().map(logged);
        record
    }
}

/// What one turn of [`Session::pump_one`] left a request doing.
enum Pumped {
    /// It ended and was removed.
    Finished,
    /// It had nothing more ready.
    Waiting,
    /// Its slice ran out while it had more ready.
    Busy,
}

/// Why a running request is being stopped.
enum Stop {
    Cancelled(Vec<Canceller>),
    TimedOut(ErrorBody<'static>),
    InputClosed,
}

/// A request being served.
struct Running {
    id: Id,
    method: &'static str,
    provider_id: Option<String>,
    /// Whether the host serves the provider the request names.
    provider_served: bool,
    conversation_id: Option<String>,
    /// The active phase's limits for a `conversation.send`; none for status checks.
    timeouts: Option<Timeouts>,
    start_timeout_error: ErrorBody<'static>,
    stop_grace: Duration,
    started_at: Instant,
    last_update: Instant,
    response_started: bool,
    stop: Option<Stop>,
    /// When a stopping request is dropped if it still hasn't ended.
    stop_limit: Option<Instant>,
    exchange: Box<dyn Exchange>,
}

impl Running {
    fn new(
        id: Id,
        method: &'static str,
        provider: Option<(String, bool)>,
        conversation_id: Option<String>,
        timeouts: Option<Timeouts>,
        exchange: Box<dyn Exchange>,
    ) -> Self {
        Self::new_with_start_error(
            id,
            method,
            provider,
            conversation_id,
            timeouts,
            START_TIMEOUT,
            exchange,
        )
    }

    fn new_with_start_error(
        id: Id,
        method: &'static str,
        provider: Option<(String, bool)>,
        conversation_id: Option<String>,
        timeouts: Option<Timeouts>,
        start_timeout_error: ErrorBody<'static>,
        exchange: Box<dyn Exchange>,
    ) -> Self {
        let now = Instant::now();
        let (provider_id, provider_served) =
            provider.map_or((None, false), |(id, served)| (Some(id), served));
        Self {
            id,
            method,
            provider_id,
            provider_served,
            conversation_id,
            timeouts,
            start_timeout_error,
            stop_grace: timeouts.map_or(STATUS_STOP_GRACE, |timeouts| timeouts.stop_grace),
            started_at: now,
            last_update: now,
            response_started: false,
            stop: None,
            stop_limit: None,
            exchange,
        }
    }

    /// The timeout this request has run into, if any.
    fn expired(&self, now: Instant) -> Option<ErrorBody<'static>> {
        let timeouts = self.timeouts?;
        if self.response_started {
            (now.saturating_duration_since(self.last_update) >= timeouts.idle)
                .then_some(RESPONSE_TIMEOUT)
        } else {
            (now.saturating_duration_since(self.started_at) >= timeouts.start)
                .then_some(self.start_timeout_error)
        }
    }

    /// Stops the request for `stop`, unless it is already stopping.
    fn stop(&mut self, stop: Stop, grace: Duration) {
        if self.stop.is_none() {
            self.exchange.cancel(grace);
            self.stop = Some(stop);
            let now = Instant::now();
            self.stop_limit = Some(now.checked_add(grace + STOP_SLACK).unwrap_or(now));
        }
    }

    /// This request's record: identifiers and timing, never its content.
    /// `conversations` holds the conversations the host created.
    fn record<'a>(
        &'a self,
        event: LifecycleEvent,
        error: Option<ErrorBody<'static>>,
        conversations: &HashSet<String>,
    ) -> Record<'a> {
        let mut record = Record::new(event);
        record.request_id = Some(diagnostics::request_id(Cow::Borrowed(&self.id.value)));
        record.method = Some(self.method);
        record.provider_id = self
            .provider_id
            .as_deref()
            .map(|id| issued_id(id, self.provider_served));
        record.conversation_id = self
            .conversation_id
            .as_deref()
            .map(|id| issued_id(id, conversations.contains(id)));
        record.duration_ms = Some(millis(self.started_at.elapsed()));
        record.error = error.as_ref().map(logged);
        record
    }
}

fn logged(error: &ErrorBody<'static>) -> LoggedError {
    LoggedError {
        code: error.code,
        reason: error.reason,
    }
}

/// The state of one host session.
struct Session<'a, W: ?Sized, L: Write> {
    providers: &'a Providers,
    searches: &'a SearchProviders,
    output: &'a mut W,
    log: &'a mut Diagnostics<L>,
    counts: &'a mut Counts,
    running: Vec<Running>,
    /// The conversations the host's providers created, whose IDs records may
    /// hold.
    conversations: HashSet<String>,
}

impl<W: Write + ?Sized, L: Write> Session<'_, W, L> {
    fn serve(&mut self, frames: &Receiver<Inbound>) -> Result<(), HostError> {
        let ended = match self.read_until_input_ends(frames) {
            Ok(ended) => ended,
            Err(Undeliverable) => return Err(self.abort()),
        };
        match self.shut_down() {
            Ok(()) => ended,
            Err(Undeliverable) => Err(self.abort()),
        }
    }

    /// Serves frames until the input ends, cleanly or with a framing error.
    fn read_until_input_ends(
        &mut self,
        frames: &Receiver<Inbound>,
    ) -> Result<Result<(), HostError>, Undeliverable> {
        events::write_host_ready(self.output).map_err(|_| Undeliverable)?;
        // Whether a request still had updates ready when its slice ran out.
        let mut busy = false;
        loop {
            let inbound = if self.running.is_empty() {
                frames.recv().unwrap_or(Inbound::End)
            } else {
                // A busy request keeps the loop going: only check for a frame.
                let wait = if busy { Duration::ZERO } else { TICK };
                match frames.recv_timeout(wait) {
                    Ok(inbound) => inbound,
                    Err(RecvTimeoutError::Timeout) => {
                        busy = self.pump()?;
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => Inbound::End,
                }
            };
            match inbound {
                Inbound::Frame(frame) => self.handle(&frame)?,
                Inbound::End => return Ok(Ok(())),
                Inbound::Broken(error) => return Ok(Err(error.into())),
            }
            busy = self.pump()?;
        }
    }

    /// Validates one frame and starts serving it.
    fn handle(&mut self, frame: &[u8]) -> Result<(), Undeliverable> {
        let request = match request::parse_request(frame) {
            Ok(request) => request,
            Err(failure) => {
                self.counts.rejected += 1;
                let written =
                    events::write_request_failure(self.output, &failure).map_err(|_| Undeliverable);
                self.log.record(&rejected(&failure));
                return written;
            }
        };

        let id = Id::new(request.request_id);
        if self.in_flight(&id.value) {
            // v1 §2: IDs are unique among requests in flight. The duplicate
            // is refused; the request already running is unaffected.
            self.counts.rejected += 1;
            let written = write_failure(self.output, &id.raw, DUPLICATE_REQUEST_ID);
            let mut record = Record::new(LifecycleEvent::RequestRejected);
            record.request_id = Some(diagnostics::request_id(Cow::Borrowed(&id.value)));
            record.method = Some(request.method.name());
            record.error = Some(logged(&DUPLICATE_REQUEST_ID));
            self.log.record(&record);
            return written;
        }

        self.counts.requests += 1;
        let running = match request.method {
            Method::ConversationSend {
                provider_id,
                conversation_id,
                text,
                history,
                context,
                model,
                search,
            } => {
                let provider_id = provider_id.decode().into_owned();
                let conversation_id = conversation_id.map(|id| id.decode().into_owned());
                let provider = self.providers.get(&provider_id);
                let provider_served = provider.is_some();
                let question = text.decode().into_owned();
                let search_requested = search.is_some();
                let (exchange, timeouts): (Box<dyn Exchange>, _) = match provider {
                    Some(provider)
                        if context.is_some()
                            && provider.capabilities().page_context != Capability::Supported =>
                    {
                        (
                            Box::new(Scripted::failed(PAGE_CONTEXT_UNSUPPORTED)),
                            Some(provider.timeouts()),
                        )
                    }
                    // A model the provider can't be given is refused, not
                    // silently replaced by its default.
                    Some(provider)
                        if model.is_some()
                            && provider.capabilities().model_selection != Capability::Supported =>
                    {
                        (
                            Box::new(Scripted::failed(MODEL_SELECTION_UNSUPPORTED)),
                            Some(provider.timeouts()),
                        )
                    }
                    Some(provider) => {
                        let provider_timeouts = provider.timeouts();
                        let request = SendRequest {
                            text: question.clone(),
                            history,
                            conversation_id: conversation_id.clone(),
                            context,
                            model,
                            search_results: if search_requested {
                                Some(Vec::new())
                            } else {
                                None
                            },
                        };
                        match search {
                            Some(options) => match provider.preflight(&request) {
                                Err(error) => (
                                    Box::new(Scripted::failed(error)) as Box<dyn Exchange>,
                                    Some(provider_timeouts),
                                ),
                                Ok(()) => match self.searches.get(&options.backend_id) {
                                    Some(search_provider) => {
                                        let search_timeouts = search_provider.timeouts();
                                        let handle = search_provider.search(SearchRequest {
                                            query: question,
                                            count: options.count,
                                        });
                                        (
                                            Box::new(SynthesisExchange::new(
                                                handle,
                                                provider,
                                                provider_timeouts,
                                                request,
                                            ))
                                                as Box<dyn Exchange>,
                                            Some(search_timeouts),
                                        )
                                    }
                                    None => (
                                        Box::new(Scripted::failed(SEARCH_BACKEND_NOT_FOUND))
                                            as Box<dyn Exchange>,
                                        Some(provider_timeouts),
                                    ),
                                },
                            },
                            None => (provider.send(request), Some(provider_timeouts)),
                        }
                    }
                    None => (Box::new(Scripted::failed(PROVIDER_NOT_INSTALLED)), None),
                };
                if search_requested && timeouts.is_some() {
                    Running::new_with_start_error(
                        id,
                        "conversation.send",
                        Some((provider_id, provider_served)),
                        conversation_id,
                        timeouts,
                        SEARCH_TIMEOUT,
                        exchange,
                    )
                } else {
                    Running::new(
                        id,
                        "conversation.send",
                        Some((provider_id, provider_served)),
                        conversation_id,
                        timeouts,
                        exchange,
                    )
                }
            }
            Method::ProviderStatus { provider_id } => {
                let provider_id = provider_id.map(|id| id.decode().into_owned());
                let (exchange, served): (Box<dyn Exchange>, bool) = match provider_id.as_deref() {
                    None => (Box::new(StatusOfAll::new(self.providers)), false),
                    Some(provider_id) => match self.providers.get(provider_id) {
                        Some(provider) => (provider.status(), true),
                        None => (Box::new(Scripted::failed(PROVIDER_NOT_INSTALLED)), false),
                    },
                };
                Running::new(
                    id,
                    "provider.status",
                    provider_id.map(|id| (id, served)),
                    None,
                    None,
                    exchange,
                )
            }
            Method::ConversationForget {
                provider_id,
                conversation_id,
            } => {
                let provider_id = provider_id.decode().into_owned();
                let conversation_id = conversation_id.decode().into_owned();
                let provider = self.providers.get(&provider_id);
                let served = provider.is_some();
                let exchange: Box<dyn Exchange> = match provider {
                    Some(provider) => provider.forget(&conversation_id),
                    None => Box::new(Scripted::failed(PROVIDER_NOT_INSTALLED)),
                };
                Running::new(
                    id,
                    "conversation.forget",
                    Some((provider_id, served)),
                    Some(conversation_id),
                    None,
                    exchange,
                )
            }
            Method::RequestCancel { target_request_id } => {
                return self.cancel(Canceller {
                    id,
                    target: target_request_id.decode().into_owned(),
                    started_at: Instant::now(),
                });
            }
        };
        self.running.push(running);
        Ok(())
    }

    /// Whether a request or a pending cancellation already uses `id`.
    fn in_flight(&self, id: &str) -> bool {
        self.running.iter().any(|running| {
            running.id.value == id
                || matches!(&running.stop, Some(Stop::Cancelled(cancellers))
                    if cancellers.iter().any(|canceller| canceller.id.value == id))
        })
    }

    /// Starts cancelling the target of `canceller`, or refuses the
    /// cancellation when the target isn't running.
    fn cancel(&mut self, canceller: Canceller) -> Result<(), Undeliverable> {
        let target = self
            .running
            .iter_mut()
            .find(|running| running.id.value == canceller.target);
        match target {
            Some(target) if matches!(target.stop, None | Some(Stop::Cancelled(_))) => {
                match &mut target.stop {
                    Some(Stop::Cancelled(cancellers)) => cancellers.push(canceller),
                    _ => {
                        let grace = target.stop_grace;
                        target.stop(Stop::Cancelled(vec![canceller]), grace);
                    }
                }
                Ok(())
            }
            // Not running, or already ending for another reason.
            _ => {
                let written = write_failure(self.output, &canceller.id.raw, UNKNOWN_TARGET_REQUEST);
                let record =
                    canceller.record(LifecycleEvent::RequestFailed, Some(UNKNOWN_TARGET_REQUEST));
                log_outcome(self.log, record, written)
            }
        }
    }

    /// Delivers what every running request has ready, a slice at a time, and
    /// applies timeouts. Returns whether a request had more ready when its
    /// slice ran out.
    fn pump(&mut self) -> Result<bool, Undeliverable> {
        let mut busy = false;
        let mut index = 0;
        while index < self.running.len() {
            match self.pump_one(index)? {
                Pumped::Finished => {}
                Pumped::Waiting => index += 1,
                Pumped::Busy => {
                    busy = true;
                    index += 1;
                }
            }
        }
        Ok(busy)
    }

    /// Delivers what request `index` has ready, for one slice at most.
    fn pump_one(&mut self, index: usize) -> Result<Pumped, Undeliverable> {
        let now = Instant::now();
        let slice_end = now.checked_add(SLICE).unwrap_or(now);
        let running = &mut self.running[index];
        if let Some(error) = running.expired(now) {
            let grace = running.stop_grace;
            running.stop(Stop::TimedOut(error), grace);
        }
        loop {
            let running = &mut self.running[index];
            let Some(update) = running.exchange.next(now) else {
                if running
                    .stop_limit
                    .is_some_and(|limit| Instant::now() >= limit)
                {
                    // The adapter didn't stop in time: end the request anyway.
                    let running = self.running.remove(index);
                    self.finish(running, Update::Stopped)?;
                    return Ok(Pumped::Finished);
                }
                return Ok(Pumped::Waiting);
            };
            running.last_update = Instant::now();
            if let Update::ResetTimeouts(timeouts) = update {
                let now = Instant::now();
                running.timeouts = Some(timeouts);
                running.start_timeout_error = START_TIMEOUT;
                running.stop_grace = timeouts.stop_grace;
                running.started_at = now;
                running.last_update = now;
                running.response_started = false;
                continue;
            }
            if let Update::ConversationCreated(conversation_id) = &update {
                self.conversations.insert(conversation_id.clone());
            }
            if update.is_terminal() {
                let running = self.running.remove(index);
                self.finish(running, update)?;
                return Ok(Pumped::Finished);
            }
            forward(&mut *self.output, &mut self.running[index], update)?;
            if Instant::now() >= slice_end {
                return Ok(Pumped::Busy);
            }
        }
    }

    /// Writes the terminal event of `running`, and confirms its cancellations.
    fn finish(&mut self, running: Running, update: Update) -> Result<(), Undeliverable> {
        let failure = match (&running.stop, update) {
            (Some(Stop::Cancelled(_)), _) => Some(CANCELLED),
            (Some(Stop::TimedOut(error)), _) => Some(*error),
            (Some(Stop::InputClosed), _) => Some(INPUT_CLOSED),
            (None, Update::Failed(error)) => Some(error),
            (None, Update::Completed) => None,
            (None, _) => Some(STOPPED_UNASKED),
        };
        let (written, record) = match failure {
            None => (
                write_event(
                    self.output,
                    &running.id.raw,
                    Event::ResponseCompleted,
                    &ResponseCompleted {},
                ),
                running.record(LifecycleEvent::RequestCompleted, None, &self.conversations),
            ),
            Some(error) => (
                write_failure(self.output, &running.id.raw, error),
                running.record(
                    LifecycleEvent::RequestFailed,
                    Some(error),
                    &self.conversations,
                ),
            ),
        };
        let mut result = log_outcome(self.log, record, written);

        // v1 §7.6: the target has ended, so each cancellation is confirmed.
        if let Some(Stop::Cancelled(cancellers)) = &running.stop {
            for canceller in cancellers {
                let written = if result.is_ok() {
                    let payload = RequestCancelled {
                        target_request_id: &canceller.target,
                    };
                    write_event(
                        self.output,
                        &canceller.id.raw,
                        Event::RequestCancelled,
                        &payload,
                    )
                } else {
                    Err(Undeliverable)
                };
                let record = canceller.record(LifecycleEvent::RequestCompleted, None);
                result = log_outcome(self.log, record, written);
            }
        }
        result
    }

    /// Stops every request still running at the end of input and waits for
    /// them to finish.
    fn shut_down(&mut self) -> Result<(), Undeliverable> {
        for running in &mut self.running {
            running.stop(Stop::InputClosed, SHUTDOWN_GRACE);
        }
        loop {
            let busy = self.pump()?;
            if self.running.is_empty() {
                return Ok(());
            }
            if !busy {
                thread::sleep(TICK);
            }
        }
    }

    /// Ends every running request without a terminal event, once the
    /// extension can no longer be reached. Dropping each exchange kills its
    /// processes.
    fn abort(&mut self) -> HostError {
        for running in self.running.drain(..) {
            let record = running.record(LifecycleEvent::RequestAborted, None, &self.conversations);
            let _ = log_outcome(self.log, record, Err(Undeliverable));
            if let Some(Stop::Cancelled(cancellers)) = &running.stop {
                for canceller in cancellers {
                    let record = canceller.record(LifecycleEvent::RequestAborted, None);
                    let _ = log_outcome(self.log, record, Err(Undeliverable));
                }
            }
        }
        HostError::Io
    }
}

/// Writes one non-terminal update of `running`. Nothing is written once the
/// request is being stopped, except, later, its terminal event.
fn forward<W: Write + ?Sized>(
    output: &mut W,
    running: &mut Running,
    update: Update,
) -> Result<(), Undeliverable> {
    if running.stop.is_some() {
        return Ok(());
    }
    let raw = &running.id.raw;
    match update {
        Update::ConversationCreated(conversation_id) => {
            let payload = ConversationCreated {
                conversation_id: &conversation_id,
            };
            let written = write_event(output, raw, Event::ConversationCreated, &payload);
            running.conversation_id = Some(conversation_id);
            written
        }
        Update::Started { conversation_id } => {
            running.response_started = true;
            let payload = ResponseStarted {
                provider_id: running.provider_id.as_deref().unwrap_or_default(),
                conversation_id: conversation_id.as_deref(),
            };
            let written = write_event(output, raw, Event::ResponseStarted, &payload);
            if conversation_id.is_some() {
                running.conversation_id = conversation_id;
            }
            written
        }
        Update::Delta(text) => {
            for piece in split_text(&text, MAX_DELTA_BYTES) {
                write_event(
                    output,
                    raw,
                    Event::ResponseDelta,
                    &ResponseDelta { text: piece },
                )?;
            }
            Ok(())
        }
        Update::Source(source) => {
            let payload = ResponseSource {
                source_id: &source.id,
                data: &source,
            };
            write_event(output, raw, Event::ResponseSource, &payload)
        }
        Update::Status {
            provider_id,
            status,
        } => {
            let payload = ProviderStatus {
                provider_id: &provider_id,
                status,
            };
            write_event(output, raw, Event::ProviderStatus, &payload)
        }
        Update::ResetTimeouts(_)
        | Update::Activity
        | Update::Completed
        | Update::Failed(_)
        | Update::Stopped => Ok(()),
    }
}

fn write_event<W: Write + ?Sized, P: Serialize + ?Sized>(
    output: &mut W,
    raw_request_id: &[u8],
    event: Event,
    payload: &P,
) -> Result<(), Undeliverable> {
    events::write_event(output, raw_request_id, event, payload).map_err(|_| Undeliverable)
}

fn write_failure<W: Write + ?Sized>(
    output: &mut W,
    raw_request_id: &[u8],
    error: ErrorBody<'_>,
) -> Result<(), Undeliverable> {
    events::write_failure(output, raw_request_id, error).map_err(|_| Undeliverable)
}

/// Records `record`, or records the request as aborted if its event could
/// not be written. Returns `written`.
fn log_outcome<L: Write>(
    log: &mut Diagnostics<L>,
    mut record: Record<'_>,
    written: Result<(), Undeliverable>,
) -> Result<(), Undeliverable> {
    if written.is_err() {
        record.event = LifecycleEvent::RequestAborted;
        record.error = None;
        record.reason = Some(HostError::Io.reason());
    }
    log.record(&record);
    written
}

/// The record of a request that failed validation. The frame itself is never
/// copied into it.
fn rejected<'a>(failure: &RequestFailure<'a>) -> Record<'a> {
    let mut record = Record::new(LifecycleEvent::RequestRejected);
    record.request_id = failure
        .request_id
        .map(|id| diagnostics::request_id(id.decode()));
    record.error = Some(LoggedError {
        code: ErrorCode::InvalidRequest,
        reason: failure.kind.reason(),
    });
    record
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::io::Cursor;
    use std::rc::Rc;

    use serde_json::{Value, json};

    use super::*;
    use crate::HOST_VERSION;
    use crate::limits::MAX_FRAME_SIZE;
    use crate::protocol::events::Capabilities;
    use crate::providers::{Provider, fake};
    use crate::search::{SearchHandle, SearchProvider};
    use pervue_core::framing::PREFIX_SIZE;
    use pervue_core::protocol::Source;

    fn framed(payloads: &[&str]) -> Vec<u8> {
        let mut wire = Vec::new();
        for payload in payloads {
            framing::write_frame(&mut wire, payload.as_bytes()).unwrap();
        }
        wire
    }

    fn request(request_id: &str, method: &str, payload: &str) -> String {
        format!(
            r#"{{"version":1,"type":"request","request_id":"{request_id}","method":"{method}","payload":{payload}}}"#
        )
    }

    fn send(request_id: &str, provider_id: &str) -> String {
        request(
            request_id,
            "conversation.send",
            &format!(r#"{{"provider_id":"{provider_id}","input":{{"text":"hi"}}}}"#),
        )
    }

    fn cancel(request_id: &str, target: &str) -> String {
        request(
            request_id,
            "request.cancel",
            &format!(r#"{{"target_request_id":"{target}"}}"#),
        )
    }

    /// A request ID in the shape the extension gives every request, the only
    /// one diagnostics keep, spelling `name` (16 bytes at most) in hex.
    fn rid(name: &str) -> String {
        assert!(name.len() <= 16, "{name} is too long");
        let value = name
            .bytes()
            .fold(0_u128, |value, byte| (value << 8) | u128::from(byte));
        let hex = format!("{value:032x}");
        format!(
            "req_{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        )
    }

    struct Session {
        result: Result<(), HostError>,
        frames: Vec<String>,
        records: Vec<Value>,
    }

    impl Session {
        /// The frames as JSON, `host.ready` first.
        fn events(&self) -> Vec<Value> {
            self.frames
                .iter()
                .map(|frame| serde_json::from_str(frame).unwrap())
                .collect()
        }

        /// `(request_id, event)` for every frame after `host.ready`.
        fn sequence(&self) -> Vec<(String, String)> {
            self.events()[1..]
                .iter()
                .map(|event| {
                    (
                        event["request_id"].as_str().unwrap_or("null").to_owned(),
                        event["event"].as_str().unwrap().to_owned(),
                    )
                })
                .collect()
        }

        fn record_events(&self) -> Vec<&str> {
            self.records
                .iter()
                .map(|record| record["event"].as_str().unwrap())
                .collect()
        }
    }

    fn run_session_with_services<R: Read + Send>(
        providers: &Providers,
        searches: &SearchProviders,
        mut input: R,
    ) -> Session {
        let mut output = Vec::new();
        let mut log = Diagnostics::new(Vec::new());
        let result = run_with_services(providers, searches, &mut input, &mut output, &mut log);

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
        Session {
            result,
            frames,
            records,
        }
    }

    fn run_session<R: Read + Send>(providers: &Providers, input: R) -> Session {
        let searches = SearchProviders::new(Vec::new());
        run_session_with_services(providers, &searches, input)
    }

    fn run_host(input: &[u8]) -> Session {
        run_session(&Providers::scaffold(), input)
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

    fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
        expected
            .iter()
            .map(|(id, event)| ((*id).to_owned(), (*event).to_owned()))
            .collect()
    }

    /// Input that ends only after `linger`, so timeouts can fire first.
    struct LingeringInput {
        data: Cursor<Vec<u8>>,
        linger: Duration,
    }

    impl Read for LingeringInput {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let count = self.data.read(buffer)?;
            if count == 0 && !self.linger.is_zero() {
                thread::sleep(std::mem::take(&mut self.linger));
            }
            Ok(count)
        }
    }

    fn lingering(payloads: &[&str], linger: Duration) -> LingeringInput {
        LingeringInput {
            data: Cursor::new(framed(payloads)),
            linger,
        }
    }

    /// How a [`Controlled`] exchange behaves.
    #[derive(Clone, Copy, Default)]
    struct Script {
        /// Reports `Started`.
        starts: bool,
        /// Then reports this answer and completes.
        answer: Option<&'static str>,
        /// Reports `Stopped` without being cancelled.
        stops_unasked: bool,
        /// After a cancel, still reports a delta.
        talks_after_cancel: bool,
        /// After a cancel, takes this long to report `Stopped`; `None` never.
        stop_delay: Option<Duration>,
    }

    impl Script {
        /// Starts, then waits until cancelled.
        fn waits() -> Self {
            Self {
                starts: true,
                stop_delay: Some(Duration::ZERO),
                ..Self::default()
            }
        }

        /// Never starts; waits until cancelled.
        fn never_starts() -> Self {
            Self {
                stop_delay: Some(Duration::ZERO),
                ..Self::default()
            }
        }

        /// Starts, answers `text`, and completes.
        fn answers(text: &'static str) -> Self {
            Self {
                starts: true,
                answer: Some(text),
                ..Self::default()
            }
        }

        fn stopping_after(mut self, delay: Option<Duration>) -> Self {
            self.stop_delay = delay;
            self
        }
    }

    struct Controlled {
        script: Script,
        started: bool,
        answered: bool,
        talked: bool,
        cancelled_at: Option<Instant>,
        done: bool,
    }

    impl Exchange for Controlled {
        fn next(&mut self, _deadline: Instant) -> Option<Update> {
            if self.done {
                return None;
            }
            if let Some(cancelled_at) = self.cancelled_at {
                if self.script.talks_after_cancel && !self.talked {
                    self.talked = true;
                    return Some(Update::Delta("after cancel".to_owned()));
                }
                let delay = self.script.stop_delay?;
                if cancelled_at.elapsed() < delay {
                    return None;
                }
                self.done = true;
                return Some(Update::Stopped);
            }
            if self.script.starts && !self.started {
                self.started = true;
                return Some(Update::Started {
                    conversation_id: None,
                });
            }
            if self.script.stops_unasked {
                self.done = true;
                return Some(Update::Stopped);
            }
            let answer = self.script.answer?;
            if !self.answered {
                self.answered = true;
                return Some(Update::Delta(answer.to_owned()));
            }
            self.done = true;
            Some(Update::Completed)
        }

        fn cancel(&mut self, _grace: Duration) {
            if !self.done && self.cancelled_at.is_none() {
                self.cancelled_at = Some(Instant::now());
            }
        }
    }

    /// A test provider that follows `script` and records what it was asked.
    struct TestProvider {
        id: &'static str,
        script: Script,
        timeouts: Timeouts,
        capabilities: Capabilities,
        preflight_error: Option<ErrorBody<'static>>,
        calls: Rc<RefCell<Vec<String>>>,
    }

    impl TestProvider {
        fn new(id: &'static str, script: Script) -> Self {
            Self {
                id,
                script,
                timeouts: Timeouts {
                    start: Duration::from_secs(60),
                    idle: Duration::from_secs(60),
                    stop_grace: Duration::ZERO,
                },
                capabilities: fake::STATUS.capabilities,
                preflight_error: None,
                calls: Rc::default(),
            }
        }

        fn rejecting_preflight(mut self, error: ErrorBody<'static>) -> Self {
            self.preflight_error = Some(error);
            self
        }
    }

    impl Provider for TestProvider {
        fn id(&self) -> &str {
            self.id
        }

        fn timeouts(&self) -> Timeouts {
            self.timeouts
        }

        fn capabilities(&self) -> Capabilities {
            self.capabilities
        }

        fn preflight(&self, _request: &SendRequest) -> Result<(), ErrorBody<'static>> {
            self.preflight_error.map_or(Ok(()), Err)
        }

        fn status(&self) -> Box<dyn Exchange> {
            self.calls.borrow_mut().push("status".to_owned());
            Box::new(Scripted::new([
                Update::Status {
                    provider_id: self.id.to_owned(),
                    status: fake::STATUS,
                },
                Update::Completed,
            ]))
        }

        fn send(&self, request: SendRequest) -> Box<dyn Exchange> {
            let context = if request.context.is_some() {
                "+context"
            } else {
                ""
            };
            let model = request
                .model
                .map(|model| format!("+model={model}"))
                .unwrap_or_default();
            let sources = match request.search_results.as_ref() {
                None => String::new(),
                Some(sources) => format!("+sources={}", sources.len()),
            };
            self.calls
                .borrow_mut()
                .push(format!("send:{}{context}{model}{sources}", request.text));
            Box::new(Controlled {
                script: self.script,
                started: false,
                answered: false,
                talked: false,
                cancelled_at: None,
                done: false,
            })
        }
    }

    fn with(provider: TestProvider) -> Providers {
        Providers::new(vec![Box::new(fake::Fake), Box::new(provider)])
    }

    struct TestSearch {
        calls: Rc<RefCell<Vec<SearchRequest>>>,
    }

    impl SearchProvider for TestSearch {
        fn id(&self) -> &str {
            "brave"
        }

        fn timeouts(&self) -> Timeouts {
            Timeouts {
                start: Duration::from_secs(1),
                idle: Duration::from_secs(1),
                stop_grace: Duration::ZERO,
            }
        }

        fn search(&self, request: SearchRequest) -> SearchHandle {
            self.calls.borrow_mut().push(request);
            let results = Rc::new(RefCell::new(Some(vec![
                Source {
                    id: "src_search_1".to_owned(),
                    backend_id: "brave".to_owned(),
                    title: "One".to_owned(),
                    url: "https://example.com/one".to_owned(),
                    snippet: "First result".to_owned(),
                    source_name: Some("Example".to_owned()),
                    age: None,
                },
                Source {
                    id: "src_search_2".to_owned(),
                    backend_id: "brave".to_owned(),
                    title: "Two".to_owned(),
                    url: "https://example.org/two".to_owned(),
                    snippet: "Second result".to_owned(),
                    source_name: None,
                    age: Some("1 day ago".to_owned()),
                },
            ])));
            SearchHandle {
                exchange: Box::new(Scripted::new([Update::Completed])),
                results,
            }
        }
    }

    #[test]
    fn search_retrieval_is_normalized_before_provider_synthesis_and_source_events() {
        let provider = TestProvider::new("model", Script::answers("grounded answer"));
        let provider_calls = Rc::clone(&provider.calls);
        let providers = Providers::new(vec![Box::new(provider)]);
        let search_calls = Rc::new(RefCell::new(Vec::new()));
        let searches = SearchProviders::new(vec![Box::new(TestSearch {
            calls: Rc::clone(&search_calls),
        })]);

        let input = framed(&[
            r#"{"version":1,"type":"request","request_id":"req_search","method":"conversation.send","payload":{"provider_id":"model","input":{"text":"What changed?"},"search":{"backend_id":"brave","count":2}}}"#,
        ]);
        let mut output = Vec::new();
        let mut log = Diagnostics::new(Vec::new());
        let mut wire = input.as_slice();
        assert_eq!(
            run_with_services(&providers, &searches, &mut wire, &mut output, &mut log),
            Ok(())
        );

        let mut frames = output.as_slice();
        let mut events = Vec::new();
        while let Some(frame) = framing::read_frame(&mut frames).unwrap() {
            events.push(serde_json::from_slice::<Value>(&frame).unwrap());
        }
        let sequence: Vec<_> = events[1..]
            .iter()
            .map(|event| event["event"].as_str().unwrap())
            .collect();
        assert_eq!(
            sequence,
            [
                "response.started",
                "response.source",
                "response.source",
                "response.delta",
                "response.completed"
            ]
        );
        assert_eq!(events[2]["payload"]["source_id"], "src_search_1");
        assert_eq!(events[2]["payload"]["data"]["backend_id"], "brave");
        assert_eq!(events[3]["payload"]["source_id"], "src_search_2");
        assert_eq!(
            search_calls.borrow().as_slice(),
            &[SearchRequest {
                query: "What changed?".to_owned(),
                count: 2,
            }]
        );
        assert_eq!(
            provider_calls.borrow().as_slice(),
            &["send:What changed?+sources=2"]
        );
    }

    struct EmptySearch;

    impl SearchProvider for EmptySearch {
        fn id(&self) -> &str {
            "brave"
        }

        fn timeouts(&self) -> Timeouts {
            Timeouts {
                start: Duration::from_secs(1),
                idle: Duration::from_secs(1),
                stop_grace: Duration::ZERO,
            }
        }

        fn search(&self, _request: SearchRequest) -> SearchHandle {
            let results = Rc::new(RefCell::new(Some(Vec::new())));
            SearchHandle {
                exchange: Box::new(Scripted::new([Update::Completed])),
                results,
            }
        }
    }

    #[test]
    fn zero_search_results_still_reach_provider_as_search_turn() {
        let provider = TestProvider::new("model", Script::answers("no grounded sources"));
        let calls = Rc::clone(&provider.calls);
        let providers = Providers::new(vec![Box::new(provider)]);
        let searches = SearchProviders::new(vec![Box::new(EmptySearch)]);
        let session = run_session_with_services(
            &providers,
            &searches,
            Cursor::new(framed(&[
                r#"{"version":1,"type":"request","request_id":"req_empty_search","method":"conversation.send","payload":{"provider_id":"model","input":{"text":"What changed?"},"search":{"backend_id":"brave","count":2}}}"#,
            ])),
        );
        assert_eq!(session.result, Ok(()));
        assert_eq!(
            calls.borrow().as_slice(),
            &["send:What changed?+sources=0"]
        );
        assert!(
            session
                .events()
                .iter()
                .all(|event| event["event"] != "response.source")
        );
    }

    #[test]
    fn provider_preflight_blocks_search_before_query_leaves_host() {
        const BLOCKED: ErrorBody<'static> = ErrorBody {
            code: ErrorCode::InvalidRequest,
            reason: "PREFLIGHT_BLOCKED",
            message: "Blocked before search.",
            retryable: false,
        };
        let provider =
            TestProvider::new("model", Script::answers("unused")).rejecting_preflight(BLOCKED);
        let providers = Providers::new(vec![Box::new(provider)]);
        let search_calls = Rc::new(RefCell::new(Vec::new()));
        let searches = SearchProviders::new(vec![Box::new(TestSearch {
            calls: Rc::clone(&search_calls),
        })]);
        let session = run_session_with_services(
            &providers,
            &searches,
            Cursor::new(framed(&[
                r#"{"version":1,"type":"request","request_id":"req_preflight","method":"conversation.send","payload":{"provider_id":"model","input":{"text":"private query"},"search":{"backend_id":"brave","count":2}}}"#,
            ])),
        );
        assert_eq!(session.result, Ok(()));
        assert!(search_calls.borrow().is_empty());
        let failure = session
            .events()
            .into_iter()
            .find(|event| event["request_id"] == "req_preflight")
            .unwrap();
        assert_eq!(failure["event"], "response.failed");
        assert_eq!(failure["payload"]["error"]["reason"], "PREFLIGHT_BLOCKED");
    }

    struct StalledSearch;

    struct StalledSearchExchange {
        stopped: bool,
    }

    impl Exchange for StalledSearchExchange {
        fn next(&mut self, _deadline: Instant) -> Option<Update> {
            self.stopped.then_some(Update::Stopped)
        }

        fn cancel(&mut self, _grace: Duration) {
            self.stopped = true;
        }
    }

    impl SearchProvider for StalledSearch {
        fn id(&self) -> &str {
            "brave"
        }

        fn timeouts(&self) -> Timeouts {
            Timeouts {
                start: Duration::from_millis(20),
                idle: Duration::from_millis(20),
                stop_grace: Duration::ZERO,
            }
        }

        fn search(&self, _request: SearchRequest) -> SearchHandle {
            SearchHandle {
                exchange: Box::new(StalledSearchExchange { stopped: false }),
                results: Rc::new(RefCell::new(None)),
            }
        }
    }

    #[test]
    fn search_timeout_is_search_failure_not_provider_start_timeout() {
        let provider = TestProvider::new("model", Script::answers("unused"));
        let providers = Providers::new(vec![Box::new(provider)]);
        let searches = SearchProviders::new(vec![Box::new(StalledSearch)]);
        let session = run_session_with_services(
            &providers,
            &searches,
            lingering(
                &[
                    r#"{"version":1,"type":"request","request_id":"req_search_timeout","method":"conversation.send","payload":{"provider_id":"model","input":{"text":"slow"},"search":{"backend_id":"brave","count":2}}}"#,
                ],
                Duration::from_millis(100),
            ),
        );
        assert_eq!(session.result, Ok(()));
        let failure = session
            .events()
            .into_iter()
            .find(|event| event["request_id"] == "req_search_timeout")
            .unwrap();
        assert_eq!(failure["event"], "response.failed");
        assert_eq!(failure["payload"]["error"]["code"], "SEARCH_FAILED");
        assert_eq!(failure["payload"]["error"]["reason"], "SEARCH_TIMEOUT");
    }

    /// An exchange that always has another update ready, like a provider that
    /// never stops reporting progress.
    struct Flooding {
        cancelled: bool,
    }

    impl Exchange for Flooding {
        fn next(&mut self, _deadline: Instant) -> Option<Update> {
            Some(if self.cancelled {
                Update::Stopped
            } else {
                Update::Activity
            })
        }

        fn cancel(&mut self, _grace: Duration) {
            self.cancelled = true;
        }
    }

    /// Serves `flood` requests with a [`Flooding`] exchange.
    struct FloodingProvider;

    impl Provider for FloodingProvider {
        fn id(&self) -> &str {
            "flood"
        }

        fn timeouts(&self) -> Timeouts {
            Timeouts {
                start: Duration::from_secs(60),
                idle: Duration::from_secs(60),
                stop_grace: Duration::ZERO,
            }
        }

        fn capabilities(&self) -> Capabilities {
            fake::STATUS.capabilities
        }

        fn status(&self) -> Box<dyn Exchange> {
            Box::new(Scripted::new([Update::Completed]))
        }

        fn send(&self, _request: SendRequest) -> Box<dyn Exchange> {
            Box::new(Flooding { cancelled: false })
        }
    }

    #[test]
    fn a_request_that_never_runs_dry_cannot_hold_up_the_others() {
        // The session runs on its own thread, so a host stuck on the flood
        // fails the test instead of hanging it.
        let (done, finished) = mpsc::channel();
        thread::spawn(move || {
            let providers = Providers::new(vec![Box::new(fake::Fake), Box::new(FloodingProvider)]);
            let session = run_session(
                &providers,
                lingering(
                    &[
                        &send("req_flood", "flood"),
                        &send("req_fake", "fake"),
                        &cancel("req_stop", "req_flood"),
                    ],
                    Duration::from_millis(200),
                ),
            );
            let _ = done.send((session.result, session.events()));
        });
        let (result, events) = finished
            .recv_timeout(Duration::from_secs(10))
            .expect("the host should not be stuck on the flood");
        assert_eq!(result, Ok(()));

        let ending = |request_id: &str| {
            events
                .iter()
                .rfind(|event| event["request_id"] == request_id)
                .map(|event| event["event"].as_str().unwrap().to_owned())
        };
        assert_eq!(ending("req_fake").as_deref(), Some("response.completed"));
        assert_eq!(ending("req_stop").as_deref(), Some("request.cancelled"));
        let flood = events
            .iter()
            .rfind(|event| event["request_id"] == "req_flood")
            .unwrap();
        assert_eq!(flood["payload"]["error"]["code"], "REQUEST_CANCELLED");
    }

    #[test]
    fn empty_input_emits_only_host_ready() {
        let session = run_host(&[]);
        assert_eq!(session.result, Ok(()));
        assert_eq!(session.frames, [host_ready()]);
    }

    #[test]
    fn host_version_is_the_package_version() {
        assert_eq!(HOST_VERSION, "0.1.0-dev");
    }

    #[test]
    fn framing_failures_stop_the_host() {
        let session = run_host(&[0x01, 0x00]);
        assert_eq!(session.result, Err(HostError::FrameTruncated));
        assert_eq!(session.frames, [host_ready()]);

        let oversized = u32::try_from(MAX_FRAME_SIZE + 1).unwrap().to_ne_bytes();
        assert_eq!(run_host(&oversized).result, Err(HostError::FrameTooLarge));

        let mut truncated_payload = framed(&["{}"]);
        truncated_payload.truncate(PREFIX_SIZE + 1);
        assert_eq!(
            run_host(&truncated_payload).result,
            Err(HostError::FrameTruncated)
        );
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
            run_with(
                &Providers::scaffold(),
                &mut &[][..],
                &mut ClosedPipe,
                &mut log
            ),
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

        let session = run_host(&input);
        assert_eq!(session.result, Ok(()));
        assert_eq!(
            session.frames,
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
                    r#"{"provider_id":"fake","conversation_id":"fake-conversation"}"#
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
        let not_installed = r#"{"error":{"code":"PROVIDER_NOT_FOUND","reason":"PROVIDER_NOT_INSTALLED","message":"Pervue's companion app doesn't support this AI provider yet. Update it, then try again.","retryable":false}}"#;

        let session = run_host(&input);
        assert_eq!(session.result, Ok(()));
        assert_eq!(
            session.frames,
            [
                host_ready(),
                event(
                    r#""req_continue""#,
                    "response.started",
                    r#"{"provider_id":"fake","conversation_id":"conv_1"}"#
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
                // The fake request finished before its cancellation arrived.
                event(
                    r#""req_cancel""#,
                    "response.failed",
                    r#"{"error":{"code":"INVALID_REQUEST","reason":"UNKNOWN_TARGET_REQUEST","message":"The target request is not in flight.","retryable":false}}"#
                ),
            ]
        );
    }

    #[test]
    fn invalid_requests_never_reach_a_provider() {
        // Every validation failure is answered before routing, so no provider
        // work starts for an invalid request (SEC-01).
        let too_deep = format!(
            r#"{{"provider_id":"test","input":{{"text":"hi"}},"extra":{}{}}}"#,
            "[".repeat(200),
            "]".repeat(200)
        );
        let invalid = [
            request("req_syntax", "conversation.send", r#"{"provider_id":"test""#),
            r#"{"version":1,"request_id":"req_envelope","method":"conversation.send","payload":{"provider_id":"test","input":{"text":"hi"}}}"#.to_owned(),
            r#"{"version":1,"type":"request","request_id":"req_extra","method":"conversation.send","payload":{"provider_id":"test","input":{"text":"hi"}},"extra":1}"#.to_owned(),
            r#"{"version":2,"type":"request","request_id":"req_version","method":"conversation.send","payload":{"provider_id":"test","input":{"text":"hi"}}}"#.to_owned(),
            request("req_method", "provider.spawn", "{}"),
            request("req_payload", "conversation.send", r#"{"provider_id":"test","input":{"text":""}}"#),
            request(
                "req_duplicate",
                "conversation.send",
                r#"{"provider_id":"test","provider_id":"test","input":{"text":"hi"}}"#,
            ),
            request("req_deep", "conversation.send", &too_deep),
            request(
                &"a".repeat(crate::limits::MAX_REQUEST_ID_LENGTH + 1),
                "conversation.send",
                r#"{"provider_id":"test","input":{"text":"hi"}}"#,
            ),
            request("req_status", "provider.status", r#"{"provider_id":""}"#),
            request("req_cancel", "request.cancel", r#"{"target_request_id":"bad id"}"#),
        ];
        let valid = request(
            "req_valid",
            "conversation.send",
            r#"{"provider_id":"test","input":{"text":"valid"}}"#,
        );

        let provider = TestProvider::new("test", Script::answers("ok"));
        let calls = Rc::clone(&provider.calls);
        let mut frames: Vec<&str> = invalid.iter().map(String::as_str).collect();
        frames.push(&valid);
        let session = run_session(&with(provider), framed(&frames).as_slice());

        assert_eq!(session.result, Ok(()));
        assert_eq!(*calls.borrow(), ["send:valid"]);
        let events = session.events();
        for failure in &events[1..=invalid.len()] {
            assert_eq!(failure["event"], "response.failed", "{failure}");
            assert_eq!(
                failure["payload"]["error"]["code"], "INVALID_REQUEST",
                "{failure}"
            );
        }
    }

    #[test]
    fn the_provider_learns_whether_context_is_attached() {
        // An adapter whose provider can't use browser context must be able to
        // refuse a request that attaches some.
        let provider = TestProvider::new("test", Script::answers("ok"));
        let calls = Rc::clone(&provider.calls);
        let requests = [
            request(
                "req_plain",
                "conversation.send",
                r#"{"provider_id":"test","input":{"text":"plain"}}"#,
            ),
            request(
                "req_context",
                "conversation.send",
                r#"{"provider_id":"test","input":{"text":"with"},"context":{"mode":"selection","text":"x","truncated":false,"page":{"title":"T","url":"https://example.com/"}}}"#,
            ),
        ];
        let frames: Vec<&str> = requests.iter().map(String::as_str).collect();
        let session = run_session(&with(provider), framed(&frames).as_slice());
        assert_eq!(session.result, Ok(()));
        assert_eq!(*calls.borrow(), ["send:plain", "send:with+context"]);
    }

    #[test]
    fn context_is_rejected_before_an_unsupported_provider_runs() {
        let mut provider = TestProvider::new("test", Script::answers("ok"));
        provider.capabilities.page_context = Capability::Unsupported;
        let calls = Rc::clone(&provider.calls);
        let request = request(
            "req_context",
            "conversation.send",
            r#"{"provider_id":"test","input":{"text":"with"},"context":{"mode":"selection","text":"x","truncated":false,"page":{"title":"T","url":"https://example.com/"}}}"#,
        );
        let session = run_session(&with(provider), framed(&[&request]).as_slice());
        assert_eq!(session.result, Ok(()));
        assert!(
            calls.borrow().is_empty(),
            "provider ran despite unsupported context"
        );
        let failure = &session.events()[1];
        assert_eq!(failure["event"], "response.failed");
        assert_eq!(
            failure["payload"]["error"]["reason"],
            "PAGE_CONTEXT_UNSUPPORTED"
        );
    }

    #[test]
    fn a_model_reaches_a_provider_that_can_switch_models() {
        let mut provider = TestProvider::new("test", Script::answers("ok"));
        provider.capabilities.model_selection = Capability::Supported;
        let calls = Rc::clone(&provider.calls);
        let requests = [
            request(
                "req_default",
                "conversation.send",
                r#"{"provider_id":"test","input":{"text":"plain"}}"#,
            ),
            request(
                "req_model",
                "conversation.send",
                r#"{"provider_id":"test","input":{"text":"chosen"},"model":"sonnet"}"#,
            ),
        ];
        let frames: Vec<&str> = requests.iter().map(String::as_str).collect();
        let session = run_session(&with(provider), framed(&frames).as_slice());
        assert_eq!(session.result, Ok(()));
        assert_eq!(*calls.borrow(), ["send:plain", "send:chosen+model=sonnet"]);
    }

    #[test]
    fn a_model_is_rejected_before_a_provider_that_cannot_switch_runs() {
        let mut provider = TestProvider::new("test", Script::answers("ok"));
        provider.capabilities.model_selection = Capability::Unsupported;
        let calls = Rc::clone(&provider.calls);
        let request = request(
            "req_model",
            "conversation.send",
            r#"{"provider_id":"test","input":{"text":"chosen"},"model":"sonnet"}"#,
        );
        let session = run_session(&with(provider), framed(&[&request]).as_slice());
        assert_eq!(session.result, Ok(()));
        assert!(
            calls.borrow().is_empty(),
            "provider ran with a model it can't take"
        );
        let failure = &session.events()[1];
        assert_eq!(failure["event"], "response.failed");
        assert_eq!(
            failure["payload"]["error"]["reason"],
            "MODEL_SELECTION_UNSUPPORTED"
        );
        assert_eq!(failure["payload"]["error"]["retryable"], false);
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
            for payload in [
                format!(r#"{{"provider_id":{id_json},"input":{{"text":"hi"}}}}"#),
                format!(r#"{{"provider_id":{id_json}}}"#),
            ] {
                let method = if payload.contains("input") {
                    "conversation.send"
                } else {
                    "provider.status"
                };
                let session = run_host(&framed(&[&request("req", method, &payload)]));
                assert_eq!(session.result, Ok(()));
                assert_eq!(session.frames.len(), 2, "{provider_id:?}");
                assert!(
                    session.frames[1].contains(r#""reason":"PROVIDER_NOT_INSTALLED""#),
                    "{provider_id:?}: {}",
                    session.frames[1]
                );
            }
        }
    }

    #[test]
    fn provider_status_without_an_id_reports_every_provider_in_order() {
        let session = run_session(
            &with(TestProvider::new("second", Script::never_starts())),
            framed(&[&request("req_all", "provider.status", "{}")]).as_slice(),
        );
        let events = session.events();
        assert_eq!(
            session.sequence(),
            pairs(&[
                ("req_all", "provider.status"),
                ("req_all", "provider.status"),
                ("req_all", "response.completed"),
            ])
        );
        assert_eq!(events[1]["payload"]["provider_id"], "fake");
        assert_eq!(events[2]["payload"]["provider_id"], "second");
    }

    #[test]
    fn forget_completes_for_a_known_provider_and_fails_for_an_unknown_one() {
        let session = run_session(
            &with(TestProvider::new("second", Script::never_starts())),
            framed(&[
                &request(
                    "req_forget",
                    "conversation.forget",
                    r#"{"provider_id":"fake","conversation_id":"conv_0123456789abcdef"}"#,
                ),
                &request(
                    "req_unknown",
                    "conversation.forget",
                    r#"{"provider_id":"missing","conversation_id":"conv_0123456789abcdef"}"#,
                ),
            ])
            .as_slice(),
        );
        assert_eq!(
            session.sequence(),
            pairs(&[
                ("req_forget", "response.completed"),
                ("req_unknown", "response.failed"),
            ])
        );
        let events = session.events();
        assert_eq!(
            events[2]["payload"]["error"]["reason"],
            "PROVIDER_NOT_INSTALLED"
        );
    }

    #[test]
    fn requests_run_side_by_side_and_a_cancelled_one_ends_first() {
        // `slow` keeps answering while the fake provider completes a request
        // and the extension cancels `slow`.
        let session = run_session(
            &with(TestProvider::new("slow", Script::waits())),
            framed(&[
                &send("req_slow", "slow"),
                &send("req_fast", "fake"),
                &cancel("req_cancel", "req_slow"),
            ])
            .as_slice(),
        );
        assert_eq!(session.result, Ok(()));
        assert_eq!(
            session.sequence(),
            pairs(&[
                ("req_slow", "response.started"),
                ("req_fast", "conversation.created"),
                ("req_fast", "response.started"),
                ("req_fast", "response.delta"),
                ("req_fast", "response.completed"),
                ("req_slow", "response.failed"),
                ("req_cancel", "request.cancelled"),
            ])
        );
        let events = session.events();
        assert_eq!(
            events[6]["payload"]["error"],
            json!({
                "code": "REQUEST_CANCELLED",
                "reason": "USER_CANCELLED",
                "message": "Stopped. You can ask again.",
                "retryable": true
            })
        );
        assert_eq!(
            events[7]["payload"],
            json!({"target_request_id": "req_slow"})
        );
    }

    #[test]
    fn a_request_cancelled_before_it_starts_gets_no_response_started() {
        let session = run_session(
            &with(TestProvider::new("idle", Script::never_starts())),
            framed(&[&send("req_idle", "idle"), &cancel("req_cancel", "req_idle")]).as_slice(),
        );
        assert_eq!(
            session.sequence(),
            pairs(&[
                ("req_idle", "response.failed"),
                ("req_cancel", "request.cancelled"),
            ])
        );
    }

    #[test]
    fn nothing_but_the_terminal_event_follows_a_cancellation() {
        // The provider keeps talking after it was cancelled; none of it is
        // forwarded.
        let session = run_session(
            &with(TestProvider::new(
                "chatty",
                Script {
                    talks_after_cancel: true,
                    ..Script::waits()
                },
            )),
            framed(&[
                &send("req_chatty", "chatty"),
                &cancel("req_cancel", "req_chatty"),
            ])
            .as_slice(),
        );
        assert_eq!(
            session.sequence(),
            pairs(&[
                ("req_chatty", "response.started"),
                ("req_chatty", "response.failed"),
                ("req_cancel", "request.cancelled"),
            ])
        );
    }

    /// A provider whose requests start, then take 100 ms to stop once
    /// cancelled: long enough for more frames to arrive meanwhile.
    fn slow_to_stop() -> Providers {
        with(TestProvider::new(
            "slow",
            Script::waits().stopping_after(Some(Duration::from_millis(100))),
        ))
    }

    #[test]
    fn every_cancellation_of_one_request_is_confirmed() {
        let session = run_session(
            &slow_to_stop(),
            framed(&[
                &send("req_slow", "slow"),
                &cancel("req_cancel_1", "req_slow"),
                &cancel("req_cancel_2", "req_slow"),
            ])
            .as_slice(),
        );
        assert_eq!(
            session.sequence(),
            pairs(&[
                ("req_slow", "response.started"),
                ("req_slow", "response.failed"),
                ("req_cancel_1", "request.cancelled"),
                ("req_cancel_2", "request.cancelled"),
            ])
        );
    }

    #[test]
    fn a_cancellation_without_a_running_target_fails() {
        let session = run_host(&framed(&[
            &cancel("req_nothing", "req_missing"),
            &send("req_done", "fake"),
            &cancel("req_late", "req_done"),
        ]));
        let events = session.events();
        for index in [1, 6] {
            assert_eq!(events[index]["event"], "response.failed");
            assert_eq!(
                events[index]["payload"]["error"]["reason"],
                "UNKNOWN_TARGET_REQUEST"
            );
        }
    }

    #[test]
    fn a_request_id_in_flight_cannot_be_reused() {
        let session = run_session(
            &slow_to_stop(),
            framed(&[
                &send("req_1", "slow"),
                &send("req_1", "fake"),
                &cancel("req_2", "req_1"),
                // A pending cancellation's ID is in flight too.
                &cancel("req_2", "req_1"),
            ])
            .as_slice(),
        );
        assert_eq!(
            session.sequence(),
            pairs(&[
                ("req_1", "response.started"),
                ("req_1", "response.failed"),
                ("req_2", "response.failed"),
                ("req_1", "response.failed"),
                ("req_2", "request.cancelled"),
            ])
        );
        let events = session.events();
        assert_eq!(
            events[2]["payload"]["error"]["reason"],
            "DUPLICATE_REQUEST_ID"
        );
        assert_eq!(
            events[3]["payload"]["error"]["reason"],
            "DUPLICATE_REQUEST_ID"
        );
        assert_eq!(events[4]["payload"]["error"]["reason"], "USER_CANCELLED");
        let rejected: Vec<&Value> = session
            .records
            .iter()
            .filter(|record| record["event"] == "request.rejected")
            .collect();
        assert_eq!(rejected.len(), 2);
        assert_eq!(rejected[0]["method"], "conversation.send");
        assert_eq!(rejected[1]["method"], "request.cancel");
    }

    #[test]
    fn a_request_id_is_free_again_once_its_request_ends() {
        let session = run_host(&framed(&[&send("req_1", "fake"), &send("req_1", "fake")]));
        assert_eq!(
            session
                .sequence()
                .iter()
                .filter(|(_, event)| event == "response.completed")
                .count(),
            2
        );
    }

    #[test]
    fn a_provider_that_never_starts_times_out() {
        let mut provider = TestProvider::new("idle", Script::never_starts());
        provider.timeouts.start = Duration::from_millis(50);
        let session = run_session(
            &with(provider),
            lingering(&[&send("req_idle", "idle")], Duration::from_millis(500)),
        );
        assert_eq!(
            session.sequence(),
            pairs(&[("req_idle", "response.failed")])
        );
        assert_eq!(
            session.events()[1]["payload"]["error"],
            json!({
                "code": "REQUEST_TIMEOUT",
                "reason": "PROVIDER_START_TIMEOUT",
                "message": "The answer took too long to start. Try again.",
                "retryable": true
            })
        );
    }

    #[test]
    fn a_provider_that_goes_quiet_times_out() {
        let mut provider = TestProvider::new("quiet", Script::waits());
        provider.timeouts.idle = Duration::from_millis(50);
        let session = run_session(
            &with(provider),
            lingering(&[&send("req_quiet", "quiet")], Duration::from_millis(500)),
        );
        assert_eq!(
            session.sequence(),
            pairs(&[
                ("req_quiet", "response.started"),
                ("req_quiet", "response.failed"),
            ])
        );
        assert_eq!(
            session.events()[2]["payload"]["error"]["reason"],
            "PROVIDER_RESPONSE_TIMEOUT"
        );
        // A timed-out request can't be cancelled any more.
        assert_eq!(session.records[1]["error"]["code"], "REQUEST_TIMEOUT");
    }

    #[test]
    fn the_end_of_input_stops_requests_still_running() {
        let session = run_session(
            &with(TestProvider::new("slow", Script::waits())),
            framed(&[&send("req_slow", "slow")]).as_slice(),
        );
        assert_eq!(session.result, Ok(()));
        assert_eq!(
            session.sequence(),
            pairs(&[
                ("req_slow", "response.started"),
                ("req_slow", "response.failed"),
            ])
        );
        assert_eq!(
            session.events()[2]["payload"]["error"]["reason"],
            "INPUT_CLOSED"
        );
    }

    #[test]
    fn an_exchange_that_stops_unasked_is_an_internal_error() {
        let session = run_session(
            &with(TestProvider::new(
                "odd",
                Script {
                    stops_unasked: true,
                    ..Script::waits()
                },
            )),
            framed(&[&send("req_odd", "odd")]).as_slice(),
        );
        assert_eq!(
            session.events()[2]["payload"]["error"]["code"],
            "INTERNAL_ERROR"
        );
    }

    #[test]
    fn a_long_delta_is_split_into_frames_that_fit() {
        // Control characters escape to six bytes each: the worst case.
        const TEXT: &str = "\u{1}\u{2}";
        let text: &'static str = Box::leak(TEXT.repeat(MAX_FRAME_SIZE / 4).into_boxed_str());
        let session = run_session(
            &with(TestProvider::new("long", Script::answers(text))),
            framed(&[&send("req_long", "long")]).as_slice(),
        );
        assert_eq!(session.result, Ok(()));
        let events = session.events();
        let deltas: Vec<&Value> = events
            .iter()
            .filter(|event| event["event"] == "response.delta")
            .collect();
        assert!(deltas.len() > 1);
        assert!(
            session
                .frames
                .iter()
                .all(|frame| frame.len() <= MAX_FRAME_SIZE)
        );
        let joined: String = deltas
            .iter()
            .map(|delta| delta["payload"]["text"].as_str().unwrap())
            .collect();
        assert!(joined == text, "the deltas differ from the text");
    }

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

    /// Runs the host into an output that breaks after `host.ready` and one
    /// more frame of `extra` bytes.
    fn run_until_stdout_closes(
        providers: &Providers,
        requests: &[&str],
        extra: usize,
    ) -> Vec<Value> {
        let mut ready = Vec::new();
        events::write_host_ready(&mut ready).unwrap();
        let mut output = BreakingPipe {
            capacity: ready.len() + extra,
        };
        let mut log = Diagnostics::new(Vec::new());
        let result = run_with(
            providers,
            &mut framed(requests).as_slice(),
            &mut output,
            &mut log,
        );
        assert_eq!(result, Err(HostError::Io));
        let records: Vec<Value> = String::from_utf8(log.into_inner())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let stopped = records.last().unwrap();
        assert_eq!(stopped["reason"], "io_error");
        assert_eq!(stopped["exit_code"], 2);
        records
    }

    #[test]
    fn a_request_the_host_stops_answering_is_still_recorded() {
        let records = run_until_stdout_closes(
            &Providers::scaffold(),
            &[&request(
                &rid("cut"),
                "conversation.send",
                r#"{"provider_id":"fake","conversation_id":"conv_1","input":{"text":"hi"}}"#,
            )],
            0,
        );
        let events: Vec<&str> = records
            .iter()
            .map(|r| r["event"].as_str().unwrap())
            .collect();
        assert_eq!(events, ["host.started", "request.aborted", "host.stopped"]);
        let aborted = &records[1];
        assert_eq!(aborted["request_id"], rid("cut"));
        assert_eq!(aborted["method"], "conversation.send");
        assert_eq!(aborted["provider_id"], "fake");
        // Named by the request, not created by this host.
        assert_eq!(aborted["conversation_id"], diagnostics::REDACTED);
        assert_eq!(aborted["reason"], "io_error");
        assert!(aborted["duration_ms"].is_u64());
        assert!(aborted.get("error").is_none());
        assert_eq!(
            (&records[2]["requests"], &records[2]["rejected"]),
            (&1.into(), &0.into())
        );

        let records = run_until_stdout_closes(
            &Providers::scaffold(),
            &[
                r#"{"version":1,"type":"request","request_id":"req_bad","method":"provider.spawn","payload":{}}"#,
            ],
            0,
        );
        let events: Vec<&str> = records
            .iter()
            .map(|r| r["event"].as_str().unwrap())
            .collect();
        assert_eq!(events, ["host.started", "request.rejected", "host.stopped"]);
        assert_eq!(
            (&records[2]["requests"], &records[2]["rejected"]),
            (&0.into(), &1.into())
        );
    }

    #[test]
    fn an_aborted_request_records_the_conversation_its_provider_created() {
        // stdout closes on the request's first event, `conversation.created`.
        // The provider has created the conversation, so the record names it.
        let records =
            run_until_stdout_closes(&Providers::scaffold(), &[&send(&rid("new"), "fake")], 0);
        let aborted = &records[1];
        assert_eq!(aborted["event"], "request.aborted");
        assert_eq!(aborted["request_id"], rid("new"));
        assert_eq!(aborted["conversation_id"], fake::CONVERSATION_ID);
    }

    #[test]
    fn requests_still_running_when_stdout_closes_are_recorded_as_aborted() {
        // `slow` is being cancelled when the fake request's first event finds
        // stdout closed: all three are recorded as aborted.
        let providers = with(TestProvider::new(
            "slow",
            Script::never_starts().stopping_after(Some(Duration::from_secs(60))),
        ));
        let records = run_until_stdout_closes(
            &providers,
            &[
                &send(&rid("slow"), "slow"),
                &cancel(&rid("cancel"), &rid("slow")),
                &send(&rid("fast"), "fake"),
            ],
            0,
        );
        let mut aborted: Vec<(&str, &str)> = records
            .iter()
            .filter(|record| record["event"] == "request.aborted")
            .map(|record| {
                (
                    record["request_id"].as_str().unwrap(),
                    record["method"].as_str().unwrap(),
                )
            })
            .collect();
        aborted.sort_unstable();
        let mut expected = [
            (rid("cancel"), "request.cancel"),
            (rid("fast"), "conversation.send"),
            (rid("slow"), "conversation.send"),
        ];
        expected.sort_unstable();
        let expected: Vec<(&str, &str)> = expected
            .iter()
            .map(|(id, method)| (id.as_str(), *method))
            .collect();
        assert_eq!(aborted, expected);
        assert!(records.iter().all(|record| record.get("error").is_none()));
    }

    #[test]
    fn a_request_that_never_stops_is_ended_anyway() {
        // The adapter ignores its cancel entirely; the host ends the request
        // once the grace period and the slack have passed.
        let providers = with(TestProvider::new(
            "stuck",
            Script::waits().stopping_after(None),
        ));
        let started = Instant::now();
        let session = run_session(
            &providers,
            framed(&[
                &send("req_stuck", "stuck"),
                &cancel("req_cancel", "req_stuck"),
            ])
            .as_slice(),
        );
        assert!(started.elapsed() >= STOP_SLACK);
        assert_eq!(
            session.sequence(),
            pairs(&[
                ("req_stuck", "response.started"),
                ("req_stuck", "response.failed"),
                ("req_cancel", "request.cancelled"),
            ])
        );
        assert_eq!(
            session.events()[2]["payload"]["error"]["reason"],
            "USER_CANCELLED"
        );
    }

    #[test]
    fn diagnostics_record_the_session_lifecycle() {
        // The request ID is written with an escape; the record holds it decoded.
        let ok = rid("ok");
        let escaped_id = format!("r{}u0065{}", '\\', &ok[2..]);
        let input = framed(&[
            &request(&rid("bad"), "provider.spawn", "{}"),
            "{not json",
            &format!(
                r#"{{"version":1,"type":"request","request_id":"{escaped_id}","method":"conversation.send","payload":{{"provider_id":"fake","conversation_id":"conv_1","input":{{"text":"hi"}}}}}}"#
            ),
            &request(
                &rid("status"),
                "provider.status",
                r#"{"provider_id":"codex"}"#,
            ),
            &cancel(&rid("cancel"), &ok),
        ]);
        let session = run_host(&input);
        assert_eq!(session.result, Ok(()));
        let records = &session.records;

        assert_eq!(
            session.record_events(),
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
        for record in records {
            let ts = record["ts"].as_str().unwrap();
            assert_eq!(ts.len(), "2025-09-25T01:23:45.678Z".len(), "{ts}");
            assert!(ts.ends_with('Z') && ts.as_bytes()[10] == b'T', "{ts}");
        }

        assert_eq!(records[0]["host_version"], HOST_VERSION);
        assert_eq!(records[0]["pid"], std::process::id());

        assert_eq!(records[1]["request_id"], rid("bad"));
        assert_eq!(
            records[1]["error"],
            json!({"code": "INVALID_REQUEST", "reason": "UNKNOWN_METHOD"})
        );
        assert!(records[2].get("request_id").is_none());
        assert_eq!(records[2]["error"]["reason"], "MALFORMED_MESSAGE");

        assert_eq!(records[3]["request_id"], ok);
        assert_eq!(records[3]["method"], "conversation.send");
        assert_eq!(records[3]["provider_id"], "fake");
        assert_eq!(records[3]["conversation_id"], diagnostics::REDACTED);
        assert!(records[3]["duration_ms"].is_u64());
        assert!(records[3].get("error").is_none());

        assert_eq!(records[4]["request_id"], rid("status"));
        assert_eq!(records[4]["method"], "provider.status");
        // This host serves the fake provider alone.
        assert_eq!(records[4]["provider_id"], diagnostics::REDACTED);
        assert_eq!(
            records[4]["error"],
            json!({"code": "PROVIDER_NOT_FOUND", "reason": "PROVIDER_NOT_INSTALLED"})
        );
        assert_eq!(records[5]["request_id"], rid("cancel"));
        assert_eq!(records[5]["method"], "request.cancel");
        assert_eq!(records[5]["target_request_id"], ok);
        assert_eq!(records[5]["error"]["reason"], "UNKNOWN_TARGET_REQUEST");

        let stopped = &records[6];
        assert_eq!(stopped["reason"], "end_of_input");
        assert_eq!(stopped["exit_code"], 0);
        assert_eq!(stopped["requests"], 3);
        assert_eq!(stopped["rejected"], 2);
        assert!(stopped["duration_ms"].is_u64());
    }

    #[test]
    fn cancellations_are_recorded_with_their_target() {
        let session = run_session(
            &with(TestProvider::new("slow", Script::waits())),
            framed(&[
                &send(&rid("slow"), "slow"),
                &cancel(&rid("cancel"), &rid("slow")),
            ])
            .as_slice(),
        );
        let records = &session.records;
        assert_eq!(records[1]["request_id"], rid("slow"));
        assert_eq!(records[1]["event"], "request.failed");
        assert_eq!(
            records[1]["error"],
            json!({"code": "REQUEST_CANCELLED", "reason": "USER_CANCELLED"})
        );
        assert_eq!(records[2]["request_id"], rid("cancel"));
        assert_eq!(records[2]["event"], "request.completed");
        assert_eq!(records[2]["target_request_id"], rid("slow"));
    }

    #[test]
    fn request_records_agree_with_each_terminal_event() {
        // Across every path, a request is recorded as failed exactly when its
        // last frame is response.failed, with the same code and reason. Its
        // conversation is the one conversation.created announced, or else the
        // one the request continued, which only this host could have created.
        let requests = [
            send(&rid("send_new"), "fake"),
            request(
                &rid("send_existing"),
                "conversation.send",
                r#"{"provider_id":"fake","conversation_id":"c1","input":{"text":"hi"}}"#,
            ),
            send(&rid("send_unknown"), "codex"),
            request(&rid("status_all"), "provider.status", "{}"),
            request(
                &rid("status_fake"),
                "provider.status",
                r#"{"provider_id":"fake"}"#,
            ),
            request(
                &rid("status_unknown"),
                "provider.status",
                r#"{"provider_id":"codex"}"#,
            ),
            cancel(&rid("cancel"), &rid("send_new")),
        ];
        let frames: Vec<&str> = requests.iter().map(String::as_str).collect();
        let session = run_host(&framed(&frames));
        assert_eq!(session.result, Ok(()));

        let mut conversations = HashMap::new();
        for request in &requests {
            let request: Value = serde_json::from_str(request).unwrap();
            let conversation_id = request["payload"]
                .get("conversation_id")
                .map(|_| Value::from(diagnostics::REDACTED));
            conversations.insert(
                request["request_id"].as_str().unwrap().to_owned(),
                conversation_id,
            );
        }
        let mut last_frames = HashMap::new();
        for frame in &session.events()[1..] {
            let request_id = frame["request_id"].as_str().unwrap().to_owned();
            if frame["event"] == "conversation.created" {
                let created = frame["payload"]["conversation_id"].clone();
                conversations.insert(request_id.clone(), Some(created));
            }
            last_frames.insert(request_id, frame.clone());
        }
        let request_records = &session.records[1..session.records.len() - 1];
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
        assert_eq!(request_records[1]["conversation_id"], diagnostics::REDACTED);
    }

    #[test]
    fn diagnostics_record_why_the_host_stopped() {
        let oversized = u32::try_from(MAX_FRAME_SIZE + 1).unwrap().to_ne_bytes();
        for (input, reason, exit_code) in [
            (&oversized[..], "frame_too_large", 4),
            (&[0x01, 0x00][..], "frame_truncated", 3),
        ] {
            let session = run_host(input);
            let stopped = session.records.last().unwrap();
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
                r#"{{"version":1,"type":"request","request_id":"req_1","method":"conversation.send","payload":{{"provider_id":"fake","input":{{"text":"{MARKER}"}},"context":{{"mode":"page","text":"{MARKER}","truncated":false,"page":{{"title":"{MARKER}","url":"https://example.com/"}}}},"extra":"{MARKER}"}}}}"#
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
        let session = run_host(&input);
        assert_eq!(session.result, Ok(()));
        assert_eq!(session.records.len(), 8);
        for record in &session.records {
            assert!(!record.to_string().contains(MARKER), "{record}");
        }
    }

    #[test]
    fn identifiers_pervue_did_not_issue_are_redacted_in_diagnostics() {
        // SEC-02: a record keeps a request ID only in the extension's shape, a
        // provider only if this host serves it, and a conversation only if
        // this host created it. Anything else, such as a secret where an ID
        // belongs, recognizable or not, never reaches the diagnostics.
        const KEYS: [&str; 3] = [
            "sk-proj-SECRETabcd1234",
            "ya29.SECRETa0AfH6SMBx3dEFG",
            "SECRET",
        ];
        let mut frames = Vec::new();
        for key in KEYS {
            frames.extend([
                send(key, "fake"),
                send(&rid("provider"), key),
                request(
                    &rid("conversation"),
                    "conversation.send",
                    &format!(
                        r#"{{"provider_id":"fake","conversation_id":"{key}","input":{{"text":"hi"}}}}"#
                    ),
                ),
                cancel(&rid("cancel"), key),
                request(key, "no.such.method", "{}"),
            ]);
        }
        // What this host issued is kept: its provider, and a conversation it
        // created, continued.
        frames.extend([
            send(&rid("new"), "fake"),
            request(
                &rid("continued"),
                "conversation.send",
                &format!(
                    r#"{{"provider_id":"fake","conversation_id":"{}","input":{{"text":"hi"}}}}"#,
                    fake::CONVERSATION_ID
                ),
            ),
        ]);
        let frames: Vec<&str> = frames.iter().map(String::as_str).collect();
        let session = run_host(&framed(&frames));
        assert_eq!(session.result, Ok(()));
        let records: Vec<String> = session.records.iter().map(Value::to_string).collect();
        assert!(
            records.iter().all(|record| !record.contains("SECRET")),
            "{records:?}"
        );
        for field in [
            "request_id",
            "provider_id",
            "conversation_id",
            "target_request_id",
        ] {
            assert!(
                session
                    .records
                    .iter()
                    .any(|record| record[field] == diagnostics::REDACTED),
                "{field} was never redacted: {records:?}"
            );
        }
        let continued = session
            .records
            .iter()
            .find(|record| record["request_id"] == rid("continued"))
            .unwrap();
        assert_eq!(continued["provider_id"], "fake");
        assert_eq!(continued["conversation_id"], fake::CONVERSATION_ID);
        // The frames still echo the request IDs: they go to the extension,
        // which sent them.
        assert!(session.frames.iter().any(|frame| frame.contains(KEYS[0])));
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
            &send("req_1", "fake"),
            "{not json",
            &request("req_2", "provider.status", "{}"),
            &cancel("req_3", "req_1"),
        ]);
        let mut with_log = Vec::new();
        let mut log = Diagnostics::new(Vec::new());
        run_with(
            &Providers::scaffold(),
            &mut input.as_slice(),
            &mut with_log,
            &mut log,
        )
        .unwrap();

        let mut without_log = Vec::new();
        let mut broken = Diagnostics::new(Broken);
        run_with(
            &Providers::scaffold(),
            &mut input.as_slice(),
            &mut without_log,
            &mut broken,
        )
        .unwrap();

        assert_eq!(with_log, without_log);
        assert!(!log.into_inner().is_empty());
    }
}
