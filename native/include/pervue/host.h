#ifndef PERVUE_HOST_H
#define PERVUE_HOST_H

#include "pervue/version.h"

#include <stdio.h>

typedef enum pervue_host_result {
  PERVUE_HOST_OK = 0,
  PERVUE_HOST_INVALID_ARGUMENT = 1,
  PERVUE_HOST_IO_ERROR = 2
} pervue_host_result_t;

/*
 * Foundation host loop.
 *
 * NAT-01 intentionally does not interpret Native Messaging frames yet.
 * It validates the stdio lifecycle by consuming input until EOF and
 * flushing output before returning.
 */
pervue_host_result_t pervue_host_run(FILE *input, FILE *output);

const char *pervue_host_version(void);

#endif
