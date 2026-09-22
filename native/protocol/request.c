#include "pervue/protocol.h"

#include "json_internal.h"

#include <ctype.h>
#include <string.h>

typedef struct payload_span {
  const unsigned char *data;
  size_t length;
} payload_span_t;

static pervue_raw_json_string_t raw_string(
    const pervue_json_string_view_t *view) {
  pervue_raw_json_string_t result;
  result.data = view->data;
  result.length = view->length;
  return result;
}

static bool request_id_is_valid(const pervue_json_string_view_t *view) {
  char decoded[PERVUE_REQUEST_ID_MAX_LENGTH + 1U];
  size_t length;
  size_t index;

  if (!pervue_json_string_decode_ascii(
          view,
          decoded,
          sizeof(decoded),
          &length) ||
      length == 0U ||
      length > PERVUE_REQUEST_ID_MAX_LENGTH) {
    return false;
  }

  if (!isalnum((unsigned char)decoded[0])) {
    return false;
  }

  for (index = 1U; index < length; ++index) {
    unsigned char c = (unsigned char)decoded[index];
    if (!isalnum(c) &&
        c != (unsigned char)'.' &&
        c != (unsigned char)'_' &&
        c != (unsigned char)':' &&
        c != (unsigned char)'-') {
      return false;
    }
  }

  return true;
}

static pervue_request_parse_result_t json_result_to_request_result(
    pervue_json_result_t result) {
  if (result == PERVUE_JSON_DEPTH_EXCEEDED) {
    return PERVUE_REQUEST_PARSE_INVALID_PAYLOAD;
  }
  return PERVUE_REQUEST_PARSE_MALFORMED;
}

static bool object_value_is_object(pervue_json_reader_t *reader) {
  pervue_json_skip_whitespace(reader);
  return pervue_json_peek(reader) == (int)'{';
}

static pervue_request_parse_result_t parse_input_payload(
    const unsigned char *data,
    size_t length) {
  pervue_json_reader_t reader;
  bool saw_text = false;
  bool duplicate = false;

  pervue_json_reader_init(&reader, data, length);
  pervue_json_skip_whitespace(&reader);

  if (!pervue_json_consume(&reader, (unsigned char)'{')) {
    return PERVUE_REQUEST_PARSE_INVALID_PAYLOAD;
  }

  pervue_json_skip_whitespace(&reader);
  if (pervue_json_consume(&reader, (unsigned char)'}')) {
    return PERVUE_REQUEST_PARSE_INVALID_PAYLOAD;
  }

  for (;;) {
    pervue_json_string_view_t key;
    pervue_json_result_t result;

    result = pervue_json_parse_string(&reader, &key);
    if (result != PERVUE_JSON_OK) {
      return json_result_to_request_result(result);
    }

    pervue_json_skip_whitespace(&reader);
    if (!pervue_json_consume(&reader, (unsigned char)':')) {
      return PERVUE_REQUEST_PARSE_MALFORMED;
    }
    pervue_json_skip_whitespace(&reader);

    if (pervue_json_string_equals_ascii(&key, "text")) {
      pervue_json_string_view_t text;
      if (saw_text) {
        duplicate = true;
      }
      saw_text = true;

      if (pervue_json_peek(&reader) != (int)'"') {
        result = pervue_json_skip_value(&reader);
        if (result != PERVUE_JSON_OK) {
          return json_result_to_request_result(result);
        }
        duplicate = true;
      } else {
        result = pervue_json_parse_string(&reader, &text);
        if (result != PERVUE_JSON_OK) {
          return json_result_to_request_result(result);
        }
        if (!pervue_json_string_is_nonempty(&text)) {
          duplicate = true;
        }
      }
    } else {
      result = pervue_json_skip_value(&reader);
      if (result != PERVUE_JSON_OK) {
        return json_result_to_request_result(result);
      }
    }

    pervue_json_skip_whitespace(&reader);
    if (pervue_json_consume(&reader, (unsigned char)'}')) {
      break;
    }
    if (!pervue_json_consume(&reader, (unsigned char)',')) {
      return PERVUE_REQUEST_PARSE_MALFORMED;
    }
    pervue_json_skip_whitespace(&reader);
  }

  pervue_json_skip_whitespace(&reader);
  if (!pervue_json_at_end(&reader)) {
    return PERVUE_REQUEST_PARSE_MALFORMED;
  }

  if (!saw_text || duplicate) {
    return PERVUE_REQUEST_PARSE_INVALID_PAYLOAD;
  }

  return PERVUE_REQUEST_PARSE_OK;
}

static pervue_request_parse_result_t parse_conversation_payload(
    const payload_span_t *payload,
    pervue_request_t *request) {
  pervue_json_reader_t reader;
  bool saw_provider = false;
  bool saw_input = false;
  bool saw_conversation = false;
  bool saw_context = false;
  bool invalid = false;

  pervue_json_reader_init(&reader, payload->data, payload->length);
  pervue_json_skip_whitespace(&reader);

  if (!pervue_json_consume(&reader, (unsigned char)'{')) {
    return PERVUE_REQUEST_PARSE_INVALID_PAYLOAD;
  }

  pervue_json_skip_whitespace(&reader);
  if (pervue_json_consume(&reader, (unsigned char)'}')) {
    return PERVUE_REQUEST_PARSE_INVALID_PAYLOAD;
  }

  for (;;) {
    pervue_json_string_view_t key;
    pervue_json_result_t result;

    result = pervue_json_parse_string(&reader, &key);
    if (result != PERVUE_JSON_OK) {
      return json_result_to_request_result(result);
    }

    pervue_json_skip_whitespace(&reader);
    if (!pervue_json_consume(&reader, (unsigned char)':')) {
      return PERVUE_REQUEST_PARSE_MALFORMED;
    }
    pervue_json_skip_whitespace(&reader);

    if (pervue_json_string_equals_ascii(&key, "provider_id")) {
      pervue_json_string_view_t value;
      if (saw_provider) {
        invalid = true;
      }
      saw_provider = true;

      if (pervue_json_peek(&reader) != (int)'"') {
        result = pervue_json_skip_value(&reader);
        if (result != PERVUE_JSON_OK) {
          return json_result_to_request_result(result);
        }
        invalid = true;
      } else {
        result = pervue_json_parse_string(&reader, &value);
        if (result != PERVUE_JSON_OK) {
          return json_result_to_request_result(result);
        }
        if (!pervue_json_string_is_nonempty(&value)) {
          invalid = true;
        } else {
          request->has_provider_id = true;
          request->provider_id = raw_string(&value);
          request->provider_is_fake =
              pervue_json_string_equals_ascii(&value, "fake");
        }
      }
    } else if (pervue_json_string_equals_ascii(&key, "input")) {
      size_t start;
      size_t end;

      if (saw_input) {
        invalid = true;
      }
      saw_input = true;

      if (!object_value_is_object(&reader)) {
        result = pervue_json_skip_value(&reader);
        if (result != PERVUE_JSON_OK) {
          return json_result_to_request_result(result);
        }
        invalid = true;
      } else {
        start = reader.position;
        result = pervue_json_skip_value(&reader);
        end = reader.position;
        if (result != PERVUE_JSON_OK) {
          return json_result_to_request_result(result);
        }

        if (parse_input_payload(
                payload->data + start,
                end - start) != PERVUE_REQUEST_PARSE_OK) {
          invalid = true;
        }
      }
    } else if (pervue_json_string_equals_ascii(&key, "conversation_id")) {
      pervue_json_string_view_t value;
      if (saw_conversation) {
        invalid = true;
      }
      saw_conversation = true;

      if (pervue_json_peek(&reader) != (int)'"') {
        result = pervue_json_skip_value(&reader);
        if (result != PERVUE_JSON_OK) {
          return json_result_to_request_result(result);
        }
        invalid = true;
      } else {
        result = pervue_json_parse_string(&reader, &value);
        if (result != PERVUE_JSON_OK) {
          return json_result_to_request_result(result);
        }
        if (!pervue_json_string_is_nonempty(&value)) {
          invalid = true;
        } else {
          request->has_conversation_id = true;
        }
      }
    } else if (pervue_json_string_equals_ascii(&key, "context")) {
      if (saw_context) {
        invalid = true;
      }
      saw_context = true;

      if (!object_value_is_object(&reader)) {
        result = pervue_json_skip_value(&reader);
        if (result != PERVUE_JSON_OK) {
          return json_result_to_request_result(result);
        }
        invalid = true;
      } else {
        result = pervue_json_skip_value(&reader);
        if (result != PERVUE_JSON_OK) {
          return json_result_to_request_result(result);
        }
      }
    } else {
      result = pervue_json_skip_value(&reader);
      if (result != PERVUE_JSON_OK) {
        return json_result_to_request_result(result);
      }
    }

    pervue_json_skip_whitespace(&reader);
    if (pervue_json_consume(&reader, (unsigned char)'}')) {
      break;
    }
    if (!pervue_json_consume(&reader, (unsigned char)',')) {
      return PERVUE_REQUEST_PARSE_MALFORMED;
    }
    pervue_json_skip_whitespace(&reader);
  }

  if (!saw_provider || !saw_input || invalid) {
    return PERVUE_REQUEST_PARSE_INVALID_PAYLOAD;
  }

  return PERVUE_REQUEST_PARSE_OK;
}

static pervue_request_parse_result_t parse_status_payload(
    const payload_span_t *payload,
    pervue_request_t *request) {
  pervue_json_reader_t reader;
  bool saw_provider = false;
  bool invalid = false;

  pervue_json_reader_init(&reader, payload->data, payload->length);
  pervue_json_skip_whitespace(&reader);

  if (!pervue_json_consume(&reader, (unsigned char)'{')) {
    return PERVUE_REQUEST_PARSE_INVALID_PAYLOAD;
  }

  pervue_json_skip_whitespace(&reader);
  if (pervue_json_consume(&reader, (unsigned char)'}')) {
    return PERVUE_REQUEST_PARSE_OK;
  }

  for (;;) {
    pervue_json_string_view_t key;
    pervue_json_result_t result;

    result = pervue_json_parse_string(&reader, &key);
    if (result != PERVUE_JSON_OK) {
      return json_result_to_request_result(result);
    }

    pervue_json_skip_whitespace(&reader);
    if (!pervue_json_consume(&reader, (unsigned char)':')) {
      return PERVUE_REQUEST_PARSE_MALFORMED;
    }
    pervue_json_skip_whitespace(&reader);

    if (pervue_json_string_equals_ascii(&key, "provider_id")) {
      pervue_json_string_view_t value;

      if (saw_provider) {
        invalid = true;
      }
      saw_provider = true;

      if (pervue_json_peek(&reader) != (int)'"') {
        result = pervue_json_skip_value(&reader);
        if (result != PERVUE_JSON_OK) {
          return json_result_to_request_result(result);
        }
        invalid = true;
      } else {
        result = pervue_json_parse_string(&reader, &value);
        if (result != PERVUE_JSON_OK) {
          return json_result_to_request_result(result);
        }

        if (!pervue_json_string_is_nonempty(&value)) {
          invalid = true;
        } else {
          request->has_provider_id = true;
          request->provider_id = raw_string(&value);
          request->provider_is_fake =
              pervue_json_string_equals_ascii(&value, "fake");
        }
      }
    } else {
      result = pervue_json_skip_value(&reader);
      if (result != PERVUE_JSON_OK) {
        return json_result_to_request_result(result);
      }
    }

    pervue_json_skip_whitespace(&reader);
    if (pervue_json_consume(&reader, (unsigned char)'}')) {
      break;
    }
    if (!pervue_json_consume(&reader, (unsigned char)',')) {
      return PERVUE_REQUEST_PARSE_MALFORMED;
    }
    pervue_json_skip_whitespace(&reader);
  }

  return invalid
      ? PERVUE_REQUEST_PARSE_INVALID_PAYLOAD
      : PERVUE_REQUEST_PARSE_OK;
}

static pervue_request_parse_result_t parse_cancel_payload(
    const payload_span_t *payload) {
  pervue_json_reader_t reader;
  bool saw_target = false;
  bool invalid = false;

  pervue_json_reader_init(&reader, payload->data, payload->length);
  pervue_json_skip_whitespace(&reader);

  if (!pervue_json_consume(&reader, (unsigned char)'{')) {
    return PERVUE_REQUEST_PARSE_INVALID_PAYLOAD;
  }

  pervue_json_skip_whitespace(&reader);
  if (pervue_json_consume(&reader, (unsigned char)'}')) {
    return PERVUE_REQUEST_PARSE_INVALID_PAYLOAD;
  }

  for (;;) {
    pervue_json_string_view_t key;
    pervue_json_result_t result;

    result = pervue_json_parse_string(&reader, &key);
    if (result != PERVUE_JSON_OK) {
      return json_result_to_request_result(result);
    }

    pervue_json_skip_whitespace(&reader);
    if (!pervue_json_consume(&reader, (unsigned char)':')) {
      return PERVUE_REQUEST_PARSE_MALFORMED;
    }
    pervue_json_skip_whitespace(&reader);

    if (pervue_json_string_equals_ascii(&key, "target_request_id")) {
      pervue_json_string_view_t value;

      if (saw_target) {
        invalid = true;
      }
      saw_target = true;

      if (pervue_json_peek(&reader) != (int)'"') {
        result = pervue_json_skip_value(&reader);
        if (result != PERVUE_JSON_OK) {
          return json_result_to_request_result(result);
        }
        invalid = true;
      } else {
        result = pervue_json_parse_string(&reader, &value);
        if (result != PERVUE_JSON_OK) {
          return json_result_to_request_result(result);
        }
        if (!request_id_is_valid(&value)) {
          invalid = true;
        }
      }
    } else {
      result = pervue_json_skip_value(&reader);
      if (result != PERVUE_JSON_OK) {
        return json_result_to_request_result(result);
      }
    }

    pervue_json_skip_whitespace(&reader);
    if (pervue_json_consume(&reader, (unsigned char)'}')) {
      break;
    }
    if (!pervue_json_consume(&reader, (unsigned char)',')) {
      return PERVUE_REQUEST_PARSE_MALFORMED;
    }
    pervue_json_skip_whitespace(&reader);
  }

  if (!saw_target || invalid) {
    return PERVUE_REQUEST_PARSE_INVALID_PAYLOAD;
  }

  return PERVUE_REQUEST_PARSE_OK;
}

pervue_request_parse_result_t pervue_request_parse(
    const unsigned char *data,
    size_t length,
    pervue_request_t *request,
    pervue_request_failure_t *failure) {
  pervue_json_reader_t reader;
  payload_span_t payload = {NULL, 0U};
  bool saw_version = false;
  bool saw_type = false;
  bool saw_request_id = false;
  bool saw_method = false;
  bool saw_payload = false;
  bool invalid_envelope = false;
  bool version_is_integer = false;
  int64_t version = 0;
  bool method_known = false;
  pervue_json_string_view_t request_id_view = {NULL, 0U};

  if (request == NULL || failure == NULL || (data == NULL && length != 0U)) {
    return PERVUE_REQUEST_PARSE_MALFORMED;
  }

  memset(request, 0, sizeof(*request));
  memset(failure, 0, sizeof(*failure));
  failure->result = PERVUE_REQUEST_PARSE_MALFORMED;

  pervue_json_reader_init(&reader, data, length);
  pervue_json_skip_whitespace(&reader);

  if (pervue_json_peek(&reader) != (int)'{') {
    pervue_json_result_t root_result = pervue_json_skip_value(&reader);

    if (root_result == PERVUE_JSON_SYNTAX_ERROR) {
      return PERVUE_REQUEST_PARSE_MALFORMED;
    }

    pervue_json_skip_whitespace(&reader);
    if (!pervue_json_at_end(&reader)) {
      return PERVUE_REQUEST_PARSE_MALFORMED;
    }

    failure->result = PERVUE_REQUEST_PARSE_INVALID_ENVELOPE;
    return failure->result;
  }

  (void)pervue_json_consume(&reader, (unsigned char)'{');

  pervue_json_skip_whitespace(&reader);
  if (pervue_json_consume(&reader, (unsigned char)'}')) {
    failure->result = PERVUE_REQUEST_PARSE_INVALID_ENVELOPE;
    return failure->result;
  }

  for (;;) {
    pervue_json_string_view_t key;
    pervue_json_result_t json_result;

    json_result = pervue_json_parse_string(&reader, &key);
    if (json_result != PERVUE_JSON_OK) {
      failure->result = json_result_to_request_result(json_result);
      return failure->result;
    }

    pervue_json_skip_whitespace(&reader);
    if (!pervue_json_consume(&reader, (unsigned char)':')) {
      return PERVUE_REQUEST_PARSE_MALFORMED;
    }
    pervue_json_skip_whitespace(&reader);

    if (pervue_json_string_equals_ascii(&key, "version")) {
      bool integer = false;

      if (saw_version) {
        invalid_envelope = true;
      }
      saw_version = true;

      if (pervue_json_peek(&reader) != (int)'-' &&
          (pervue_json_peek(&reader) < (int)'0' ||
           pervue_json_peek(&reader) > (int)'9')) {
        json_result = pervue_json_skip_value(&reader);
        if (json_result != PERVUE_JSON_OK) {
          failure->result = json_result_to_request_result(json_result);
          return failure->result;
        }
        invalid_envelope = true;
      } else {
        json_result = pervue_json_parse_integer(&reader, &version, &integer);
        if (json_result != PERVUE_JSON_OK) {
          failure->result = json_result_to_request_result(json_result);
          return failure->result;
        }
        version_is_integer = integer;
        if (!integer) {
          invalid_envelope = true;
        }
      }
    } else if (pervue_json_string_equals_ascii(&key, "type")) {
      pervue_json_string_view_t value;

      if (saw_type) {
        invalid_envelope = true;
      }
      saw_type = true;

      if (pervue_json_peek(&reader) != (int)'"') {
        json_result = pervue_json_skip_value(&reader);
        if (json_result != PERVUE_JSON_OK) {
          failure->result = json_result_to_request_result(json_result);
          return failure->result;
        }
        invalid_envelope = true;
      } else {
        json_result = pervue_json_parse_string(&reader, &value);
        if (json_result != PERVUE_JSON_OK) {
          failure->result = json_result_to_request_result(json_result);
          return failure->result;
        }
        if (!pervue_json_string_equals_ascii(&value, "request")) {
          invalid_envelope = true;
        }
      }
    } else if (pervue_json_string_equals_ascii(&key, "request_id")) {
      pervue_json_string_view_t value;

      if (saw_request_id) {
        invalid_envelope = true;
      }
      saw_request_id = true;

      if (pervue_json_peek(&reader) != (int)'"') {
        json_result = pervue_json_skip_value(&reader);
        if (json_result != PERVUE_JSON_OK) {
          failure->result = json_result_to_request_result(json_result);
          return failure->result;
        }
        invalid_envelope = true;
      } else {
        json_result = pervue_json_parse_string(&reader, &value);
        if (json_result != PERVUE_JSON_OK) {
          failure->result = json_result_to_request_result(json_result);
          return failure->result;
        }

        if (!request_id_is_valid(&value)) {
          invalid_envelope = true;
        } else {
          request_id_view = value;
          failure->has_request_id = true;
          failure->request_id = raw_string(&value);
        }
      }
    } else if (pervue_json_string_equals_ascii(&key, "method")) {
      pervue_json_string_view_t value;

      if (saw_method) {
        invalid_envelope = true;
      }
      saw_method = true;

      if (pervue_json_peek(&reader) != (int)'"') {
        json_result = pervue_json_skip_value(&reader);
        if (json_result != PERVUE_JSON_OK) {
          failure->result = json_result_to_request_result(json_result);
          return failure->result;
        }
        invalid_envelope = true;
      } else {
        json_result = pervue_json_parse_string(&reader, &value);
        if (json_result != PERVUE_JSON_OK) {
          failure->result = json_result_to_request_result(json_result);
          return failure->result;
        }

        if (pervue_json_string_equals_ascii(&value, "conversation.send")) {
          request->method = PERVUE_METHOD_CONVERSATION_SEND;
          method_known = true;
        } else if (pervue_json_string_equals_ascii(&value, "provider.status")) {
          request->method = PERVUE_METHOD_PROVIDER_STATUS;
          method_known = true;
        } else if (pervue_json_string_equals_ascii(&value, "request.cancel")) {
          request->method = PERVUE_METHOD_REQUEST_CANCEL;
          method_known = true;
        }
      }
    } else if (pervue_json_string_equals_ascii(&key, "payload")) {
      size_t start;

      if (saw_payload) {
        invalid_envelope = true;
      }
      saw_payload = true;

      if (!object_value_is_object(&reader)) {
        json_result = pervue_json_skip_value(&reader);
        if (json_result != PERVUE_JSON_OK) {
          failure->result = json_result_to_request_result(json_result);
          return failure->result;
        }
        invalid_envelope = true;
      } else {
        start = reader.position;
        json_result = pervue_json_skip_value(&reader);
        if (json_result != PERVUE_JSON_OK) {
          failure->result = json_result_to_request_result(json_result);
          return failure->result;
        }
        payload.data = data + start;
        payload.length = reader.position - start;
      }
    } else {
      invalid_envelope = true;
      json_result = pervue_json_skip_value(&reader);
      if (json_result != PERVUE_JSON_OK) {
        failure->result = json_result_to_request_result(json_result);
        return failure->result;
      }
    }

    pervue_json_skip_whitespace(&reader);
    if (pervue_json_consume(&reader, (unsigned char)'}')) {
      break;
    }
    if (!pervue_json_consume(&reader, (unsigned char)',')) {
      return PERVUE_REQUEST_PARSE_MALFORMED;
    }
    pervue_json_skip_whitespace(&reader);
  }

  pervue_json_skip_whitespace(&reader);
  if (!pervue_json_at_end(&reader)) {
    return PERVUE_REQUEST_PARSE_MALFORMED;
  }

  if (!saw_version || !saw_type || !saw_request_id || !saw_method ||
      !saw_payload || invalid_envelope) {
    failure->result = PERVUE_REQUEST_PARSE_INVALID_ENVELOPE;
    return failure->result;
  }

  if (!version_is_integer || version != PERVUE_PROTOCOL_VERSION) {
    failure->result = PERVUE_REQUEST_PARSE_UNSUPPORTED_VERSION;
    failure->has_received_version = version_is_integer;
    failure->received_version = version;
    return failure->result;
  }

  if (!method_known) {
    failure->result = PERVUE_REQUEST_PARSE_UNKNOWN_METHOD;
    return failure->result;
  }

  request->request_id = raw_string(&request_id_view);

  switch (request->method) {
    case PERVUE_METHOD_CONVERSATION_SEND:
      failure->result = parse_conversation_payload(&payload, request);
      break;
    case PERVUE_METHOD_PROVIDER_STATUS:
      failure->result = parse_status_payload(&payload, request);
      break;
    case PERVUE_METHOD_REQUEST_CANCEL:
      failure->result = parse_cancel_payload(&payload);
      break;
    default:
      failure->result = PERVUE_REQUEST_PARSE_UNKNOWN_METHOD;
      break;
  }

  return failure->result;
}
