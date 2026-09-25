//! Strict, bounded JSON syntax reader for request validation.
//!
//! Requests are validated with this pull reader rather than a serde
//! deserializer because protocol v1 (`docs/protocol/v1.md` §1 and §8) depends
//! on details serde's data model hides: duplicate member names must be
//! rejected, member order decides whether a request ID was recovered before a
//! syntax error, request IDs are echoed as their raw JSON bytes, and depth
//! overflow is classified differently inside and outside the method payload.

use std::borrow::Cow;
use std::collections::HashSet;

use crate::limits::MAX_JSON_DEPTH;

/// Why the reader rejected its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonError {
    Syntax,
    DepthExceeded,
    /// An object repeats a member name. Only reported by readers created with
    /// [`Reader::rejecting_duplicate_members`].
    DuplicateMember,
}

/// A validated JSON string token, still in its raw escaped form (the bytes
/// between the quotes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonStr<'a> {
    raw: &'a [u8],
}

impl<'a> JsonStr<'a> {
    /// The string's bytes exactly as they appeared between the quotes.
    pub fn raw(self) -> &'a [u8] {
        self.raw
    }

    /// Whether the token was written as `""`.
    pub fn is_empty(self) -> bool {
        self.raw.is_empty()
    }

    /// Whether the decoded string is exactly `ascii`. Strings that contain any
    /// non-ASCII character never match.
    pub fn equals_ascii(self, ascii: &str) -> bool {
        let mut expected = ascii.bytes();
        let mut position = 0;
        while position < self.raw.len() {
            match decode_next_ascii(self.raw, &mut position) {
                Some(decoded) if expected.next() == Some(decoded) => {}
                _ => return false,
            }
        }
        expected.next().is_none()
    }

    /// Decodes the next character, or returns `None` at the end of the string.
    /// `Some(None)` means the next character is not ASCII.
    pub(crate) fn next_ascii(self, position: &mut usize) -> Option<Option<u8>> {
        (*position < self.raw.len()).then(|| decode_next_ascii(self.raw, position))
    }

    /// The string with its escapes resolved. A token without escapes is
    /// borrowed rather than copied.
    pub fn decode(self) -> Cow<'a, str> {
        if !self.raw.contains(&b'\\') {
            // `parse_string` validated the UTF-8, so nothing is replaced.
            return String::from_utf8_lossy(self.raw);
        }

        let mut decoded = Vec::with_capacity(self.raw.len());
        let mut position = 0;
        while let Some(&byte) = self.raw.get(position) {
            position += 1;
            if byte != b'\\' {
                decoded.push(byte);
                continue;
            }
            match decode_escape(self.raw, &mut position) {
                Some(character) => {
                    let mut buffer = [0; 4];
                    decoded.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
                }
                // Unreachable for tokens from `parse_string`, which rejects
                // malformed escapes.
                None => break,
            }
        }
        Cow::Owned(match String::from_utf8(decoded) {
            Ok(text) => text,
            Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
        })
    }
}

/// Pull reader over one JSON text.
#[derive(Debug)]
pub struct Reader<'a> {
    data: &'a [u8],
    position: usize,
    depth: usize,
    reject_duplicate_members: bool,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            position: 0,
            depth: 0,
            reject_duplicate_members: false,
        }
    }

    /// A reader whose [`Reader::skip_value`] also fails with
    /// [`JsonError::DuplicateMember`] when any object repeats a member name,
    /// comparing names after decoding escapes. A duplicate is reported as soon
    /// as it is seen, so use this on input whose syntax is already validated.
    pub fn rejecting_duplicate_members(data: &'a [u8]) -> Self {
        Self {
            reject_duplicate_members: true,
            ..Self::new(data)
        }
    }

    /// Byte offset of the next unread byte.
    pub fn position(&self) -> usize {
        self.position
    }

    /// The input consumed since `start`, an offset previously returned by
    /// [`Reader::position`].
    pub fn span_from(&self, start: usize) -> &'a [u8] {
        &self.data[start..self.position]
    }

    pub fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.position += 1;
        }
    }

    pub fn at_end(&self) -> bool {
        self.position >= self.data.len()
    }

    pub fn peek(&self) -> Option<u8> {
        self.data.get(self.position).copied()
    }

    /// Consumes `expected` if it is the next byte.
    pub fn consume(&mut self, expected: u8) -> bool {
        let matched = self.peek() == Some(expected);
        if matched {
            self.position += 1;
        }
        matched
    }

    /// Parses a string token, validating escapes, surrogate pairs, and UTF-8.
    pub fn parse_string(&mut self) -> Result<JsonStr<'a>, JsonError> {
        if !self.consume(b'"') {
            return Err(JsonError::Syntax);
        }

        let start = self.position;
        let mut position = start;
        while let Some(&byte) = self.data.get(position) {
            match byte {
                b'"' => {
                    self.position = position + 1;
                    return Ok(JsonStr {
                        raw: &self.data[start..position],
                    });
                }
                0x00..=0x1f => return Err(JsonError::Syntax),
                b'\\' => {
                    position += 1;
                    match self.data.get(position) {
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => {
                            position += 1;
                        }
                        Some(b'u') => {
                            position += 1;
                            let first =
                                parse_hex4(self.data, &mut position).ok_or(JsonError::Syntax)?;
                            if (0xd800..=0xdbff).contains(&first) {
                                if self.data.get(position..position + 2) != Some(&b"\\u"[..]) {
                                    return Err(JsonError::Syntax);
                                }
                                position += 2;
                                let second = parse_hex4(self.data, &mut position)
                                    .ok_or(JsonError::Syntax)?;
                                if !(0xdc00..=0xdfff).contains(&second) {
                                    return Err(JsonError::Syntax);
                                }
                            } else if (0xdc00..=0xdfff).contains(&first) {
                                return Err(JsonError::Syntax);
                            }
                        }
                        _ => return Err(JsonError::Syntax),
                    }
                }
                0x20..=0x7f => position += 1,
                _ => position = utf8_sequence_end(self.data, position).ok_or(JsonError::Syntax)?,
            }
        }

        Err(JsonError::Syntax)
    }

    /// Parses a number token. Returns `Ok(None)` for a valid number that is not
    /// an integer token (fraction or exponent) or that does not fit in `i64`.
    pub fn parse_integer(&mut self) -> Result<Option<i64>, JsonError> {
        let number = self.parse_number()?;
        if !number.is_integer {
            return Ok(None);
        }

        let token = &self.data[number.start..number.end];
        let (negative, digits) = match token.split_first() {
            Some((b'-', digits)) => (true, digits),
            _ => (false, token),
        };
        let limit = if negative {
            i64::MIN.unsigned_abs()
        } else {
            i64::MAX.unsigned_abs()
        };

        let mut magnitude: u64 = 0;
        for &digit in digits {
            let digit = u64::from(digit - b'0');
            if magnitude > (limit - digit) / 10 {
                return Ok(None);
            }
            magnitude = magnitude * 10 + digit;
        }

        Ok(if negative {
            0_i64.checked_sub_unsigned(magnitude)
        } else {
            i64::try_from(magnitude).ok()
        })
    }

    /// Validates and skips one value of any type.
    pub fn skip_value(&mut self) -> Result<(), JsonError> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'{') => self.skip_object(),
            Some(b'[') => self.skip_array(),
            Some(b'"') => self.parse_string().map(|_| ()),
            Some(b't') => self.match_literal(b"true"),
            Some(b'f') => self.match_literal(b"false"),
            Some(b'n') => self.match_literal(b"null"),
            Some(b'-' | b'0'..=b'9') => self.parse_number().map(|_| ()),
            _ => Err(JsonError::Syntax),
        }
    }

    fn parse_number(&mut self) -> Result<NumberToken, JsonError> {
        let start = self.position;
        let mut position = start;
        let mut is_integer = true;

        if self.data.get(position) == Some(&b'-') {
            position += 1;
        }

        match self.data.get(position) {
            Some(b'0') => {
                position += 1;
                if matches!(self.data.get(position), Some(b'0'..=b'9')) {
                    return Err(JsonError::Syntax);
                }
            }
            Some(b'1'..=b'9') => position = skip_digits(self.data, position),
            _ => return Err(JsonError::Syntax),
        }

        if self.data.get(position) == Some(&b'.') {
            is_integer = false;
            position += 1;
            if !matches!(self.data.get(position), Some(b'0'..=b'9')) {
                return Err(JsonError::Syntax);
            }
            position = skip_digits(self.data, position);
        }

        if matches!(self.data.get(position), Some(b'e' | b'E')) {
            is_integer = false;
            position += 1;
            if matches!(self.data.get(position), Some(b'+' | b'-')) {
                position += 1;
            }
            if !matches!(self.data.get(position), Some(b'0'..=b'9')) {
                return Err(JsonError::Syntax);
            }
            position = skip_digits(self.data, position);
        }

        self.position = position;
        Ok(NumberToken {
            start,
            end: position,
            is_integer,
        })
    }

    fn match_literal(&mut self, literal: &[u8]) -> Result<(), JsonError> {
        if !self.data[self.position..].starts_with(literal) {
            return Err(JsonError::Syntax);
        }
        self.position += literal.len();
        Ok(())
    }

    fn enter_depth(&mut self) -> Result<(), JsonError> {
        if self.depth >= MAX_JSON_DEPTH {
            return Err(JsonError::DepthExceeded);
        }
        self.depth += 1;
        Ok(())
    }

    fn skip_array(&mut self) -> Result<(), JsonError> {
        if !self.consume(b'[') {
            return Err(JsonError::Syntax);
        }
        self.enter_depth()?;
        let result = self.skip_array_items();
        self.depth -= 1;
        result
    }

    fn skip_array_items(&mut self) -> Result<(), JsonError> {
        self.skip_whitespace();
        if self.consume(b']') {
            return Ok(());
        }
        loop {
            self.skip_value()?;
            self.skip_whitespace();
            if self.consume(b']') {
                return Ok(());
            }
            if !self.consume(b',') {
                return Err(JsonError::Syntax);
            }
            self.skip_whitespace();
        }
    }

    fn skip_object(&mut self) -> Result<(), JsonError> {
        if !self.consume(b'{') {
            return Err(JsonError::Syntax);
        }
        self.enter_depth()?;
        let result = self.skip_object_members();
        self.depth -= 1;
        result
    }

    fn skip_object_members(&mut self) -> Result<(), JsonError> {
        self.skip_whitespace();
        if self.consume(b'}') {
            return Ok(());
        }
        let mut names = HashSet::new();
        loop {
            let name = self.parse_string()?;
            if self.reject_duplicate_members && !names.insert(name.decode()) {
                return Err(JsonError::DuplicateMember);
            }
            self.skip_whitespace();
            if !self.consume(b':') {
                return Err(JsonError::Syntax);
            }
            self.skip_value()?;
            self.skip_whitespace();
            if self.consume(b'}') {
                return Ok(());
            }
            if !self.consume(b',') {
                return Err(JsonError::Syntax);
            }
            self.skip_whitespace();
        }
    }
}

struct NumberToken {
    start: usize,
    end: usize,
    is_integer: bool,
}

fn skip_digits(data: &[u8], mut position: usize) -> usize {
    while matches!(data.get(position), Some(b'0'..=b'9')) {
        position += 1;
    }
    position
}

fn parse_hex4(data: &[u8], position: &mut usize) -> Option<u32> {
    let digits = data.get(*position..*position + 4)?;
    let mut value = 0;
    for &digit in digits {
        value = (value << 4) | char::from(digit).to_digit(16)?;
    }
    *position += 4;
    Some(value)
}

/// Returns the offset just past the well-formed UTF-8 sequence that starts at
/// `position`, rejecting overlong forms, surrogates, and values above U+10FFFF.
fn utf8_sequence_end(data: &[u8], position: usize) -> Option<usize> {
    let first = *data.get(position)?;
    let (length, mut codepoint) = match first {
        0x00..=0x7f => return Some(position + 1),
        0xc2..=0xdf => (2, u32::from(first & 0x1f)),
        0xe0..=0xef => (3, u32::from(first & 0x0f)),
        0xf0..=0xf4 => (4, u32::from(first & 0x07)),
        _ => return None,
    };

    for &next in data.get(position + 1..position + length)? {
        if next & 0xc0 != 0x80 {
            return None;
        }
        codepoint = (codepoint << 6) | u32::from(next & 0x3f);
    }

    let overlong = (length == 3 && codepoint < 0x800) || (length == 4 && codepoint < 0x1_0000);
    if overlong || codepoint > 0x10_ffff || (0xd800..=0xdfff).contains(&codepoint) {
        return None;
    }
    Some(position + length)
}

/// Decodes the escape sequence that follows a backslash at `position`,
/// joining a UTF-16 surrogate pair into one character. Returns `None` for a
/// malformed escape.
fn decode_escape(raw: &[u8], position: &mut usize) -> Option<char> {
    let escaped = *raw.get(*position)?;
    *position += 1;
    match escaped {
        b'"' | b'\\' | b'/' => Some(char::from(escaped)),
        b'b' => Some('\u{8}'),
        b'f' => Some('\u{c}'),
        b'n' => Some('\n'),
        b'r' => Some('\r'),
        b't' => Some('\t'),
        b'u' => {
            let first = parse_hex4(raw, position)?;
            if !(0xd800..=0xdbff).contains(&first) {
                return char::from_u32(first);
            }
            if raw.get(*position..*position + 2) != Some(&b"\\u"[..]) {
                return None;
            }
            *position += 2;
            let second = parse_hex4(raw, position)?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return None;
            }
            char::from_u32(0x1_0000 + ((first - 0xd800) << 10) + (second - 0xdc00))
        }
        _ => None,
    }
}

/// Decodes one character of a raw string token as ASCII. Returns `None` for
/// non-ASCII characters and malformed escapes.
fn decode_next_ascii(raw: &[u8], position: &mut usize) -> Option<u8> {
    let byte = *raw.get(*position)?;
    *position += 1;
    if !byte.is_ascii() {
        return None;
    }
    if byte != b'\\' {
        return Some(byte);
    }

    let escaped = *raw.get(*position)?;
    *position += 1;
    match escaped {
        b'"' | b'\\' | b'/' => Some(escaped),
        b'b' => Some(0x08),
        b'f' => Some(0x0c),
        b'n' => Some(b'\n'),
        b'r' => Some(b'\r'),
        b't' => Some(b'\t'),
        b'u' => {
            let value = parse_hex4(raw, position)?;
            u8::try_from(value).ok().filter(u8::is_ascii)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skip(text: &str) -> Result<(), JsonError> {
        let mut reader = Reader::new(text.as_bytes());
        reader.skip_value()?;
        reader.skip_whitespace();
        if reader.at_end() {
            Ok(())
        } else {
            Err(JsonError::Syntax)
        }
    }

    fn integer(text: &str) -> Result<Option<i64>, JsonError> {
        Reader::new(text.as_bytes()).parse_integer()
    }

    fn string(bytes: &[u8]) -> Result<JsonStr<'_>, JsonError> {
        Reader::new(bytes).parse_string()
    }

    fn nested(depth: usize) -> String {
        format!("{}0{}", "[".repeat(depth), "]".repeat(depth))
    }

    #[test]
    fn accepts_every_value_type() {
        for text in [
            "{}",
            "[]",
            r#"{"a":[1,-2.5e+3,true,false,null,"x"],"b":{}}"#,
            " \t\r\n0 ",
            "-0",
            "1E9",
            r#""\"\\\/\b\f\n\r\t\u00e9\ud83d\ude00""#,
        ] {
            assert_eq!(skip(text), Ok(()), "{text}");
        }
    }

    #[test]
    fn rejects_invalid_syntax() {
        for text in [
            "",
            "{",
            "[1,]",
            "{\"a\":1,}",
            "{\"a\" 1}",
            "{1:2}",
            "01",
            "-",
            "1.",
            "1e",
            ".5",
            "+1",
            "tru",
            "nul",
            "NaN",
            "'a'",
            "\"abc",
            "[1 2]",
        ] {
            assert_eq!(skip(text), Err(JsonError::Syntax), "{text}");
        }
    }

    #[test]
    fn rejects_invalid_strings() {
        for bytes in [
            &b"\"\x01\""[..],
            b"\"\\x\"",
            b"\"\\u12\"",
            b"\"\\u12g4\"",
            b"\"\\ud800\"",
            b"\"\\ud800\\u0041\"",
            b"\"\\udc00\"",
            b"\"\xc0\xaf\"",
            b"\"\xe0\x80\xaf\"",
            b"\"\xed\xa0\x80\"",
            b"\"\xf4\x90\x80\x80\"",
            b"\"\xf5\x80\x80\x80\"",
            b"\"\xc3\"",
            b"\"\xff\"",
        ] {
            assert_eq!(string(bytes), Err(JsonError::Syntax), "{bytes:?}");
        }
    }

    #[test]
    fn strings_keep_their_raw_escaped_bytes() {
        let token = string(br#""r\u0065q" tail"#).unwrap();
        assert_eq!(token.raw(), br"r\u0065q");
        assert!(token.equals_ascii("req"));
        assert!(!token.equals_ascii("re"));
        assert!(!token.equals_ascii("requ"));
        assert!(!string("\"é\"".as_bytes()).unwrap().equals_ascii("e"));
        assert!(string(b"\"\"").unwrap().is_empty());
    }

    #[test]
    fn integers_report_whether_they_fit() {
        assert_eq!(integer("1"), Ok(Some(1)));
        assert_eq!(integer("-0"), Ok(Some(0)));
        assert_eq!(integer("9223372036854775807"), Ok(Some(i64::MAX)));
        assert_eq!(integer("-9223372036854775808"), Ok(Some(i64::MIN)));
        assert_eq!(integer("9223372036854775808"), Ok(None));
        assert_eq!(integer("-9223372036854775809"), Ok(None));
        assert_eq!(integer("99999999999999999999"), Ok(None));
        assert_eq!(integer("1.0"), Ok(None));
        assert_eq!(integer("1e0"), Ok(None));
        assert_eq!(integer("x"), Err(JsonError::Syntax));
    }

    #[test]
    fn nesting_is_capped_at_the_maximum_depth() {
        assert_eq!(skip(&nested(MAX_JSON_DEPTH)), Ok(()));
        assert_eq!(
            skip(&nested(MAX_JSON_DEPTH + 1)),
            Err(JsonError::DepthExceeded)
        );
    }

    #[test]
    fn depth_is_released_after_each_container() {
        let siblings = vec![nested(MAX_JSON_DEPTH - 1); 3].join(",");
        assert_eq!(skip(&format!("[{siblings}]")), Ok(()));
    }

    #[test]
    fn decode_resolves_escapes_and_borrows_plain_tokens() {
        let plain = string("\"caf\u{e9} plain\"".as_bytes()).unwrap();
        assert!(matches!(plain.decode(), Cow::Borrowed("caf\u{e9} plain")));

        let escaped = string(br#""\"\\\/\b\f\n\r\t \u0041\u00e9\u20ac\ud83d\ude00""#).unwrap();
        assert_eq!(
            escaped.decode(),
            "\"\\/\u{8}\u{c}\n\r\t A\u{e9}\u{20ac}\u{1f600}"
        );
    }

    #[test]
    fn decode_matches_serde_json() {
        for text in [
            "",
            "plain",
            "tab\tnew\nline",
            "quote\" backslash\\ slash/",
            "control \u{1} \u{1f}",
            "caf\u{e9} \u{20ac} \u{1f600} \u{10ffff}",
        ] {
            // serde_json escapes quotes, backslashes, and control characters.
            let serde_escaped = serde_json::to_string(text).unwrap();
            // Escape every character, as `\uXXXX` units, to cover surrogate pairs.
            let mut all_escaped = String::from("\"");
            for unit in text.encode_utf16() {
                all_escaped.push_str(&format!("\\u{unit:04x}"));
            }
            all_escaped.push('"');

            for encoded in [serde_escaped, all_escaped] {
                let token = string(encoded.as_bytes()).unwrap();
                let expected: String = serde_json::from_str(&encoded).unwrap();
                assert_eq!(token.decode(), expected, "{encoded}");
            }
        }
    }

    #[test]
    fn duplicate_members_are_rejected_only_on_request() {
        let duplicated = br#"{"a":1,"b":{"c":[{"d":1,"d":2}]}}"#;
        assert_eq!(Reader::new(duplicated).skip_value(), Ok(()));
        assert_eq!(
            Reader::rejecting_duplicate_members(duplicated).skip_value(),
            Err(JsonError::DuplicateMember)
        );

        let unique = br#"{"a":{"a":{"a":1}},"b":[{"a":1},{"a":2}]}"#;
        assert_eq!(
            Reader::rejecting_duplicate_members(unique).skip_value(),
            Ok(())
        );
    }
}
