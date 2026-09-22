#include "pervue/frame.h"

#include "frame_internal.h"

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

_Static_assert(
    sizeof(uint32_t) == PERVUE_NATIVE_MESSAGE_PREFIX_SIZE,
    "Native Messaging requires a 32-bit length prefix");

static uint32_t decode_u32_native(
    const unsigned char prefix[PERVUE_NATIVE_MESSAGE_PREFIX_SIZE]) {
  uint32_t value;

  memcpy(&value, prefix, sizeof(value));
  return value;
}

static void encode_u32_native(
    uint32_t value,
    unsigned char prefix[PERVUE_NATIVE_MESSAGE_PREFIX_SIZE]) {
  memcpy(prefix, &value, sizeof(value));
}

static size_t file_read(void *context, unsigned char *buffer, size_t length) {
  return fread(buffer, 1U, length, (FILE *)context);
}

static size_t file_write(
    void *context,
    const unsigned char *buffer,
    size_t length) {
  return fwrite(buffer, 1U, length, (FILE *)context);
}

static int file_has_error(void *context) {
  return ferror((FILE *)context);
}

static int file_flush(void *context) {
  return fflush((FILE *)context);
}

static pervue_frame_result_t read_exact(
    pervue_frame_io_t *io,
    unsigned char *buffer,
    size_t length,
    size_t *bytes_read) {
  size_t total = 0U;

  while (total < length) {
    size_t count = io->read(io->context, buffer + total, length - total);

    if (count > 0U && count <= (length - total)) {
      total += count;
      continue;
    }

    *bytes_read = total;

    if (count > (length - total) || io->has_error(io->context) != 0) {
      return PERVUE_FRAME_IO_ERROR;
    }

    return PERVUE_FRAME_TRUNCATED;
  }

  *bytes_read = total;
  return PERVUE_FRAME_OK;
}

static pervue_frame_result_t write_exact(
    pervue_frame_io_t *io,
    const unsigned char *buffer,
    size_t length) {
  size_t total = 0U;

  while (total < length) {
    size_t count = io->write(io->context, buffer + total, length - total);

    if (count == 0U || count > (length - total)) {
      return PERVUE_FRAME_IO_ERROR;
    }

    total += count;
  }

  return PERVUE_FRAME_OK;
}

void pervue_frame_init(pervue_frame_t *frame) {
  if (frame == NULL) {
    return;
  }

  frame->data = NULL;
  frame->length = 0U;
}

void pervue_frame_destroy(pervue_frame_t *frame) {
  if (frame == NULL) {
    return;
  }

  free(frame->data);
  frame->data = NULL;
  frame->length = 0U;
}

pervue_frame_result_t pervue_frame_read_io(
    pervue_frame_io_t *io,
    pervue_frame_t *frame) {
  unsigned char prefix[PERVUE_NATIVE_MESSAGE_PREFIX_SIZE];
  size_t prefix_bytes = 0U;
  size_t payload_bytes = 0U;
  uint32_t encoded_length;
  size_t length;
  pervue_frame_result_t result;

  if (io == NULL || io->read == NULL || io->has_error == NULL ||
      frame == NULL) {
    return PERVUE_FRAME_INVALID_ARGUMENT;
  }

  if (frame->data != NULL || frame->length != 0U) {
    return PERVUE_FRAME_INVALID_ARGUMENT;
  }

  result = read_exact(io, prefix, sizeof(prefix), &prefix_bytes);
  if (result != PERVUE_FRAME_OK) {
    if (result == PERVUE_FRAME_TRUNCATED && prefix_bytes == 0U) {
      return PERVUE_FRAME_EOF;
    }

    return result;
  }

  encoded_length = decode_u32_native(prefix);
  length = (size_t)encoded_length;

  if (length > (size_t)PERVUE_NATIVE_MAX_FRAME_SIZE) {
    return PERVUE_FRAME_TOO_LARGE;
  }

  if (length == 0U) {
    return PERVUE_FRAME_OK;
  }

  frame->data = (unsigned char *)malloc(length);
  if (frame->data == NULL) {
    return PERVUE_FRAME_ALLOCATION_FAILED;
  }

  result = read_exact(io, frame->data, length, &payload_bytes);
  if (result != PERVUE_FRAME_OK) {
    (void)payload_bytes;
    pervue_frame_destroy(frame);
    return result;
  }

  frame->length = length;
  return PERVUE_FRAME_OK;
}

pervue_frame_result_t pervue_frame_write_io(
    pervue_frame_io_t *io,
    const unsigned char *data,
    size_t length) {
  unsigned char prefix[PERVUE_NATIVE_MESSAGE_PREFIX_SIZE];
  pervue_frame_result_t result;

  if (io == NULL || io->write == NULL || io->flush == NULL ||
      (data == NULL && length != 0U)) {
    return PERVUE_FRAME_INVALID_ARGUMENT;
  }

  if (length > (size_t)PERVUE_NATIVE_MAX_FRAME_SIZE ||
      length > (size_t)UINT32_MAX) {
    return PERVUE_FRAME_TOO_LARGE;
  }

  encode_u32_native((uint32_t)length, prefix);

  result = write_exact(io, prefix, sizeof(prefix));
  if (result != PERVUE_FRAME_OK) {
    return result;
  }

  if (length > 0U) {
    result = write_exact(io, data, length);
    if (result != PERVUE_FRAME_OK) {
      return result;
    }
  }

  if (io->flush(io->context) != 0) {
    return PERVUE_FRAME_IO_ERROR;
  }

  return PERVUE_FRAME_OK;
}

pervue_frame_result_t pervue_frame_read(FILE *input, pervue_frame_t *frame) {
  pervue_frame_io_t io;

  if (input == NULL) {
    return PERVUE_FRAME_INVALID_ARGUMENT;
  }

  io.context = input;
  io.read = file_read;
  io.write = NULL;
  io.has_error = file_has_error;
  io.flush = NULL;

  return pervue_frame_read_io(&io, frame);
}

pervue_frame_result_t pervue_frame_write(
    FILE *output,
    const unsigned char *data,
    size_t length) {
  pervue_frame_io_t io;

  if (output == NULL) {
    return PERVUE_FRAME_INVALID_ARGUMENT;
  }

  io.context = output;
  io.read = NULL;
  io.write = file_write;
  io.has_error = NULL;
  io.flush = file_flush;

  return pervue_frame_write_io(&io, data, length);
}
