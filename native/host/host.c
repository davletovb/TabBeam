#include "pervue/host.h"

#include <stddef.h>

enum {
  PERVUE_HOST_READ_BUFFER_SIZE = 4096
};

pervue_host_result_t pervue_host_run(FILE *input, FILE *output) {
  unsigned char buffer[PERVUE_HOST_READ_BUFFER_SIZE];

  if (input == NULL || output == NULL) {
    return PERVUE_HOST_INVALID_ARGUMENT;
  }

  while (fread(buffer, 1U, sizeof(buffer), input) > 0U) {
    /*
     * NAT-01 deliberately discards bytes. NAT-02 owns Native Messaging
     * frame parsing and must replace this foundation behavior.
     */
  }

  if (ferror(input) != 0) {
    return PERVUE_HOST_IO_ERROR;
  }

  if (fflush(output) != 0) {
    return PERVUE_HOST_IO_ERROR;
  }

  return PERVUE_HOST_OK;
}

const char *pervue_host_version(void) {
  return PERVUE_HOST_VERSION;
}
