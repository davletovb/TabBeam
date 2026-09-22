#include "pervue/protocol.h"

#include <stddef.h>
#include <stdint.h>

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
  pervue_request_t request;
  pervue_request_failure_t failure;

  (void)pervue_request_parse(
      (const unsigned char *)data,
      size,
      &request,
      &failure);

  return 0;
}
