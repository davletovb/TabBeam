//! Protocol-v1 request validation (`docs/protocol/v1.md` §1, §2, §5, §8).

use std::borrow::Cow;

use super::PROTOCOL_VERSION;
use super::json::{JsonError, JsonStr, Reader};
use crate::conversation::{BrowserContext, BrowserContextMode, HistoryMessage, is_javascript_trim_char};
use crate::limits::{
    MAX_CONTEXT_TITLE_BYTES, MAX_CONTEXT_URL_BYTES, MAX_HISTORY_BYTES, MAX_HISTORY_MESSAGES,
    MAX_PAGE_BYTES, MAX_REQUEST_ID_LENGTH, MAX_SELECTION_BYTES,
};

/// A validated request ID, kept as the raw bytes of its JSON string token so
/// events echo it byte-for-byte (v1 §4).
///
/// Re-emitting the raw bytes is safe: validation limits the decoded ID to
/// `[A-Za-z0-9][A-Za-z0-9._:-]*`, so the raw form holds only those characters
/// and `\uXXXX` escapes of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestId<'a>(JsonStr<'a>);

impl<'a> RequestId<'a> {
    /// The ID exactly as it appeared between the quotes of the request.
    pub fn raw(self) -> &'a [u8] {
        self.0.raw()
    }

    /// The ID with any escapes resolved. Validation limits it to
    /// `[A-Za-z0-9][A-Za-z0-9._:-]*`.
    pub fn decode(self) -> Cow<'a, str> {
        self.0.decode()
    }

    fn parse(token: JsonStr<'a>) -> Option<Self> {
        is_valid_request_id(token).then_some(Self(token))
    }
}

/// A validated request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request<'a> {
    pub request_id: RequestId<'a>,
    pub method: Method<'a>,
}

/// A v1 method and its validated payload fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method<'a> {
    ConversationSend {
        provider_id: JsonStr<'a>,
        conversation_id: Option<JsonStr<'a>>,
        /// `input.text`: the user's question.
        text: JsonStr<'a>,
        /// Parsed and validated once at the protocol boundary.
        history: Vec<HistoryMessage>,
        /// Validated browser context attached to this turn.
        context: Option<BrowserContext>,
    },
    ProviderStatus {
        provider_id: Option<JsonStr<'a>>,
    },
    RequestCancel {
        target_request_id: RequestId<'a>,
    },
}

impl Method<'_> {
    /// The method's protocol name.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::ConversationSend { .. } => "conversation.send",
            Self::ProviderStatus { .. } => "provider.status",
            Self::RequestCancel { .. } => "request.cancel",
        }
    }
}

/// Why a request was rejected. Each kind maps to one `INVALID_REQUEST` reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Malformed,
    InvalidEnvelope,
    InvalidPayload,
    UnknownMethod,
    UnsupportedVersion,
}

impl FailureKind {
    /// The `INVALID_REQUEST` reason reported for this kind (DOC-02).
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Malformed => "MALFORMED_MESSAGE",
            Self::InvalidEnvelope => "INVALID_ENVELOPE",
            Self::InvalidPayload => "INVALID_PAYLOAD",
            Self::UnknownMethod => "UNKNOWN_METHOD",
            Self::UnsupportedVersion => "UNSUPPORTED_PROTOCOL_VERSION",
        }
    }
}

/// A rejected request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestFailure<'a> {
    pub kind: FailureKind,
    /// The request ID recovered before the failure was detected (v1 §8.4).
    pub request_id: Option<RequestId<'a>>,
    /// The integer version received, for [`FailureKind::UnsupportedVersion`].
    pub received_version: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MethodName {
    ConversationSend,
    ProviderStatus,
    RequestCancel,
}

/// Envelope facts gathered while walking the top-level members in order.
#[derive(Default)]
struct Envelope<'a> {
    request_id: Option<RequestId<'a>>,
    version: Option<i64>,
    method: Option<MethodName>,
    payload: Option<&'a [u8]>,
    saw_version: bool,
    saw_type: bool,
    saw_request_id: bool,
    saw_method: bool,
    saw_payload: bool,
    invalid: bool,
}

/// Validates one request frame.
pub fn parse_request(data: &[u8]) -> Result<Request<'_>, RequestFailure<'_>> {
    let mut envelope = Envelope::default();
    let fail = |kind, request_id| RequestFailure {
        kind,
        request_id,
        received_version: None,
    };

    if let Err(kind) = parse_envelope(data, &mut envelope) {
        return Err(fail(kind, envelope.request_id));
    }

    let request_id = envelope.request_id;
    let valid_envelope = envelope.saw_version
        && envelope.saw_type
        && envelope.saw_request_id
        && envelope.saw_method
        && envelope.saw_payload
        && !envelope.invalid;
    // A valid envelope always carries a request ID and a payload object.
    let (true, Some(id), Some(payload)) = (valid_envelope, request_id, envelope.payload) else {
        return Err(fail(FailureKind::InvalidEnvelope, request_id));
    };

    if envelope.version != Some(PROTOCOL_VERSION) {
        return Err(RequestFailure {
            kind: FailureKind::UnsupportedVersion,
            request_id,
            received_version: envelope.version,
        });
    }

    let Some(method) = envelope.method else {
        return Err(fail(FailureKind::UnknownMethod, request_id));
    };

    // v1 §1 rule 9: no object in a method payload may repeat a member name,
    // including members this host does not interpret. The payload's syntax is
    // already validated, so this pass can only find duplicates.
    Reader::rejecting_duplicate_members(payload)
        .skip_value()
        .map_err(|error| fail(payload_error(error), request_id))?;

    let method = match method {
        MethodName::ConversationSend => parse_conversation_payload(payload),
        MethodName::ProviderStatus => parse_status_payload(payload),
        MethodName::RequestCancel => parse_cancel_payload(payload),
    }
    .map_err(|kind| fail(kind, request_id))?;

    Ok(Request {
        request_id: id,
        method,
    })
}

fn parse_envelope<'a>(data: &'a [u8], envelope: &mut Envelope<'a>) -> Result<(), FailureKind> {
    let malformed = |_| FailureKind::Malformed;
    let mut reader = Reader::new(data);
    reader.skip_whitespace();

    if reader.peek() != Some(b'{') {
        // Well-formed JSON that is not an object is an invalid envelope.
        reader.skip_value().map_err(malformed)?;
        reader.skip_whitespace();
        return Err(if reader.at_end() {
            FailureKind::InvalidEnvelope
        } else {
            FailureKind::Malformed
        });
    }

    reader.consume(b'{');
    reader.skip_whitespace();
    if reader.consume(b'}') {
        reader.skip_whitespace();
        return Err(if reader.at_end() {
            FailureKind::InvalidEnvelope
        } else {
            FailureKind::Malformed
        });
    }

    loop {
        let key = reader.parse_string().map_err(malformed)?;
        reader.skip_whitespace();
        if !reader.consume(b':') {
            return Err(FailureKind::Malformed);
        }
        reader.skip_whitespace();

        if key.equals_ascii("version") {
            envelope.invalid |= envelope.saw_version;
            envelope.saw_version = true;
            if matches!(reader.peek(), Some(b'-' | b'0'..=b'9')) {
                // v1 §1 rule 10: only the integer token `1` is version 1.
                envelope.version = reader.parse_integer().map_err(malformed)?;
                envelope.invalid |= envelope.version.is_none();
            } else {
                reader.skip_value().map_err(malformed)?;
                envelope.invalid = true;
            }
        } else if key.equals_ascii("type") {
            envelope.invalid |= envelope.saw_type;
            envelope.saw_type = true;
            match read_string(&mut reader).map_err(malformed)? {
                Some(value) if value.equals_ascii("request") => {}
                _ => envelope.invalid = true,
            }
        } else if key.equals_ascii("request_id") {
            envelope.invalid |= envelope.saw_request_id;
            envelope.saw_request_id = true;
            match read_string(&mut reader)
                .map_err(malformed)?
                .and_then(RequestId::parse)
            {
                Some(request_id) => envelope.request_id = Some(request_id),
                None => envelope.invalid = true,
            }
        } else if key.equals_ascii("method") {
            envelope.invalid |= envelope.saw_method;
            envelope.saw_method = true;
            match read_string(&mut reader).map_err(malformed)? {
                Some(value) => {
                    if let Some(method) = method_name(value) {
                        envelope.method = Some(method);
                    }
                }
                None => envelope.invalid = true,
            }
        } else if key.equals_ascii("payload") {
            envelope.invalid |= envelope.saw_payload;
            envelope.saw_payload = true;
            if reader.peek() == Some(b'{') {
                // v1 §1 rule 11: depth overflow inside the payload is a payload error.
                let start = reader.position();
                reader.skip_value().map_err(payload_error)?;
                envelope.payload = Some(&data[start..reader.position()]);
            } else {
                reader.skip_value().map_err(malformed)?;
                envelope.invalid = true;
            }
        } else {
            envelope.invalid = true;
            reader.skip_value().map_err(malformed)?;
        }

        reader.skip_whitespace();
        if reader.consume(b'}') {
            break;
        }
        if !reader.consume(b',') {
            return Err(FailureKind::Malformed);
        }
        reader.skip_whitespace();
    }

    reader.skip_whitespace();
    if reader.at_end() {
        Ok(())
    } else {
        Err(FailureKind::Malformed)
    }
}

fn method_name(value: JsonStr<'_>) -> Option<MethodName> {
    [
        ("conversation.send", MethodName::ConversationSend),
        ("provider.status", MethodName::ProviderStatus),
        ("request.cancel", MethodName::RequestCancel),
    ]
    .into_iter()
    .find_map(|(name, method)| value.equals_ascii(name).then_some(method))
}

fn parse_conversation_payload(payload: &[u8]) -> Result<Method<'_>, FailureKind> {
    let mut provider_id = None;
    let mut conversation_id = None;
    let mut input_data = None;
    let mut context = None;
    let mut invalid = false;

    let has_members = walk_payload_object(payload, |key, reader| {
        if key.equals_ascii("provider_id") {
            provider_id = read_nonempty_string(reader)?;
            invalid |= provider_id.is_none();
        } else if key.equals_ascii("input") {
            input_data = read_object(reader)?.and_then(|input| parse_input(input).ok());
            invalid |= input_data.is_none();
        } else if key.equals_ascii("conversation_id") {
            conversation_id = read_nonempty_string(reader)?;
            invalid |= conversation_id.is_none();
        } else if key.equals_ascii("context") {
            context = read_object(reader)?.and_then(|value| parse_browser_context(value).ok());
            invalid |= context.is_none();
        } else {
            reader.skip_value().map_err(payload_error)?;
        }
        Ok(())
    })?;

    match (provider_id, input_data) {
        (Some(provider_id), Some((text, history))) if has_members && !invalid => {
            Ok(Method::ConversationSend {
                provider_id,
                conversation_id,
                text,
                history,
                context,
            })
        }
        _ => Err(FailureKind::InvalidPayload),
    }
}

/// Validates `input` and returns its question and bounded dialogue history.
fn parse_input(input: &[u8]) -> Result<(JsonStr<'_>, Vec<HistoryMessage>), FailureKind> {
    let mut text = None;
    let mut history = None;
    let mut invalid = false;

    let has_members = walk_payload_object(input, |key, reader| {
        if key.equals_ascii("text") {
            text = read_nonempty_string(reader)?;
            invalid |= text.is_none();
        } else if key.equals_ascii("history") {
            let start = reader.position();
            reader.skip_value().map_err(payload_error)?;
            let parsed = serde_json::from_slice::<Vec<HistoryMessage>>(reader.span_from(start));
            invalid |= !matches!(&parsed, Ok(messages)
                if messages.len() <= MAX_HISTORY_MESSAGES
                    && messages.iter().all(|message| !message.text.is_empty())
                    && messages.iter().map(|message| message.text.len()).sum::<usize>() <= MAX_HISTORY_BYTES);
            history = parsed.ok();
        } else {
            reader.skip_value().map_err(payload_error)?;
        }
        Ok(())
    })?;

    match text {
        Some(text) if has_members && !invalid => Ok((text, history.unwrap_or_default())),
        _ => Err(FailureKind::InvalidPayload),
    }
}

fn parse_browser_context(context: &[u8]) -> Result<BrowserContext, FailureKind> {
    // Duplicate members were already rejected by the strict payload pass.
    // Typed deserialization skips forward-compatible unknown fields without
    // allocating an arbitrary serde_json::Value tree.
    let context: BrowserContext =
        serde_json::from_slice(context).map_err(|_| FailureKind::InvalidPayload)?;

    if context
        .text
        .trim_matches(is_javascript_trim_char)
        .is_empty()
    {
        return Err(FailureKind::InvalidPayload);
    }
    let text_limit = match context.mode {
        BrowserContextMode::Selection => MAX_SELECTION_BYTES,
        BrowserContextMode::Page => MAX_PAGE_BYTES,
    };
    if context.text.len() > text_limit
        || context.page.title.len() > MAX_CONTEXT_TITLE_BYTES
        || context.page.url.len() > MAX_CONTEXT_URL_BYTES
        || !valid_context_url(&context.page.url)
    {
        return Err(FailureKind::InvalidPayload);
    }

    Ok(context)
}

fn valid_context_url(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    if !matches!(scheme, "http" | "https") || rest.is_empty() {
        return false;
    }
    let authority = rest.split('/').next().unwrap_or_default();
    // The extension strips credentials, query, and fragment before sending.
    !authority.is_empty()
        && !authority.contains('@')
        && !url.contains('?')
        && !url.contains('#')
        && !url.chars().any(char::is_whitespace)
}

fn parse_status_payload(payload: &[u8]) -> Result<Method<'_>, FailureKind> {
    let mut provider_id = None;
    let mut invalid = false;

    walk_payload_object(payload, |key, reader| {
        if key.equals_ascii("provider_id") {
            provider_id = read_nonempty_string(reader)?;
            invalid |= provider_id.is_none();
        } else {
            reader.skip_value().map_err(payload_error)?;
        }
        Ok(())
    })?;

    if invalid {
        Err(FailureKind::InvalidPayload)
    } else {
        Ok(Method::ProviderStatus { provider_id })
    }
}

fn parse_cancel_payload(payload: &[u8]) -> Result<Method<'_>, FailureKind> {
    let mut target_request_id = None;
    let mut invalid = false;

    walk_payload_object(payload, |key, reader| {
        if key.equals_ascii("target_request_id") {
            target_request_id = read_string(reader)
                .map_err(payload_error)?
                .and_then(RequestId::parse);
            invalid |= target_request_id.is_none();
        } else {
            reader.skip_value().map_err(payload_error)?;
        }
        Ok(())
    })?;

    match target_request_id {
        Some(target_request_id) if !invalid => Ok(Method::RequestCancel { target_request_id }),
        _ => Err(FailureKind::InvalidPayload),
    }
}

/// Walks the members of a method-payload object, handing each key to `visit`
/// with the reader positioned at the member's value, which `visit` must
/// consume. Returns whether the object had any members. Member names are
/// unique: `parse_request` rejects payloads with duplicates before walking.
fn walk_payload_object<'a>(
    object: &'a [u8],
    mut visit: impl FnMut(JsonStr<'a>, &mut Reader<'a>) -> Result<(), FailureKind>,
) -> Result<bool, FailureKind> {
    let mut reader = Reader::new(object);
    reader.skip_whitespace();
    if !reader.consume(b'{') {
        return Err(FailureKind::InvalidPayload);
    }
    reader.skip_whitespace();
    if reader.consume(b'}') {
        return Ok(false);
    }

    loop {
        let key = reader.parse_string().map_err(payload_error)?;
        reader.skip_whitespace();
        if !reader.consume(b':') {
            return Err(FailureKind::Malformed);
        }
        reader.skip_whitespace();
        visit(key, &mut reader)?;
        reader.skip_whitespace();
        if reader.consume(b'}') {
            break;
        }
        if !reader.consume(b',') {
            return Err(FailureKind::Malformed);
        }
        reader.skip_whitespace();
    }

    reader.skip_whitespace();
    if reader.at_end() {
        Ok(true)
    } else {
        Err(FailureKind::Malformed)
    }
}

fn payload_error(error: JsonError) -> FailureKind {
    match error {
        JsonError::DepthExceeded | JsonError::DuplicateMember => FailureKind::InvalidPayload,
        JsonError::Syntax => FailureKind::Malformed,
    }
}

/// Reads a string value, or skips a value of any other type and returns `None`.
fn read_string<'a>(reader: &mut Reader<'a>) -> Result<Option<JsonStr<'a>>, JsonError> {
    if reader.peek() == Some(b'"') {
        reader.parse_string().map(Some)
    } else {
        reader.skip_value().map(|()| None)
    }
}

/// Like [`read_string`], but also returns `None` for an empty string.
fn read_nonempty_string<'a>(reader: &mut Reader<'a>) -> Result<Option<JsonStr<'a>>, FailureKind> {
    let value = read_string(reader).map_err(payload_error)?;
    Ok(value.filter(|value| !value.is_empty()))
}

/// Returns the bytes of an object value, or skips a value of any other type
/// and returns `None`.
fn read_object<'a>(reader: &mut Reader<'a>) -> Result<Option<&'a [u8]>, FailureKind> {
    let start = reader.position();
    let is_object = reader.peek() == Some(b'{');
    reader.skip_value().map_err(payload_error)?;
    Ok(is_object.then(|| reader.span_from(start)))
}

fn is_valid_request_id(token: JsonStr<'_>) -> bool {
    let mut position = 0;
    let mut length = 0;
    while let Some(decoded) = token.next_ascii(&mut position) {
        length += 1;
        let allowed = match decoded {
            Some(c) if length == 1 => c.is_ascii_alphanumeric(),
            Some(c) => c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b':' | b'-'),
            None => false,
        };
        if !allowed || length > MAX_REQUEST_ID_LENGTH {
            return false;
        }
    }
    length > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_CONVERSATION: &str = r#"{"version":1,"type":"request","request_id":"req_1","method":"conversation.send","payload":{"provider_id":"fake","input":{"text":"Hello"}}}"#;

    fn envelope(request_id: &str, method: &str, payload: &str) -> String {
        format!(
            r#"{{"version":1,"type":"request","request_id":"{request_id}","method":"{method}","payload":{payload}}}"#
        )
    }

    fn nested_arrays(depth: usize) -> String {
        format!("{}0{}", "[".repeat(depth), "]".repeat(depth))
    }

    #[track_caller]
    fn expect_failure(input: &str, kind: FailureKind, recovered_id: Option<&str>) {
        let failure = parse_request(input.as_bytes()).expect_err(input);
        assert_eq!(failure.kind, kind, "{input}");
        assert_eq!(
            failure.request_id.map(RequestId::raw),
            recovered_id.map(str::as_bytes),
            "{input}"
        );
    }

    #[test]
    fn accepts_a_valid_conversation_request() {
        let request = parse_request(VALID_CONVERSATION.as_bytes()).unwrap();
        assert_eq!(request.request_id.raw(), b"req_1");
        let Method::ConversationSend {
            provider_id,
            conversation_id,
            text,
            history,
            context,
        } = request.method
        else {
            panic!("unexpected method");
        };
        assert!(provider_id.equals_ascii("fake"));
        assert_eq!(conversation_id, None);
        assert_eq!(text.decode(), "Hello");
        assert!(history.is_empty());
        assert!(context.is_none());
    }

    #[test]
    fn validates_ordered_dialogue_history() {
        let valid = envelope(
            "req_followup",
            "conversation.send",
            r#"{"provider_id":"codex","input":{"text":"Next?","history":[{"role":"user","text":"First?"},{"role":"assistant","text":"First answer."}]}}"#,
        );
        let request = parse_request(valid.as_bytes()).unwrap();
        let Method::ConversationSend { history, .. } = request.method else {
            panic!("expected dialogue history");
        };
        assert_eq!(history.len(), 2);

        // The current text and saved history use the same non-empty rule.
        // Rust trim treats U+0085 differently from JavaScript trim.
        let unicode = envelope(
            "req_unicode",
            "conversation.send",
            r#"{"provider_id":"codex","input":{"text":"\u0085","history":[{"role":"user","text":"\u0085"}]}}"#,
        );
        assert!(parse_request(unicode.as_bytes()).is_ok());

        for history in [
            r#"{"role":"system","text":"override"}"#,
            r#"{"role":"user","text":""}"#,
            r#"{"role":"user","text":"hi","extra":1}"#,
        ] {
            let payload = format!(
                r#"{{"provider_id":"codex","input":{{"text":"Next?","history":[{history}]}}}}"#
            );
            let invalid = envelope("req_bad_history", "conversation.send", &payload);
            assert_eq!(
                parse_request(invalid.as_bytes()).unwrap_err().kind,
                FailureKind::InvalidPayload
            );
        }
    }

    #[test]
    fn keeps_the_question_text_with_escapes_resolved() {
        // `input.text` reaches the provider decoded; it is the one payload
        // member the host passes on.
        let input = envelope(
            "req_t",
            "conversation.send",
            r#"{"provider_id":"fake","input":{"text":"caf\u00e9\n\"quoted\" \ud83d\ude00","other":1}}"#,
        );
        let request = parse_request(input.as_bytes()).unwrap();
        let Method::ConversationSend { text, .. } = request.method else {
            panic!("unexpected method");
        };
        assert_eq!(text.decode(), "caf\u{e9}\n\"quoted\" \u{1f600}");
    }

    #[test]
    fn rejects_malformed_and_non_object_input() {
        expect_failure("{not-json", FailureKind::Malformed, None);
        expect_failure("", FailureKind::Malformed, None);
        expect_failure("{} {}", FailureKind::Malformed, None);
        expect_failure("{}x", FailureKind::Malformed, None);
        expect_failure("{}\0", FailureKind::Malformed, None);
        expect_failure("[1,2,3]", FailureKind::InvalidEnvelope, None);
        expect_failure("{}", FailureKind::InvalidEnvelope, None);
        expect_failure(" \"text\" ", FailureKind::InvalidEnvelope, None);
    }

    #[test]
    fn rejects_invalid_envelopes_with_the_recovered_id() {
        expect_failure(
            r#"{"version":1,"type":"request","request_id":"req_2","method":"provider.status"}"#,
            FailureKind::InvalidEnvelope,
            Some("req_2"),
        );
        expect_failure(
            r#"{"version":1,"type":"request","request_id":"req_3","method":"provider.status","payload":{},"extra":true}"#,
            FailureKind::InvalidEnvelope,
            Some("req_3"),
        );
        expect_failure(
            &envelope("req_4", "provider.status", "[]"),
            FailureKind::InvalidEnvelope,
            Some("req_4"),
        );
        expect_failure(
            r#"{"version":1,"type":"event","request_id":"req_type","method":"provider.status","payload":{}}"#,
            FailureKind::InvalidEnvelope,
            Some("req_type"),
        );
    }

    #[test]
    fn rejects_duplicate_members() {
        expect_failure(
            r#"{"version":1,"version":1,"type":"request","request_id":"req_dup_v","method":"provider.status","payload":{}}"#,
            FailureKind::InvalidEnvelope,
            Some("req_dup_v"),
        );
        expect_failure(
            r#"{"version":1,"type":"request","request_id":"req_dup_m","method":"provider.status","method":"provider.status","payload":{}}"#,
            FailureKind::InvalidEnvelope,
            Some("req_dup_m"),
        );
        expect_failure(
            &envelope(
                "req_dup_p",
                "conversation.send",
                r#"{"provider_id":"fake","provider_id":"fake","input":{"text":"Hello"}}"#,
            ),
            FailureKind::InvalidPayload,
            Some("req_dup_p"),
        );
    }

    #[test]
    fn rejects_duplicate_names_in_every_payload_object() {
        for payload in [
            // Members the host does not interpret, at every level.
            r#"{"provider_id":"fake","input":{"text":"Hi"},"future":1,"future":2}"#,
            r#"{"provider_id":"fake","input":{"text":"Hi","future":1,"future":2}}"#,
            r#"{"provider_id":"fake","input":{"text":"Hi"},"context":{"mode":"selection","text":"benign","text":"evil","truncated":false,"page":{"title":"T","url":"https://example.com/"}}}"#,
            r#"{"provider_id":"fake","input":{"text":"Hi"},"context":{"mode":"selection","mode":"page","text":"x","truncated":false,"page":{"title":"T","url":"https://example.com/"}}}"#,
            r#"{"provider_id":"fake","input":{"text":"Hi"},"context":{"mode":"selection","text":"x","truncated":false,"page":{"title":"T","url":"https://example.com/","url":"https://evil.example/"}}}"#,
            r#"{"provider_id":"fake","input":{"text":"Hi"},"future":[{"k":1},{"k":1,"k":2}]}"#,
            // Names are compared after decoding escapes.
            r#"{"provider_id":"fake","input":{"text":"Hi"},"future":1,"futur\u0065":2}"#,
            r#"{"provider_id":"fake","provider_\u0069d":"fake","input":{"text":"Hi"}}"#,
            "{\"provider_id\":\"fake\",\"input\":{\"text\":\"Hi\"},\"caf\\u00e9\":1,\"caf\u{e9}\":2}",
            "{\"provider_id\":\"fake\",\"input\":{\"text\":\"Hi\"},\"\\ud83d\\ude00\":1,\"\u{1f600}\":2}",
        ] {
            expect_failure(
                &envelope("req_dup", "conversation.send", payload),
                FailureKind::InvalidPayload,
                Some("req_dup"),
            );
        }

        for (method, payload) in [
            ("provider.status", r#"{"x":1,"x":2}"#),
            (
                "request.cancel",
                r#"{"target_request_id":"req_1","x":{},"x":[]}"#,
            ),
        ] {
            expect_failure(
                &envelope("req_dup", method, payload),
                FailureKind::InvalidPayload,
                Some("req_dup"),
            );
        }
    }

    #[test]
    fn a_name_may_repeat_across_different_objects() {
        let input = envelope(
            "req_names",
            "conversation.send",
            r#"{"provider_id":"fake","input":{"text":"Hi"},"context":{"mode":"selection","text":"x","truncated":false,"page":{"title":"T","url":"https://example.com/"},"future":{"k":1}},"k":{"k":{"k":1}}}"#,
        );
        assert!(parse_request(input.as_bytes()).is_ok());
    }

    #[test]
    fn payload_duplicates_rank_below_syntax_and_envelope_failures() {
        let duplicated = r#"{"x":1,"x":2}"#;
        expect_failure(
            &format!("{}x", envelope("req_rank", "provider.status", duplicated)),
            FailureKind::Malformed,
            Some("req_rank"),
        );
        expect_failure(
            &envelope("req_rank", "unknown.method", duplicated),
            FailureKind::UnknownMethod,
            Some("req_rank"),
        );
    }

    #[test]
    fn version_must_be_the_integer_token_one() {
        for version in ["1.0", "1e0", "\"1\"", "true", "99999999999999999999"] {
            expect_failure(
                &VALID_CONVERSATION.replace(r#""version":1"#, &format!(r#""version":{version}"#)),
                FailureKind::InvalidEnvelope,
                Some("req_1"),
            );
        }

        let input =
            envelope("req_v2", "provider.status", "{}").replace(r#""version":1"#, r#""version":2"#);
        let failure = parse_request(input.as_bytes()).unwrap_err();
        assert_eq!(failure.kind, FailureKind::UnsupportedVersion);
        assert_eq!(failure.received_version, Some(2));
        assert_eq!(failure.request_id.map(RequestId::raw), Some(&b"req_v2"[..]));
    }

    #[test]
    fn rejects_unknown_methods() {
        expect_failure(
            &envelope("req_6", "unknown.method", "{}"),
            FailureKind::UnknownMethod,
            Some("req_6"),
        );
    }

    #[test]
    fn request_ids_are_limited_to_128_grammar_characters() {
        let id128 = "a".repeat(128);
        let input = envelope(&id128, "provider.status", "{}");
        let request = parse_request(input.as_bytes()).unwrap();
        assert_eq!(request.request_id.raw(), id128.as_bytes());

        for invalid in [
            "a".repeat(129),
            String::new(),
            "_leading".into(),
            "bad id".into(),
            "caf\u{e9}".into(),
        ] {
            expect_failure(
                &envelope(&invalid, "provider.status", "{}"),
                FailureKind::InvalidEnvelope,
                None,
            );
        }
    }

    #[test]
    fn escaped_names_and_ids_are_decoded_for_matching() {
        let input = envelope(
            r"r\u0065q_escape",
            r"conversation.sen\u0064",
            r#"{"provider_id":"fak\u0065","input":{"text":"Hello"}}"#,
        );
        let request = parse_request(input.as_bytes()).unwrap();
        assert_eq!(request.request_id.raw(), br"r\u0065q_escape");
        let Method::ConversationSend { provider_id, .. } = request.method else {
            panic!("unexpected method");
        };
        assert!(provider_id.equals_ascii("fake"));
    }

    #[test]
    fn recovers_the_request_id_before_a_syntax_error() {
        expect_failure(
            r#"{"version":1,"type":"request","request_id":"req_malformed","method":"provider.status","payload":{}}x"#,
            FailureKind::Malformed,
            Some("req_malformed"),
        );
        expect_failure(
            r#"{"request_id":"req_early","version":tru}"#,
            FailureKind::Malformed,
            Some("req_early"),
        );
        expect_failure(
            r#"{"version":tru,"request_id":"req_late"}"#,
            FailureKind::Malformed,
            None,
        );
    }

    #[test]
    fn depth_overflow_is_a_payload_error_only_inside_the_payload() {
        let deep = nested_arrays(140);
        expect_failure(
            &envelope(
                "req_deep",
                "conversation.send",
                &format!(r#"{{"provider_id":"fake","input":{{"text":"Hello"}},"context":{deep}}}"#),
            ),
            FailureKind::InvalidPayload,
            Some("req_deep"),
        );
        expect_failure(
            &format!(r#"{{"request_id":"req_deep_extra","extra":{deep}}}"#),
            FailureKind::Malformed,
            Some("req_deep_extra"),
        );
        expect_failure(
            &format!(r#"{{"request_id":"req_deep_array","payload":{deep}}}"#),
            FailureKind::Malformed,
            Some("req_deep_array"),
        );
    }

    #[test]
    fn validates_conversation_payloads() {
        for payload in [
            r#"{"provider_id":"fake"}"#,
            r#"{"input":{"text":"Hello"}}"#,
            "{}",
            r#"{"provider_id":"","input":{"text":"Hello"}}"#,
            r#"{"provider_id":7,"input":{"text":"Hello"}}"#,
            r#"{"provider_id":"fake","input":{}}"#,
            r#"{"provider_id":"fake","input":{"text":""}}"#,
            r#"{"provider_id":"fake","input":{"text":1}}"#,
            r#"{"provider_id":"fake","input":{"text":"a","text":"b"}}"#,
            r#"{"provider_id":"fake","input":"Hello"}"#,
            r#"{"provider_id":"fake","input":{"text":"Hello"},"conversation_id":""}"#,
            r#"{"provider_id":"fake","input":{"text":"Hello"},"context":[]}"#,
        ] {
            expect_failure(
                &envelope("req_7", "conversation.send", payload),
                FailureKind::InvalidPayload,
                Some("req_7"),
            );
        }

        let input = envelope(
            "req_8",
            "conversation.send",
            r#"{"provider_id":"codex","input":{"text":"Hi","extra":1},"conversation_id":"conv_1","context":{"mode":"selection","text":"Selected","truncated":false,"page":{"title":"Example","url":"https://example.com/path"},"future":null},"future":null}"#,
        );
        let request = parse_request(input.as_bytes()).unwrap();
        let Method::ConversationSend {
            provider_id,
            conversation_id,
            text,
            history,
            context,
        } = request.method
        else {
            panic!("unexpected method");
        };
        assert!(provider_id.equals_ascii("codex"));
        assert_eq!(text.decode(), "Hi");
        assert!(history.is_empty());
        assert!(context.is_some());
        assert_eq!(conversation_id.map(JsonStr::raw), Some(&b"conv_1"[..]));
    }

    #[test]
    fn validates_browser_context_schema_and_bounds() {
        let valid = envelope(
            "req_context",
            "conversation.send",
            r#"{"provider_id":"codex","input":{"text":"Explain"},"context":{"mode":"page","text":"Article text","truncated":true,"page":{"title":"Article","url":"https://example.com/path"},"future":1}}"#,
        );
        let request = parse_request(valid.as_bytes()).unwrap();
        let Method::ConversationSend {
            context: Some(context),
            ..
        } = request.method
        else {
            panic!("expected browser context");
        };
        assert_eq!(context.mode, BrowserContextMode::Page);
        assert_eq!(context.text, "Article text");
        assert!(context.truncated);
        assert_eq!(context.page.title, "Article");
        assert_eq!(context.page.url, "https://example.com/path");

        // Match JavaScript String.prototype.trim(): U+0085 is content,
        // while U+FEFF is trimmed and therefore cannot be the whole context.
        let nel = envelope(
            "req_nel",
            "conversation.send",
            r#"{"provider_id":"codex","input":{"text":"Explain"},"context":{"mode":"selection","text":"\u0085","truncated":false,"page":{"title":"T","url":"https://example.com/"}}}"#,
        );
        assert!(parse_request(nel.as_bytes()).is_ok());

        let bom = envelope(
            "req_bom",
            "conversation.send",
            r#"{"provider_id":"codex","input":{"text":"Explain"},"context":{"mode":"selection","text":"\ufeff","truncated":false,"page":{"title":"T","url":"https://example.com/"}}}"#,
        );
        expect_failure(&bom, FailureKind::InvalidPayload, Some("req_bom"));

        for context in [
            r#"{}"#.to_owned(),
            r#"{"mode":"other","text":"x","truncated":false,"page":{"title":"T","url":"https://example.com/"}}"#.to_owned(),
            r#"{"mode":"selection","text":"","truncated":false,"page":{"title":"T","url":"https://example.com/"}}"#.to_owned(),
            r#"{"mode":"selection","text":"x","truncated":"no","page":{"title":"T","url":"https://example.com/"}}"#.to_owned(),
            r#"{"mode":"selection","text":"x","truncated":false,"page":{"title":"T","url":"file:///tmp/a"}}"#.to_owned(),
            r#"{"mode":"selection","text":"x","truncated":false,"page":{"title":"T","url":"https://user@example.com/"}}"#.to_owned(),
            r#"{"mode":"selection","text":"x","truncated":false,"page":{"title":"T","url":"https://example.com/?secret=1"}}"#.to_owned(),
            serde_json::json!({
                "mode": "selection",
                "text": "x".repeat(MAX_SELECTION_BYTES + 1),
                "truncated": true,
                "page": {"title":"T","url":"https://example.com/"}
            }).to_string(),
            serde_json::json!({
                "mode": "page",
                "text": "x".repeat(MAX_PAGE_BYTES + 1),
                "truncated": true,
                "page": {"title":"T","url":"https://example.com/"}
            }).to_string(),
        ] {
            let payload = format!(
                r#"{{"provider_id":"codex","input":{{"text":"Explain"}},"context":{context}}}"#
            );
            expect_failure(
                &envelope("req_bad_context", "conversation.send", &payload),
                FailureKind::InvalidPayload,
                Some("req_bad_context"),
            );
        }
    }

    #[test]
    fn validates_status_payloads() {
        let input = envelope("req_s", "provider.status", "{}");
        let request = parse_request(input.as_bytes()).unwrap();
        assert_eq!(request.method, Method::ProviderStatus { provider_id: None });

        let input = envelope(
            "req_s",
            "provider.status",
            r#"{"provider_id":"codex","x":1}"#,
        );
        let request = parse_request(input.as_bytes()).unwrap();
        let Method::ProviderStatus {
            provider_id: Some(provider_id),
        } = request.method
        else {
            panic!("unexpected method");
        };
        assert!(provider_id.equals_ascii("codex"));

        for payload in [
            r#"{"provider_id":""}"#,
            r#"{"provider_id":null}"#,
            r#"{"provider_id":"a","provider_id":"b"}"#,
        ] {
            expect_failure(
                &envelope("req_s", "provider.status", payload),
                FailureKind::InvalidPayload,
                Some("req_s"),
            );
        }
    }

    #[test]
    fn validates_cancel_payloads() {
        let input = envelope(
            "req_c",
            "request.cancel",
            r#"{"target_request_id":"req_answer_1"}"#,
        );
        let request = parse_request(input.as_bytes()).unwrap();
        let Method::RequestCancel { target_request_id } = request.method else {
            panic!("unexpected method");
        };
        assert_eq!(target_request_id.raw(), b"req_answer_1");

        for payload in [
            "{}",
            r#"{"target_request_id":"bad id"}"#,
            r#"{"target_request_id":1}"#,
            r#"{"target_request_id":"a","target_request_id":"b"}"#,
        ] {
            expect_failure(
                &envelope("req_c", "request.cancel", payload),
                FailureKind::InvalidPayload,
                Some("req_c"),
            );
        }
    }
}
