#ifndef PERVUE_FRAME_H
#define PERVUE_FRAME_H

#include <stddef.h>
#include <stdio.h>

#define PERVUE_NATIVE_MESSAGE_PREFIX_SIZE 4U
#define PERVUE_NATIVE_MAX_FRAME_SIZE (1024U * 1024U)

typedef enum pervue_frame_result {
  PERVUE_FRAME_OK = 0,
  PERVUE_FRAME_EOF = 1,
  PERVUE_FRAME_INVALID_ARGUMENT = 2,
  PERVUE_FRAME_IO_ERROR = 3,
  PERVUE_FRAME_TRUNCATED = 4,
  PERVUE_FRAME_TOO_LARGE = 5,
  PERVUE_FRAME_ALLOCATION_FAILED = 6
} pervue_frame_result_t;

typedef struct pervue_frame {
  unsigned char *data;
  size_t length;
} pervue_frame_t;

/*
 * A frame must be initialized before first use and destroyed after a
 * successful read. The reader never allocates more than
 * PERVUE_NATIVE_MAX_FRAME_SIZE bytes.
 */
void pervue_frame_init(pervue_frame_t *frame);
void pervue_frame_destroy(pervue_frame_t *frame);

/*
 * Reads one Chrome Native Messaging frame:
 *
 *   4-byte unsigned little-endian payload length
 *   payload bytes
 *
 * PERVUE_FRAME_EOF is returned only when EOF occurs before any prefix byte.
 * EOF after a partial prefix or payload is PERVUE_FRAME_TRUNCATED.
 */
pervue_frame_result_t pervue_frame_read(FILE *input, pervue_frame_t *frame);

/*
 * Writes one bounded Chrome Native Messaging frame and flushes it.
 * A NULL payload is valid only when length is zero.
 */
pervue_frame_result_t pervue_frame_write(
    FILE *output,
    const unsigned char *data,
    size_t length);

const char *pervue_frame_result_name(pervue_frame_result_t result);

#endif
