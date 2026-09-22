#include "pervue/frame.h"

#include <stdint.h>
#include <stdlib.h>

static uint32_t decode_u32_le(const unsigned char prefix[PERVUE_NATIVE_MESSAGE_PREFIX_SIZE]) {
  return ((uint32_t)prefix[0]) |
         ((uint32_t)prefix[1] << 8U) |
         ((uint32_t)prefix[2] << 16U) |
         ((uint32_t)prefix[3] << 24U);
}

static void encode_u32_le(
    uint32_t value,
    unsigned char prefix[PERVUE_NATIVE_MESSAGE_PREFIX_SIZE]) {
  prefix[0] = (unsigned char)(value & UINT32_C(0xff));
  prefix[1] = (unsigned char)((value >> 8U) & UINT32_C(0xff));
  prefix[2] = (unsigned char)((value >> 16U) & UINT32_C(0xff));
  prefix[3] = (unsigned char)((value >> 24U) & UINT32_C(0xff));
}

static pervue_frame_result_t read_exact(
    FILE *input,
    unsigned char *buffer,
    size_t length,
    size_t *bytes_read) {
  size_t total = 0U;

  while (total < length) {
    size_t count = fread(buffer + total, 1U, length - total, input);

    if (count > 0U) {
      total += count;
      continue;
    }

    *bytes_read = total;

    if (ferror(input) != 0) {
      return PERVUE_FRAME_IO_ERROR;
    }

    return PERVUE_FRAME_TRUNCATED;
  }

  *bytes_read = total;
  return PERVUE_FRAME_OK;
}

static pervue_frame_result_t write_exact(
    FILE *output,
    const unsigned char *buffer,
    size_t length) {
  size_t total = 0U;

  while (total < length) {
    size_t count = fwrite(buffer + total, 1U, length - total, output);

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

pervue_frame_result_t pervue_frame_read(FILE *input, pervue_frame_t *frame) {
  unsigned char prefix[PERVUE_NATIVE_MESSAGE_PREFIX_SIZE];
  size_t prefix_bytes = 0U;
  size_t payload_bytes = 0U;
  uint32_t encoded_length;
  size_t length;
  pervue_frame_result_t result;

  if (input == NULL || frame == NULL) {
    return PERVUE_FRAME_INVALID_ARGUMENT;
  }

  pervue_frame_init(frame);

  result = read_exact(input, prefix, sizeof(prefix), &prefix_bytes);
  if (result != PERVUE_FRAME_OK) {
    if (result == PERVUE_FRAME_TRUNCATED && prefix_bytes == 0U) {
      return PERVUE_FRAME_EOF;
    }

    return result;
  }

  encoded_length = decode_u32_le(prefix);
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

  result = read_exact(input, frame->data, length, &payload_bytes);
  if (result != PERVUE_FRAME_OK) {
    (void)payload_bytes;
    pervue_frame_destroy(frame);
    return result;
  }

  frame->length = length;
  return PERVUE_FRAME_OK;
}

pervue_frame_result_t pervue_frame_write(
    FILE *output,
    const unsigned char *data,
    size_t length) {
  unsigned char prefix[PERVUE_NATIVE_MESSAGE_PREFIX_SIZE];
  pervue_frame_result_t result;

  if (output == NULL || (data == NULL && length != 0U)) {
    return PERVUE_FRAME_INVALID_ARGUMENT;
  }

  if (length > (size_t)PERVUE_NATIVE_MAX_FRAME_SIZE ||
      length > (size_t)UINT32_MAX) {
    return PERVUE_FRAME_TOO_LARGE;
  }

  encode_u32_le((uint32_t)length, prefix);

  result = write_exact(output, prefix, sizeof(prefix));
  if (result != PERVUE_FRAME_OK) {
    return result;
  }

  if (length > 0U) {
    result = write_exact(output, data, length);
    if (result != PERVUE_FRAME_OK) {
      return result;
    }
  }

  if (fflush(output) != 0) {
    return PERVUE_FRAME_IO_ERROR;
  }

  return PERVUE_FRAME_OK;
}

const char *pervue_frame_result_name(pervue_frame_result_t result) {
  switch (result) {
    case PERVUE_FRAME_OK:
      return "ok";
    case PERVUE_FRAME_EOF:
      return "eof";
    case PERVUE_FRAME_INVALID_ARGUMENT:
      return "invalid_argument";
    case PERVUE_FRAME_IO_ERROR:
      return "io_error";
    case PERVUE_FRAME_TRUNCATED:
      return "truncated";
    case PERVUE_FRAME_TOO_LARGE:
      return "too_large";
    case PERVUE_FRAME_ALLOCATION_FAILED:
      return "allocation_failed";
    default:
      return "unknown";
  }
}
