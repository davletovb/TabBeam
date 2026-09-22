#include "pervue/host.h"

#include "pervue/frame.h"

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

pervue_host_result_t pervue_host_run(FILE *input, FILE *output) {
  if (input == NULL || output == NULL) {
    return PERVUE_HOST_INVALID_ARGUMENT;
  }

  for (;;) {
    pervue_frame_t frame;
    pervue_frame_result_t frame_result;

    pervue_frame_init(&frame);
    frame_result = pervue_frame_read(input, &frame);

    if (frame_result == PERVUE_FRAME_EOF) {
      pervue_frame_destroy(&frame);

      if (fflush(output) != 0) {
        return PERVUE_HOST_IO_ERROR;
      }

      return PERVUE_HOST_OK;
    }

    if (frame_result != PERVUE_FRAME_OK) {
      pervue_host_result_t host_result = map_frame_error(frame_result);
      pervue_frame_destroy(&frame);
      return host_result;
    }

    /*
     * NAT-02 stops at validated framing. NAT-03 will parse and route the
     * payload, then use pervue_frame_write() for protocol events.
     */
    pervue_frame_destroy(&frame);
  }
}

const char *pervue_host_version(void) {
  return PERVUE_HOST_VERSION;
}
