#include "pervue/frame.h"

#include "frame_internal.h"

#include <stddef.h>
#include <stdint.h>
#include <string.h>

typedef struct pervue_fuzz_input {
  const uint8_t *data;
  size_t size;
  size_t offset;
} pervue_fuzz_input_t;

static size_t fuzz_read(void *context, unsigned char *buffer, size_t length) {
  pervue_fuzz_input_t *input = (pervue_fuzz_input_t *)context;
  size_t remaining = input->size - input->offset;
  size_t count = length < remaining ? length : remaining;

  if (count > 0U) {
    memcpy(buffer, input->data + input->offset, count);
    input->offset += count;
  }

  return count;
}

static int fuzz_has_error(void *context) {
  (void)context;
  return 0;
}

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
  pervue_fuzz_input_t input;
  pervue_frame_io_t io;
  size_t index;

  input.data = data;
  input.size = size;
  input.offset = 0U;

  io.context = &input;
  io.read = fuzz_read;
  io.write = NULL;
  io.has_error = fuzz_has_error;
  io.flush = NULL;

  /*
   * Bound work per fuzz input. Zero-length frames consume four input bytes,
   * so a valid stream cannot contain more than size / 4 + 1 frames.
   */
  for (index = 0U; index <= (size / PERVUE_NATIVE_MESSAGE_PREFIX_SIZE); ++index) {
    pervue_frame_t frame;
    pervue_frame_result_t result;

    pervue_frame_init(&frame);
    result = pervue_frame_read_io(&io, &frame);
    pervue_frame_destroy(&frame);

    if (result != PERVUE_FRAME_OK) {
      break;
    }
  }

  return 0;
}
