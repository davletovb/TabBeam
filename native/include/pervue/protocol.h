#ifndef PERVUE_PROTOCOL_H
#define PERVUE_PROTOCOL_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>

#define PERVUE_PROTOCOL_VERSION 1
#define PERVUE_REQUEST_ID_MAX_LENGTH 128U

typedef enum pervue_method {
  PERVUE_METHOD_CONVERSATION_SEND = 0,
  PERVUE_METHOD_PROVIDER_STATUS = 1,
  PERVUE_METHOD_REQUEST_CANCEL = 2
} pervue_method_t;

typedef struct pervue_raw_json_string {
  const unsigned char *data;
  size_t length;
} pervue_raw_json_string_t;

typedef struct pervue_request {
  pervue_method_t method;
  /*
   * Raw JSON string bytes are intentionally preserved for byte-for-byte
   * correlation on emitted events. Re-emission is safe only because
   * request_id_is_valid() restricts the decoded ID to the protocol's ASCII
   * [A-Za-z0-9][A-Za-z0-9._:-]* grammar before this structure is produced.
   */
  pervue_raw_json_string_t request_id;
  bool provider_is_fake;
  bool has_provider_id;
  pervue_raw_json_string_t provider_id;
  bool has_conversation_id;
} pervue_request_t;

typedef enum pervue_request_parse_result {
  PERVUE_REQUEST_PARSE_OK = 0,
  PERVUE_REQUEST_PARSE_MALFORMED = 1,
  PERVUE_REQUEST_PARSE_INVALID_ENVELOPE = 2,
  PERVUE_REQUEST_PARSE_INVALID_PAYLOAD = 3,
  PERVUE_REQUEST_PARSE_UNKNOWN_METHOD = 4,
  PERVUE_REQUEST_PARSE_UNSUPPORTED_VERSION = 5
} pervue_request_parse_result_t;

typedef struct pervue_request_failure {
  pervue_request_parse_result_t result;
  bool has_request_id;
  pervue_raw_json_string_t request_id;
  bool has_received_version;
  int64_t received_version;
} pervue_request_failure_t;

typedef int (*pervue_route_handler_t)(
    void *context,
    FILE *output,
    const pervue_request_t *request);

typedef struct pervue_router_handlers {
  pervue_route_handler_t conversation_send;
  pervue_route_handler_t provider_status;
  pervue_route_handler_t request_cancel;
} pervue_router_handlers_t;

pervue_request_parse_result_t pervue_request_parse(
    const unsigned char *data,
    size_t length,
    pervue_request_t *request,
    pervue_request_failure_t *failure);

int pervue_router_dispatch(
    const pervue_router_handlers_t *handlers,
    void *context,
    FILE *output,
    const pervue_request_t *request);

int pervue_protocol_write_host_ready(FILE *output);
int pervue_protocol_write_parse_failure(
    FILE *output,
    const pervue_request_failure_t *failure);
int pervue_protocol_write_error(
    FILE *output,
    const pervue_raw_json_string_t *request_id,
    const char *code,
    const char *reason,
    const char *message,
    bool retryable);
int pervue_protocol_write_event(
    FILE *output,
    const pervue_raw_json_string_t *request_id,
    const char *event,
    const char *payload_json);

#endif
