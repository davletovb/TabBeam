#include "pervue/frame.h"

#include <stddef.h>
#include <stdint.h>
#include <stdio.h>

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
  FILE *stream;
  size_t index;

  stream = tmpfile();
  if (stream == NULL) {
    return 0;
  }

  if (size > 0U && fwrite(data, 1U, size, stream) != size) {
    fclose(stream);
    return 0;
  }

  if (fflush(stream) != 0 || fseek(stream, 0L, SEEK_SET) != 0) {
    fclose(stream);
    return 0;
  }

  /*
   * Bound work per fuzz input. Zero-length frames consume four input bytes,
   * so a valid stream cannot contain more than size / 4 + 1 frames.
   */
  for (index = 0U; index <= (size / PERVUE_NATIVE_MESSAGE_PREFIX_SIZE); ++index) {
    pervue_frame_t frame;
    pervue_frame_result_t result;

    pervue_frame_init(&frame);
    result = pervue_frame_read(stream, &frame);
    pervue_frame_destroy(&frame);

    if (result != PERVUE_FRAME_OK) {
      break;
    }
  }

  fclose(stream);
  return 0;
}
