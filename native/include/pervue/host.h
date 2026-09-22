#ifndef PERVUE_HOST_H
#define PERVUE_HOST_H

#include "pervue/version.h"

#include <stdio.h>

typedef enum pervue_host_result {
  PERVUE_HOST_OK = 0,
  PERVUE_HOST_INVALID_ARGUMENT = 1,
  PERVUE_HOST_IO_ERROR = 2,
  PERVUE_HOST_FRAME_TRUNCATED = 3,
  PERVUE_HOST_FRAME_TOO_LARGE = 4,
  PERVUE_HOST_ALLOCATION_FAILED = 5
} pervue_host_result_t;

/*
 * Host loop for bounded Native Messaging frames.
 *
 * NAT-02 consumes and validates complete frames but deliberately does not
 * interpret JSON payloads or emit protocol events. NAT-03 owns that layer.
 */
pervue_host_result_t pervue_host_run(FILE *input, FILE *output);

const char *pervue_host_version(void);

#endif
