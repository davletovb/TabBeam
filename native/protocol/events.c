#include "pervue/protocol.h"

#include "pervue/frame.h"
#include "pervue/version.h"

#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int write_json_frame(FILE *output, const char *json, size_t length) {
  return pervue_frame_write(
      output,
      (const unsigned char *)json,
      length) == PERVUE_FRAME_OK
      ? 0
      : 1;
}

static int append_bytes(
    char *buffer,
    size_t capacity,
    size_t *position,
    const void *data,
    size_t length) {
  if (*position > capacity || capacity - *position < length) {
    return 1;
  }

  memcpy(buffer + *position, data, length);
  *position += length;
  return 0;
}

static int append_cstr(
    char *buffer,
    size_t capacity,
    size_t *position,
    const char *text) {
  return append_bytes(buffer, capacity, position, text, strlen(text));
}

static int write_request_event(
    FILE *output,
    const pervue_raw_json_string_t *request_id,
    const char *event,
    const char *payload_json) {
  static const char prefix[] =
      "{\"version\":1,\"type\":\"event\",\"request_id\":\"";
  static const char middle[] = "\",\"event\":\"";
  static const char payload_prefix[] = "\",\"payload\":";
  static const char suffix[] = "}";
  size_t capacity;
  size_t position = 0U;
  char *buffer;
  int result = 1;

  if (output == NULL || request_id == NULL || event == NULL ||
      payload_json == NULL) {
    return 1;
  }

  capacity = sizeof(prefix) - 1U +
             request_id->length +
             sizeof(middle) - 1U +
             strlen(event) +
             sizeof(payload_prefix) - 1U +
             strlen(payload_json) +
             sizeof(suffix);

  buffer = (char *)malloc(capacity);
  if (buffer == NULL) {
    return 1;
  }

  if (append_cstr(buffer, capacity, &position, prefix) != 0 ||
      append_bytes(
          buffer,
          capacity,
          &position,
          request_id->data,
          request_id->length) != 0 ||
      append_cstr(buffer, capacity, &position, middle) != 0 ||
      append_cstr(buffer, capacity, &position, event) != 0 ||
      append_cstr(buffer, capacity, &position, payload_prefix) != 0 ||
      append_cstr(buffer, capacity, &position, payload_json) != 0 ||
      append_cstr(buffer, capacity, &position, suffix) != 0) {
    goto cleanup;
  }

  result = write_json_frame(output, buffer, position);

cleanup:
  free(buffer);
  return result;
}

int pervue_protocol_write_event(
    FILE *output,
    const pervue_raw_json_string_t *request_id,
    const char *event,
    const char *payload_json) {
  return write_request_event(output, request_id, event, payload_json);
}

int pervue_protocol_write_host_ready(FILE *output) {
  char buffer[512];
  int length = snprintf(
      buffer,
      sizeof(buffer),
      "{\"version\":1,\"type\":\"event\",\"request_id\":null,"
      "\"event\":\"host.ready\",\"payload\":{\"host_version\":\"%s\","
      "\"protocol_versions\":[1]}}",
      PERVUE_HOST_VERSION);

  if (length < 0 || (size_t)length >= sizeof(buffer)) {
    return 1;
  }

  return write_json_frame(output, buffer, (size_t)length);
}

static int write_uncorrelated_error(
    FILE *output,
    const char *reason,
    const char *message) {
  char buffer[768];
  int length = snprintf(
      buffer,
      sizeof(buffer),
      "{\"version\":1,\"type\":\"event\",\"request_id\":null,"
      "\"event\":\"response.failed\",\"payload\":{\"error\":{"
      "\"code\":\"INVALID_REQUEST\",\"reason\":\"%s\","
      "\"message\":\"%s\",\"retryable\":false}}}",
      reason,
      message);

  if (length < 0 || (size_t)length >= sizeof(buffer)) {
    return 1;
  }

  return write_json_frame(output, buffer, (size_t)length);
}

int pervue_protocol_write_error(
    FILE *output,
    const pervue_raw_json_string_t *request_id,
    const char *code,
    const char *reason,
    const char *message,
    bool retryable) {
  char payload[1024];
  int length;

  if (request_id == NULL) {
    return write_uncorrelated_error(output, reason, message);
  }

  length = snprintf(
      payload,
      sizeof(payload),
      "{\"error\":{\"code\":\"%s\",\"reason\":\"%s\","
      "\"message\":\"%s\",\"retryable\":%s}}",
      code,
      reason,
      message,
      retryable ? "true" : "false");

  if (length < 0 || (size_t)length >= sizeof(payload)) {
    return 1;
  }

  return write_request_event(
      output,
      request_id,
      "response.failed",
      payload);
}

int pervue_protocol_write_parse_failure(
    FILE *output,
    const pervue_request_failure_t *failure) {
  const pervue_raw_json_string_t *request_id;

  if (failure == NULL) {
    return 1;
  }

  request_id = failure->has_request_id ? &failure->request_id : NULL;

  switch (failure->result) {
    case PERVUE_REQUEST_PARSE_MALFORMED:
      return pervue_protocol_write_error(
          output,
          NULL,
          "INVALID_REQUEST",
          "MALFORMED_MESSAGE",
          "Malformed request.",
          false);

    case PERVUE_REQUEST_PARSE_INVALID_ENVELOPE:
      return pervue_protocol_write_error(
          output,
          request_id,
          "INVALID_REQUEST",
          "INVALID_ENVELOPE",
          "Invalid request envelope.",
          false);

    case PERVUE_REQUEST_PARSE_INVALID_PAYLOAD:
      return pervue_protocol_write_error(
          output,
          request_id,
          "INVALID_REQUEST",
          "INVALID_PAYLOAD",
          "Invalid request payload.",
          false);

    case PERVUE_REQUEST_PARSE_UNKNOWN_METHOD:
      return pervue_protocol_write_error(
          output,
          request_id,
          "INVALID_REQUEST",
          "UNKNOWN_METHOD",
          "Unsupported method.",
          false);

    case PERVUE_REQUEST_PARSE_UNSUPPORTED_VERSION: {
      char payload[1024];
      int length;

      if (request_id == NULL || !failure->has_received_version) {
        return pervue_protocol_write_error(
            output,
            request_id,
            "INVALID_REQUEST",
            "UNSUPPORTED_PROTOCOL_VERSION",
            "Unsupported protocol version.",
            false);
      }

      length = snprintf(
          payload,
          sizeof(payload),
          "{\"error\":{\"code\":\"INVALID_REQUEST\","
          "\"reason\":\"UNSUPPORTED_PROTOCOL_VERSION\","
          "\"message\":\"Unsupported protocol version.\","
          "\"retryable\":false},\"protocol\":{\"received_version\":%"
          PRId64 ",\"supported_versions\":[1]}}",
          failure->received_version);

      if (length < 0 || (size_t)length >= sizeof(payload)) {
        return 1;
      }

      return write_request_event(
          output,
          request_id,
          "response.failed",
          payload);
    }

    case PERVUE_REQUEST_PARSE_OK:
    default:
      return 1;
  }
}
