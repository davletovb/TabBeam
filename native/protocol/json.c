#include "json_internal.h"

#include <limits.h>
#include <string.h>

static bool is_hex(unsigned char c) {
  return (c >= (unsigned char)'0' && c <= (unsigned char)'9') ||
         (c >= (unsigned char)'a' && c <= (unsigned char)'f') ||
         (c >= (unsigned char)'A' && c <= (unsigned char)'F');
}

static unsigned int hex_value(unsigned char c) {
  if (c >= (unsigned char)'0' && c <= (unsigned char)'9') {
    return (unsigned int)(c - (unsigned char)'0');
  }
  if (c >= (unsigned char)'a' && c <= (unsigned char)'f') {
    return 10U + (unsigned int)(c - (unsigned char)'a');
  }
  return 10U + (unsigned int)(c - (unsigned char)'A');
}

static bool parse_hex4(
    const unsigned char *data,
    size_t length,
    size_t *position,
    unsigned int *value) {
  size_t index;
  unsigned int result = 0U;

  if (*position > length || length - *position < 4U) {
    return false;
  }

  for (index = 0U; index < 4U; ++index) {
    unsigned char c = data[*position + index];
    if (!is_hex(c)) {
      return false;
    }
    result = (result << 4U) | hex_value(c);
  }

  *position += 4U;
  *value = result;
  return true;
}

static bool validate_utf8_sequence(
    const unsigned char *data,
    size_t length,
    size_t *position) {
  size_t pos = *position;
  unsigned char first;
  uint32_t codepoint;
  size_t count;
  size_t index;

  if (pos >= length) {
    return false;
  }

  first = data[pos];

  if (first < 0x80U) {
    *position = pos + 1U;
    return true;
  }

  if (first >= 0xc2U && first <= 0xdfU) {
    count = 2U;
    codepoint = (uint32_t)(first & 0x1fU);
  } else if (first >= 0xe0U && first <= 0xefU) {
    count = 3U;
    codepoint = (uint32_t)(first & 0x0fU);
  } else if (first >= 0xf0U && first <= 0xf4U) {
    count = 4U;
    codepoint = (uint32_t)(first & 0x07U);
  } else {
    return false;
  }

  if (length - pos < count) {
    return false;
  }

  for (index = 1U; index < count; ++index) {
    unsigned char next = data[pos + index];
    if ((next & 0xc0U) != 0x80U) {
      return false;
    }
    codepoint = (codepoint << 6U) | (uint32_t)(next & 0x3fU);
  }

  if ((count == 3U && codepoint < UINT32_C(0x800)) ||
      (count == 4U && codepoint < UINT32_C(0x10000)) ||
      codepoint > UINT32_C(0x10ffff) ||
      (codepoint >= UINT32_C(0xd800) && codepoint <= UINT32_C(0xdfff))) {
    return false;
  }

  *position = pos + count;
  return true;
}

void pervue_json_reader_init(
    pervue_json_reader_t *reader,
    const unsigned char *data,
    size_t length) {
  reader->data = data;
  reader->length = length;
  reader->position = 0U;
  reader->depth = 0U;
}

void pervue_json_skip_whitespace(pervue_json_reader_t *reader) {
  while (reader->position < reader->length) {
    unsigned char c = reader->data[reader->position];
    if (c != (unsigned char)' ' &&
        c != (unsigned char)'\t' &&
        c != (unsigned char)'\n' &&
        c != (unsigned char)'\r') {
      break;
    }
    reader->position += 1U;
  }
}

bool pervue_json_at_end(const pervue_json_reader_t *reader) {
  return reader->position >= reader->length;
}

int pervue_json_peek(const pervue_json_reader_t *reader) {
  if (reader->position >= reader->length) {
    return -1;
  }
  return (int)reader->data[reader->position];
}

bool pervue_json_consume(pervue_json_reader_t *reader, unsigned char expected) {
  if (reader->position >= reader->length ||
      reader->data[reader->position] != expected) {
    return false;
  }

  reader->position += 1U;
  return true;
}

pervue_json_result_t pervue_json_parse_string(
    pervue_json_reader_t *reader,
    pervue_json_string_view_t *view) {
  size_t start;
  size_t pos;

  if (!pervue_json_consume(reader, (unsigned char)'"')) {
    return PERVUE_JSON_SYNTAX_ERROR;
  }

  start = reader->position;
  pos = reader->position;

  while (pos < reader->length) {
    unsigned char c = reader->data[pos];

    if (c == (unsigned char)'"') {
      if (view != NULL) {
        view->data = reader->data + start;
        view->length = pos - start;
      }
      reader->position = pos + 1U;
      return PERVUE_JSON_OK;
    }

    if (c < 0x20U) {
      return PERVUE_JSON_SYNTAX_ERROR;
    }

    if (c == (unsigned char)'\\') {
      unsigned char escaped;

      pos += 1U;
      if (pos >= reader->length) {
        return PERVUE_JSON_SYNTAX_ERROR;
      }

      escaped = reader->data[pos];
      if (escaped == (unsigned char)'"' ||
          escaped == (unsigned char)'\\' ||
          escaped == (unsigned char)'/' ||
          escaped == (unsigned char)'b' ||
          escaped == (unsigned char)'f' ||
          escaped == (unsigned char)'n' ||
          escaped == (unsigned char)'r' ||
          escaped == (unsigned char)'t') {
        pos += 1U;
        continue;
      }

      if (escaped == (unsigned char)'u') {
        unsigned int first;
        pos += 1U;

        if (!parse_hex4(reader->data, reader->length, &pos, &first)) {
          return PERVUE_JSON_SYNTAX_ERROR;
        }

        if (first >= 0xd800U && first <= 0xdbffU) {
          unsigned int second;

          if (pos + 2U > reader->length ||
              reader->data[pos] != (unsigned char)'\\' ||
              reader->data[pos + 1U] != (unsigned char)'u') {
            return PERVUE_JSON_SYNTAX_ERROR;
          }

          pos += 2U;
          if (!parse_hex4(reader->data, reader->length, &pos, &second) ||
              second < 0xdc00U || second > 0xdfffU) {
            return PERVUE_JSON_SYNTAX_ERROR;
          }
        } else if (first >= 0xdc00U && first <= 0xdfffU) {
          return PERVUE_JSON_SYNTAX_ERROR;
        }

        continue;
      }

      return PERVUE_JSON_SYNTAX_ERROR;
    }

    if (c < 0x80U) {
      pos += 1U;
    } else if (!validate_utf8_sequence(reader->data, reader->length, &pos)) {
      return PERVUE_JSON_SYNTAX_ERROR;
    }
  }

  return PERVUE_JSON_SYNTAX_ERROR;
}

static pervue_json_result_t parse_number(
    pervue_json_reader_t *reader,
    size_t *start_out,
    size_t *end_out,
    bool *is_integer) {
  size_t start = reader->position;
  size_t pos = start;
  bool integer = true;

  if (pos < reader->length && reader->data[pos] == (unsigned char)'-') {
    pos += 1U;
  }

  if (pos >= reader->length) {
    return PERVUE_JSON_SYNTAX_ERROR;
  }

  if (reader->data[pos] == (unsigned char)'0') {
    pos += 1U;
    if (pos < reader->length &&
        reader->data[pos] >= (unsigned char)'0' &&
        reader->data[pos] <= (unsigned char)'9') {
      return PERVUE_JSON_SYNTAX_ERROR;
    }
  } else {
    if (reader->data[pos] < (unsigned char)'1' ||
        reader->data[pos] > (unsigned char)'9') {
      return PERVUE_JSON_SYNTAX_ERROR;
    }

    while (pos < reader->length &&
           reader->data[pos] >= (unsigned char)'0' &&
           reader->data[pos] <= (unsigned char)'9') {
      pos += 1U;
    }
  }

  if (pos < reader->length && reader->data[pos] == (unsigned char)'.') {
    integer = false;
    pos += 1U;

    if (pos >= reader->length ||
        reader->data[pos] < (unsigned char)'0' ||
        reader->data[pos] > (unsigned char)'9') {
      return PERVUE_JSON_SYNTAX_ERROR;
    }

    while (pos < reader->length &&
           reader->data[pos] >= (unsigned char)'0' &&
           reader->data[pos] <= (unsigned char)'9') {
      pos += 1U;
    }
  }

  if (pos < reader->length &&
      (reader->data[pos] == (unsigned char)'e' ||
       reader->data[pos] == (unsigned char)'E')) {
    integer = false;
    pos += 1U;

    if (pos < reader->length &&
        (reader->data[pos] == (unsigned char)'+' ||
         reader->data[pos] == (unsigned char)'-')) {
      pos += 1U;
    }

    if (pos >= reader->length ||
        reader->data[pos] < (unsigned char)'0' ||
        reader->data[pos] > (unsigned char)'9') {
      return PERVUE_JSON_SYNTAX_ERROR;
    }

    while (pos < reader->length &&
           reader->data[pos] >= (unsigned char)'0' &&
           reader->data[pos] <= (unsigned char)'9') {
      pos += 1U;
    }
  }

  reader->position = pos;
  if (start_out != NULL) {
    *start_out = start;
  }
  if (end_out != NULL) {
    *end_out = pos;
  }
  if (is_integer != NULL) {
    *is_integer = integer;
  }
  return PERVUE_JSON_OK;
}

pervue_json_result_t pervue_json_parse_integer(
    pervue_json_reader_t *reader,
    int64_t *value,
    bool *is_integer) {
  size_t start;
  size_t end;
  size_t pos;
  bool integer;
  bool negative = false;
  uint64_t magnitude = 0U;
  uint64_t limit;
  pervue_json_result_t result;

  result = parse_number(reader, &start, &end, &integer);
  if (result != PERVUE_JSON_OK) {
    return result;
  }

  if (is_integer != NULL) {
    *is_integer = integer;
  }

  if (!integer || value == NULL) {
    return PERVUE_JSON_OK;
  }

  pos = start;
  if (reader->data[pos] == (unsigned char)'-') {
    negative = true;
    pos += 1U;
  }

  limit = negative
      ? (uint64_t)INT64_MAX + UINT64_C(1)
      : (uint64_t)INT64_MAX;

  while (pos < end) {
    unsigned int digit = (unsigned int)(reader->data[pos] - (unsigned char)'0');
    if (magnitude > (limit - digit) / UINT64_C(10)) {
      *is_integer = false;
      return PERVUE_JSON_OK;
    }
    magnitude = magnitude * UINT64_C(10) + digit;
    pos += 1U;
  }

  if (negative) {
    if (magnitude == (uint64_t)INT64_MAX + UINT64_C(1)) {
      *value = INT64_MIN;
    } else {
      *value = -(int64_t)magnitude;
    }
  } else {
    *value = (int64_t)magnitude;
  }

  return PERVUE_JSON_OK;
}

static bool match_literal(pervue_json_reader_t *reader, const char *literal) {
  size_t length = strlen(literal);

  if (reader->position > reader->length ||
      reader->length - reader->position < length) {
    return false;
  }

  if (memcmp(reader->data + reader->position, literal, length) != 0) {
    return false;
  }

  reader->position += length;
  return true;
}

static pervue_json_result_t skip_array(pervue_json_reader_t *reader);
static pervue_json_result_t skip_object(pervue_json_reader_t *reader);

static pervue_json_result_t enter_depth(pervue_json_reader_t *reader) {
  if (reader->depth >= PERVUE_JSON_MAX_DEPTH) {
    return PERVUE_JSON_DEPTH_EXCEEDED;
  }
  reader->depth += 1U;
  return PERVUE_JSON_OK;
}

static void leave_depth(pervue_json_reader_t *reader) {
  if (reader->depth > 0U) {
    reader->depth -= 1U;
  }
}

static pervue_json_result_t skip_array(pervue_json_reader_t *reader) {
  pervue_json_result_t result;

  if (!pervue_json_consume(reader, (unsigned char)'[')) {
    return PERVUE_JSON_SYNTAX_ERROR;
  }

  result = enter_depth(reader);
  if (result != PERVUE_JSON_OK) {
    return result;
  }

  pervue_json_skip_whitespace(reader);
  if (pervue_json_consume(reader, (unsigned char)']')) {
    leave_depth(reader);
    return PERVUE_JSON_OK;
  }

  for (;;) {
    result = pervue_json_skip_value(reader);
    if (result != PERVUE_JSON_OK) {
      leave_depth(reader);
      return result;
    }

    pervue_json_skip_whitespace(reader);
    if (pervue_json_consume(reader, (unsigned char)']')) {
      leave_depth(reader);
      return PERVUE_JSON_OK;
    }

    if (!pervue_json_consume(reader, (unsigned char)',')) {
      leave_depth(reader);
      return PERVUE_JSON_SYNTAX_ERROR;
    }

    pervue_json_skip_whitespace(reader);
  }
}

static pervue_json_result_t skip_object(pervue_json_reader_t *reader) {
  pervue_json_result_t result;

  if (!pervue_json_consume(reader, (unsigned char)'{')) {
    return PERVUE_JSON_SYNTAX_ERROR;
  }

  result = enter_depth(reader);
  if (result != PERVUE_JSON_OK) {
    return result;
  }

  pervue_json_skip_whitespace(reader);
  if (pervue_json_consume(reader, (unsigned char)'}')) {
    leave_depth(reader);
    return PERVUE_JSON_OK;
  }

  for (;;) {
    result = pervue_json_parse_string(reader, NULL);
    if (result != PERVUE_JSON_OK) {
      leave_depth(reader);
      return result;
    }

    pervue_json_skip_whitespace(reader);
    if (!pervue_json_consume(reader, (unsigned char)':')) {
      leave_depth(reader);
      return PERVUE_JSON_SYNTAX_ERROR;
    }

    pervue_json_skip_whitespace(reader);
    result = pervue_json_skip_value(reader);
    if (result != PERVUE_JSON_OK) {
      leave_depth(reader);
      return result;
    }

    pervue_json_skip_whitespace(reader);
    if (pervue_json_consume(reader, (unsigned char)'}')) {
      leave_depth(reader);
      return PERVUE_JSON_OK;
    }

    if (!pervue_json_consume(reader, (unsigned char)',')) {
      leave_depth(reader);
      return PERVUE_JSON_SYNTAX_ERROR;
    }

    pervue_json_skip_whitespace(reader);
  }
}

pervue_json_result_t pervue_json_skip_value(pervue_json_reader_t *reader) {
  int c;

  pervue_json_skip_whitespace(reader);
  c = pervue_json_peek(reader);

  if (c < 0) {
    return PERVUE_JSON_SYNTAX_ERROR;
  }

  if (c == (int)'{') {
    return skip_object(reader);
  }
  if (c == (int)'[') {
    return skip_array(reader);
  }
  if (c == (int)'"') {
    return pervue_json_parse_string(reader, NULL);
  }
  if (c == (int)'t') {
    return match_literal(reader, "true")
        ? PERVUE_JSON_OK
        : PERVUE_JSON_SYNTAX_ERROR;
  }
  if (c == (int)'f') {
    return match_literal(reader, "false")
        ? PERVUE_JSON_OK
        : PERVUE_JSON_SYNTAX_ERROR;
  }
  if (c == (int)'n') {
    return match_literal(reader, "null")
        ? PERVUE_JSON_OK
        : PERVUE_JSON_SYNTAX_ERROR;
  }
  if (c == (int)'-' || (c >= (int)'0' && c <= (int)'9')) {
    return parse_number(reader, NULL, NULL, NULL);
  }

  return PERVUE_JSON_SYNTAX_ERROR;
}

static bool decode_next_ascii(
    const pervue_json_string_view_t *view,
    size_t *position,
    unsigned char *decoded) {
  unsigned char c;

  if (*position >= view->length) {
    return false;
  }

  c = view->data[*position];
  *position += 1U;

  if (c >= 0x80U) {
    return false;
  }

  if (c != (unsigned char)'\\') {
    *decoded = c;
    return true;
  }

  if (*position >= view->length) {
    return false;
  }

  c = view->data[*position];
  *position += 1U;

  switch (c) {
    case (unsigned char)'"':
    case (unsigned char)'\\':
    case (unsigned char)'/':
      *decoded = c;
      return true;
    case (unsigned char)'b':
      *decoded = 0x08U;
      return true;
    case (unsigned char)'f':
      *decoded = 0x0cU;
      return true;
    case (unsigned char)'n':
      *decoded = (unsigned char)'\n';
      return true;
    case (unsigned char)'r':
      *decoded = (unsigned char)'\r';
      return true;
    case (unsigned char)'t':
      *decoded = (unsigned char)'\t';
      return true;
    case (unsigned char)'u': {
      unsigned int value;
      size_t pos = *position;
      if (!parse_hex4(view->data, view->length, &pos, &value) ||
          value > 0x7fU ||
          (value >= 0xd800U && value <= 0xdfffU)) {
        return false;
      }
      *position = pos;
      *decoded = (unsigned char)value;
      return true;
    }
    default:
      return false;
  }
}

bool pervue_json_string_equals_ascii(
    const pervue_json_string_view_t *view,
    const char *ascii) {
  size_t input_position = 0U;
  size_t ascii_position = 0U;

  while (input_position < view->length) {
    unsigned char decoded;
    if (!decode_next_ascii(view, &input_position, &decoded)) {
      return false;
    }
    if (ascii[ascii_position] == '\0' ||
        decoded != (unsigned char)ascii[ascii_position]) {
      return false;
    }
    ascii_position += 1U;
  }

  return ascii[ascii_position] == '\0';
}

bool pervue_json_string_decode_ascii(
    const pervue_json_string_view_t *view,
    char *output,
    size_t output_capacity,
    size_t *output_length) {
  size_t input_position = 0U;
  size_t output_position = 0U;

  if (output == NULL || output_capacity == 0U) {
    return false;
  }

  while (input_position < view->length) {
    unsigned char decoded;

    if (!decode_next_ascii(view, &input_position, &decoded) ||
        decoded == 0U ||
        output_position + 1U >= output_capacity) {
      return false;
    }

    output[output_position] = (char)decoded;
    output_position += 1U;
  }

  output[output_position] = '\0';
  if (output_length != NULL) {
    *output_length = output_position;
  }
  return true;
}

bool pervue_json_string_is_nonempty(const pervue_json_string_view_t *view) {
  return view->length > 0U;
}
