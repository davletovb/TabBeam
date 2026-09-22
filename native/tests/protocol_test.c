#include "pervue/frame.h"
#include "pervue/host.h"
#include "pervue/protocol.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define PERVUE_PROTOCOL_INPUT_PATH "pervue_protocol_input.tmp"
#define PERVUE_PROTOCOL_OUTPUT_PATH "pervue_protocol_output.tmp"

static FILE *open_test_file(const char *path, const char *mode) {
#ifdef _MSC_VER
  FILE *stream = NULL;

  if (fopen_s(&stream, path, mode) != 0) {
    return NULL;
  }
  return stream;
#else
  return fopen(path, mode);
#endif
}

static void close_and_remove(FILE **stream, const char *path) {
  if (*stream != NULL) {
    fclose(*stream);
    *stream = NULL;
  }
  remove(path);
}

static int expect_parse(
    const char *json,
    pervue_request_parse_result_t expected,
    bool expect_request_id) {
  pervue_request_t request;
  pervue_request_failure_t failure;
  pervue_request_parse_result_t result =
      pervue_request_parse(
          (const unsigned char *)json,
          strlen(json),
          &request,
          &failure);

  if (result != expected) {
    return 1;
  }

  if (expected != PERVUE_REQUEST_PARSE_OK &&
      failure.has_request_id != expect_request_id) {
    return 1;
  }

  return 0;
}

typedef struct route_probe {
  int conversation_calls;
  int status_calls;
  int cancel_calls;
} route_probe_t;

static int probe_conversation(
    void *context,
    FILE *output,
    const pervue_request_t *request) {
  route_probe_t *probe = (route_probe_t *)context;
  (void)output;

  if (!request->provider_is_fake) {
    return 1;
  }

  probe->conversation_calls += 1;
  return 0;
}

static int probe_status(
    void *context,
    FILE *output,
    const pervue_request_t *request) {
  route_probe_t *probe = (route_probe_t *)context;
  (void)output;
  (void)request;
  probe->status_calls += 1;
  return 0;
}

static int probe_cancel(
    void *context,
    FILE *output,
    const pervue_request_t *request) {
  route_probe_t *probe = (route_probe_t *)context;
  (void)output;
  (void)request;
  probe->cancel_calls += 1;
  return 0;
}

static int test_parser_cases(void) {
  static const char valid_conversation[] =
      "{\"version\":1,\"type\":\"request\",\"request_id\":\"req_1\","
      "\"method\":\"conversation.send\",\"payload\":{"
      "\"provider_id\":\"fake\",\"input\":{\"text\":\"Hello\"}}}";
  static const char missing_payload[] =
      "{\"version\":1,\"type\":\"request\",\"request_id\":\"req_2\","
      "\"method\":\"provider.status\"}";
  static const char extra_field[] =
      "{\"version\":1,\"type\":\"request\",\"request_id\":\"req_3\","
      "\"method\":\"provider.status\",\"payload\":{},\"extra\":true}";
  static const char bad_payload_type[] =
      "{\"version\":1,\"type\":\"request\",\"request_id\":\"req_4\","
      "\"method\":\"provider.status\",\"payload\":[]}";
  static const char unsupported_version[] =
      "{\"version\":2,\"type\":\"request\",\"request_id\":\"req_5\","
      "\"method\":\"provider.status\",\"payload\":{}}";
  static const char unknown_method[] =
      "{\"version\":1,\"type\":\"request\",\"request_id\":\"req_6\","
      "\"method\":\"unknown.method\",\"payload\":{}}";
  static const char invalid_conversation[] =
      "{\"version\":1,\"type\":\"request\",\"request_id\":\"req_7\","
      "\"method\":\"conversation.send\",\"payload\":{"
      "\"provider_id\":\"fake\"}}";
  static const char invalid_cancel[] =
      "{\"version\":1,\"type\":\"request\",\"request_id\":\"req_8\","
      "\"method\":\"request.cancel\",\"payload\":{"
      "\"target_request_id\":\"bad id\"}}";

  if (expect_parse(
          "{not-json",
          PERVUE_REQUEST_PARSE_MALFORMED,
          false) != 0 ||
      expect_parse(
          valid_conversation,
          PERVUE_REQUEST_PARSE_OK,
          false) != 0 ||
      expect_parse(
          missing_payload,
          PERVUE_REQUEST_PARSE_INVALID_ENVELOPE,
          true) != 0 ||
      expect_parse(
          extra_field,
          PERVUE_REQUEST_PARSE_INVALID_ENVELOPE,
          true) != 0 ||
      expect_parse(
          bad_payload_type,
          PERVUE_REQUEST_PARSE_INVALID_ENVELOPE,
          true) != 0 ||
      expect_parse(
          unsupported_version,
          PERVUE_REQUEST_PARSE_UNSUPPORTED_VERSION,
          true) != 0 ||
      expect_parse(
          unknown_method,
          PERVUE_REQUEST_PARSE_UNKNOWN_METHOD,
          true) != 0 ||
      expect_parse(
          invalid_conversation,
          PERVUE_REQUEST_PARSE_INVALID_PAYLOAD,
          true) != 0 ||
      expect_parse(
          invalid_cancel,
          PERVUE_REQUEST_PARSE_INVALID_PAYLOAD,
          true) != 0) {
    return 1;
  }

  return 0;
}

static int test_router_dispatch(void) {
  static const char json[] =
      "{\"version\":1,\"type\":\"request\",\"request_id\":\"req_route\","
      "\"method\":\"conversation.send\",\"payload\":{"
      "\"provider_id\":\"fake\",\"input\":{\"text\":\"Hello\"}}}";
  static const pervue_router_handlers_t handlers = {
    probe_conversation,
    probe_status,
    probe_cancel
  };
  pervue_request_t request;
  pervue_request_failure_t failure;
  route_probe_t probe = {0, 0, 0};
  FILE *sink = open_test_file(PERVUE_PROTOCOL_OUTPUT_PATH, "w+b");

  if (sink == NULL) {
    return 1;
  }

  if (pervue_request_parse(
          (const unsigned char *)json,
          strlen(json),
          &request,
          &failure) != PERVUE_REQUEST_PARSE_OK ||
      pervue_router_dispatch(
          &handlers,
          &probe,
          sink,
          &request) != 0 ||
      probe.conversation_calls != 1 ||
      probe.status_calls != 0 ||
      probe.cancel_calls != 0) {
    close_and_remove(&sink, PERVUE_PROTOCOL_OUTPUT_PATH);
    return 1;
  }

  close_and_remove(&sink, PERVUE_PROTOCOL_OUTPUT_PATH);
  return 0;
}

static int frame_contains(
    FILE *stream,
    const char *needle,
    const char *request_id_fragment) {
  pervue_frame_t frame;
  char *text;
  int result = 1;

  pervue_frame_init(&frame);

  if (pervue_frame_read(stream, &frame) != PERVUE_FRAME_OK) {
    goto cleanup;
  }

  text = (char *)malloc(frame.length + 1U);
  if (text == NULL) {
    goto cleanup;
  }

  memcpy(text, frame.data, frame.length);
  text[frame.length] = '\0';

  if (strstr(text, needle) != NULL &&
      (request_id_fragment == NULL ||
       strstr(text, request_id_fragment) != NULL)) {
    result = 0;
  }

  free(text);

cleanup:
  pervue_frame_destroy(&frame);
  return result;
}

static int test_host_protocol_flow(void) {
  static const char malformed[] = "{not-json";
  static const char valid[] =
      "{\"version\":1,\"type\":\"request\",\"request_id\":\"req_flow\","
      "\"method\":\"conversation.send\",\"payload\":{"
      "\"provider_id\":\"fake\",\"input\":{\"text\":\"Hello\"}}}";
  static const char unsupported[] =
      "{\"version\":2,\"type\":\"request\","
      "\"request_id\":\"r\\u0065q_v2\","
      "\"method\":\"provider.status\",\"payload\":{}}";
  FILE *input = open_test_file(PERVUE_PROTOCOL_INPUT_PATH, "w+b");
  FILE *output = open_test_file(PERVUE_PROTOCOL_OUTPUT_PATH, "w+b");
  pervue_frame_t trailing;
  int result = 1;

  if (input == NULL || output == NULL) {
    goto cleanup;
  }

  if (pervue_frame_write(
          input,
          (const unsigned char *)malformed,
          strlen(malformed)) != PERVUE_FRAME_OK ||
      pervue_frame_write(
          input,
          (const unsigned char *)valid,
          strlen(valid)) != PERVUE_FRAME_OK ||
      pervue_frame_write(
          input,
          (const unsigned char *)unsupported,
          strlen(unsupported)) != PERVUE_FRAME_OK ||
      fseek(input, 0L, SEEK_SET) != 0) {
    goto cleanup;
  }

  if (pervue_host_run(input, output) != PERVUE_HOST_OK ||
      fseek(output, 0L, SEEK_SET) != 0) {
    goto cleanup;
  }

  if (frame_contains(output, "\"event\":\"host.ready\"", NULL) != 0 ||
      frame_contains(
          output,
          "\"reason\":\"MALFORMED_MESSAGE\"",
          "\"request_id\":null") != 0 ||
      frame_contains(
          output,
          "\"event\":\"conversation.created\"",
          "\"request_id\":\"req_flow\"") != 0 ||
      frame_contains(
          output,
          "\"event\":\"response.started\"",
          "\"request_id\":\"req_flow\"") != 0 ||
      frame_contains(
          output,
          "\"event\":\"response.delta\"",
          "\"request_id\":\"req_flow\"") != 0 ||
      frame_contains(
          output,
          "\"event\":\"response.completed\"",
          "\"request_id\":\"req_flow\"") != 0 ||
      frame_contains(
          output,
          "\"reason\":\"UNSUPPORTED_PROTOCOL_VERSION\"",
          "\"request_id\":\"r\\u0065q_v2\"") != 0) {
    goto cleanup;
  }

  pervue_frame_init(&trailing);
  if (pervue_frame_read(output, &trailing) != PERVUE_FRAME_EOF) {
    pervue_frame_destroy(&trailing);
    goto cleanup;
  }
  pervue_frame_destroy(&trailing);

  result = 0;

cleanup:
  close_and_remove(&input, PERVUE_PROTOCOL_INPUT_PATH);
  close_and_remove(&output, PERVUE_PROTOCOL_OUTPUT_PATH);
  return result;
}

int main(void) {
  if (test_parser_cases() != 0) {
    fprintf(stderr, "protocol parser cases failed\n");
    return 1;
  }

  if (test_router_dispatch() != 0) {
    fprintf(stderr, "router dispatch test failed\n");
    return 1;
  }

  if (test_host_protocol_flow() != 0) {
    fprintf(stderr, "host protocol flow test failed\n");
    return 1;
  }

  return 0;
}
