#include "pervue/host.h"

#include "pervue/frame.h"
#include "pervue/protocol.h"

static pervue_host_result_t map_frame_error(pervue_frame_result_t result) {
  switch (result) {
    case PERVUE_FRAME_INVALID_ARGUMENT:
      return PERVUE_HOST_INVALID_ARGUMENT;
    case PERVUE_FRAME_IO_ERROR:
      return PERVUE_HOST_IO_ERROR;
    case PERVUE_FRAME_TRUNCATED:
      return PERVUE_HOST_FRAME_TRUNCATED;
    case PERVUE_FRAME_TOO_LARGE:
      return PERVUE_HOST_FRAME_TOO_LARGE;
    case PERVUE_FRAME_ALLOCATION_FAILED:
      return PERVUE_HOST_ALLOCATION_FAILED;
    case PERVUE_FRAME_OK:
    case PERVUE_FRAME_EOF:
    default:
      return PERVUE_HOST_IO_ERROR;
  }
}

static int handle_conversation_send(
    void *context,
    FILE *output,
    const pervue_request_t *request) {
  (void)context;

  if (!request->provider_is_fake) {
    return pervue_protocol_write_error(
        output,
        &request->request_id,
        "PROVIDER_NOT_FOUND",
        "PROVIDER_NOT_INSTALLED",
        "The selected provider runtime is not installed.",
        false);
  }

  if (!request->has_conversation_id &&
      pervue_protocol_write_event(
          output,
          &request->request_id,
          "conversation.created",
          "{\"conversation_id\":\"fake-conversation\"}") != 0) {
    return 1;
  }

  if (pervue_protocol_write_event(
          output,
          &request->request_id,
          "response.started",
          "{\"provider_id\":\"fake\"}") != 0) {
    return 1;
  }

  if (pervue_protocol_write_event(
          output,
          &request->request_id,
          "response.delta",
          "{\"text\":\"Fake provider response.\"}") != 0) {
    return 1;
  }

  return pervue_protocol_write_event(
      output,
      &request->request_id,
      "response.completed",
      "{}");
}

static int handle_provider_status(
    void *context,
    FILE *output,
    const pervue_request_t *request) {
  static const char fake_status[] =
      "{\"provider_id\":\"fake\",\"status\":{"
      "\"availability\":\"available\","
      "\"authentication\":\"authenticated\","
      "\"capabilities\":{"
      "\"streaming\":true,"
      "\"continuation\":true,"
      "\"web_search\":false,"
      "\"page_context\":true,"
      "\"attachments\":false,"
      "\"model_selection\":false,"
      "\"cancellation\":false}}}";

  (void)context;

  if (request->has_provider_id && !request->provider_is_fake) {
    return pervue_protocol_write_error(
        output,
        &request->request_id,
        "PROVIDER_NOT_FOUND",
        "PROVIDER_NOT_INSTALLED",
        "The selected provider runtime is not installed.",
        false);
  }

  if (pervue_protocol_write_event(
          output,
          &request->request_id,
          "provider.status",
          fake_status) != 0) {
    return 1;
  }

  return pervue_protocol_write_event(
      output,
      &request->request_id,
      "response.completed",
      "{}");
}

static int handle_request_cancel(
    void *context,
    FILE *output,
    const pervue_request_t *request) {
  (void)context;

  return pervue_protocol_write_error(
      output,
      &request->request_id,
      "INVALID_REQUEST",
      "UNKNOWN_TARGET_REQUEST",
      "The target request is not in flight.",
      false);
}

pervue_host_result_t pervue_host_run(FILE *input, FILE *output) {
  static const pervue_router_handlers_t handlers = {
    handle_conversation_send,
    handle_provider_status,
    handle_request_cancel
  };

  if (input == NULL || output == NULL) {
    return PERVUE_HOST_INVALID_ARGUMENT;
  }

  if (pervue_protocol_write_host_ready(output) != 0) {
    return PERVUE_HOST_IO_ERROR;
  }

  for (;;) {
    pervue_frame_t frame;
    pervue_frame_result_t frame_result;

    pervue_frame_init(&frame);
    frame_result = pervue_frame_read(input, &frame);

    if (frame_result == PERVUE_FRAME_EOF) {
      pervue_frame_destroy(&frame);
      return PERVUE_HOST_OK;
    }

    if (frame_result != PERVUE_FRAME_OK) {
      pervue_host_result_t host_result = map_frame_error(frame_result);
      pervue_frame_destroy(&frame);
      return host_result;
    }

    {
      pervue_request_t request;
      pervue_request_failure_t failure;
      pervue_request_parse_result_t parse_result =
          pervue_request_parse(frame.data, frame.length, &request, &failure);

      if (parse_result != PERVUE_REQUEST_PARSE_OK) {
        failure.result = parse_result;
        if (pervue_protocol_write_parse_failure(output, &failure) != 0) {
          pervue_frame_destroy(&frame);
          return PERVUE_HOST_IO_ERROR;
        }
      } else if (pervue_router_dispatch(
                     &handlers,
                     NULL,
                     output,
                     &request) != 0) {
        pervue_frame_destroy(&frame);
        return PERVUE_HOST_IO_ERROR;
      }
    }

    pervue_frame_destroy(&frame);
  }
}

const char *pervue_host_version(void) {
  return PERVUE_HOST_VERSION;
}
