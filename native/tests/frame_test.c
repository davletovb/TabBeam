#include "pervue/frame.h"

#include "frame_internal.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define PERVUE_FRAME_TEST_PATH "pervue_frame_test.tmp"

typedef struct test_io {
  const unsigned char *read_data;
  size_t read_length;
  size_t read_offset;
  size_t read_chunk;
  size_t read_calls;
  unsigned char write_data[64];
  size_t write_capacity;
  size_t write_length;
  size_t write_chunk;
  size_t write_calls;
  int io_error;
  int flush_error;
  size_t flush_calls;
} test_io_t;

static FILE *open_test_file(void) {
#ifdef _MSC_VER
  FILE *stream = NULL;

  if (fopen_s(&stream, PERVUE_FRAME_TEST_PATH, "w+b") != 0) {
    return NULL;
  }

  return stream;
#else
  return fopen(PERVUE_FRAME_TEST_PATH, "w+b");
#endif
}

static void close_test_file(FILE **stream) {
  if (*stream != NULL) {
    fclose(*stream);
    *stream = NULL;
  }

  remove(PERVUE_FRAME_TEST_PATH);
}

static int rewind_test_file(FILE *stream) {
  return fflush(stream) == 0 && fseek(stream, 0L, SEEK_SET) == 0 ? 0 : 1;
}

static void encode_length(
    uint32_t length,
    unsigned char prefix[PERVUE_NATIVE_MESSAGE_PREFIX_SIZE]) {
  memcpy(prefix, &length, sizeof(length));
}

static size_t test_io_read(void *context, unsigned char *buffer, size_t length) {
  test_io_t *io = (test_io_t *)context;
  size_t remaining = io->read_length - io->read_offset;
  size_t count = length;

  io->read_calls += 1U;

  if (count > io->read_chunk) {
    count = io->read_chunk;
  }

  if (count > remaining) {
    count = remaining;
  }

  if (count > 0U) {
    memcpy(buffer, io->read_data + io->read_offset, count);
    io->read_offset += count;
  }

  return count;
}

static size_t test_io_write(
    void *context,
    const unsigned char *buffer,
    size_t length) {
  test_io_t *io = (test_io_t *)context;
  size_t remaining = io->write_capacity - io->write_length;
  size_t count = length;

  io->write_calls += 1U;

  if (count > io->write_chunk) {
    count = io->write_chunk;
  }

  if (count > remaining) {
    count = remaining;
  }

  if (count > 0U) {
    memcpy(io->write_data + io->write_length, buffer, count);
    io->write_length += count;
  }

  return count;
}

static int test_io_has_error(void *context) {
  test_io_t *io = (test_io_t *)context;
  return io->io_error;
}

static int test_io_flush(void *context) {
  test_io_t *io = (test_io_t *)context;
  io->flush_calls += 1U;
  return io->flush_error;
}

static int test_round_trip(void) {
  static const unsigned char payload[] = {
    0x00U, 0x01U, 0x0aU, 0x1aU, 0x7fU, 0xffU
  };
  FILE *stream = open_test_file();
  pervue_frame_t frame;
  int result = 1;

  pervue_frame_init(&frame);

  if (stream == NULL) {
    return 1;
  }

  if (pervue_frame_write(stream, payload, sizeof(payload)) != PERVUE_FRAME_OK) {
    goto cleanup;
  }

  if (rewind_test_file(stream) != 0) {
    goto cleanup;
  }

  if (pervue_frame_read(stream, &frame) != PERVUE_FRAME_OK) {
    goto cleanup;
  }

  if (frame.length != sizeof(payload) ||
      memcmp(frame.data, payload, sizeof(payload)) != 0) {
    goto cleanup;
  }

  pervue_frame_destroy(&frame);

  if (pervue_frame_read(stream, &frame) != PERVUE_FRAME_EOF) {
    goto cleanup;
  }

  result = 0;

cleanup:
  pervue_frame_destroy(&frame);
  close_test_file(&stream);
  return result;
}

static int test_empty_frame(void) {
  FILE *stream = open_test_file();
  pervue_frame_t frame;
  int result = 1;

  pervue_frame_init(&frame);

  if (stream == NULL) {
    return 1;
  }

  if (pervue_frame_write(stream, NULL, 0U) != PERVUE_FRAME_OK) {
    goto cleanup;
  }

  if (rewind_test_file(stream) != 0) {
    goto cleanup;
  }

  if (pervue_frame_read(stream, &frame) != PERVUE_FRAME_OK) {
    goto cleanup;
  }

  if (frame.length != 0U || frame.data != NULL) {
    goto cleanup;
  }

  result = 0;

cleanup:
  pervue_frame_destroy(&frame);
  close_test_file(&stream);
  return result;
}

static int test_max_frame(void) {
  FILE *stream = open_test_file();
  unsigned char *payload = NULL;
  pervue_frame_t frame;
  size_t index;
  int result = 1;

  pervue_frame_init(&frame);

  if (stream == NULL) {
    return 1;
  }

  payload = (unsigned char *)malloc((size_t)PERVUE_NATIVE_MAX_FRAME_SIZE);
  if (payload == NULL) {
    goto cleanup;
  }

  for (index = 0U; index < (size_t)PERVUE_NATIVE_MAX_FRAME_SIZE; ++index) {
    payload[index] = (unsigned char)(index % 251U);
  }

  if (pervue_frame_write(
          stream,
          payload,
          (size_t)PERVUE_NATIVE_MAX_FRAME_SIZE) != PERVUE_FRAME_OK) {
    goto cleanup;
  }

  if (rewind_test_file(stream) != 0) {
    goto cleanup;
  }

  if (pervue_frame_read(stream, &frame) != PERVUE_FRAME_OK) {
    goto cleanup;
  }

  if (frame.length != (size_t)PERVUE_NATIVE_MAX_FRAME_SIZE ||
      memcmp(frame.data, payload, frame.length) != 0) {
    goto cleanup;
  }

  result = 0;

cleanup:
  free(payload);
  pervue_frame_destroy(&frame);
  close_test_file(&stream);
  return result;
}

static int test_wire_format_uses_native_byte_order(void) {
  static const unsigned char payload[] = {0xaaU, 0xbbU, 0xccU, 0xddU};
  static const unsigned char little_endian_expected[] = {
    0x04U, 0x00U, 0x00U, 0x00U, 0xaaU, 0xbbU, 0xccU, 0xddU
  };
  static const unsigned char big_endian_expected[] = {
    0x00U, 0x00U, 0x00U, 0x04U, 0xaaU, 0xbbU, 0xccU, 0xddU
  };
  uint32_t endian_probe = UINT32_C(1);
  const unsigned char *probe = (const unsigned char *)&endian_probe;
  unsigned char actual[sizeof(little_endian_expected)];
  const unsigned char *expected = NULL;
  FILE *stream = open_test_file();
  int result = 1;

  if (stream == NULL) {
    return 1;
  }

  if (probe[0] == 1U) {
    expected = little_endian_expected;
  } else if (probe[sizeof(endian_probe) - 1U] == 1U) {
    expected = big_endian_expected;
  } else {
    goto cleanup;
  }

  if (pervue_frame_write(stream, payload, sizeof(payload)) != PERVUE_FRAME_OK) {
    goto cleanup;
  }

  if (rewind_test_file(stream) != 0) {
    goto cleanup;
  }

  if (fread(actual, 1U, sizeof(actual), stream) != sizeof(actual)) {
    goto cleanup;
  }

  if (memcmp(actual, expected, sizeof(actual)) != 0) {
    goto cleanup;
  }

  result = 0;

cleanup:
  close_test_file(&stream);
  return result;
}

static int test_short_read_loop(void) {
  static const unsigned char payload[] = {0x61U, 0x62U, 0x63U};
  unsigned char wire[PERVUE_NATIVE_MESSAGE_PREFIX_SIZE + sizeof(payload)];
  test_io_t storage = {0};
  pervue_frame_io_t io;
  pervue_frame_t frame;
  int result = 1;

  encode_length((uint32_t)sizeof(payload), wire);
  memcpy(wire + PERVUE_NATIVE_MESSAGE_PREFIX_SIZE, payload, sizeof(payload));

  storage.read_data = wire;
  storage.read_length = sizeof(wire);
  storage.read_chunk = 1U;

  io.context = &storage;
  io.read = test_io_read;
  io.write = NULL;
  io.has_error = test_io_has_error;
  io.flush = NULL;

  pervue_frame_init(&frame);

  if (pervue_frame_read_io(&io, &frame) != PERVUE_FRAME_OK) {
    goto cleanup;
  }

  if (storage.read_calls != sizeof(wire) ||
      frame.length != sizeof(payload) ||
      memcmp(frame.data, payload, sizeof(payload)) != 0) {
    goto cleanup;
  }

  result = 0;

cleanup:
  pervue_frame_destroy(&frame);
  return result;
}

static int test_short_write_loop(void) {
  static const unsigned char payload[] = {0x61U, 0x62U, 0x63U};
  unsigned char expected[PERVUE_NATIVE_MESSAGE_PREFIX_SIZE + sizeof(payload)];
  test_io_t storage = {0};
  pervue_frame_io_t io;

  encode_length((uint32_t)sizeof(payload), expected);
  memcpy(expected + PERVUE_NATIVE_MESSAGE_PREFIX_SIZE, payload, sizeof(payload));

  storage.write_capacity = sizeof(expected);
  storage.write_chunk = 1U;

  io.context = &storage;
  io.read = NULL;
  io.write = test_io_write;
  io.has_error = NULL;
  io.flush = test_io_flush;

  if (pervue_frame_write_io(&io, payload, sizeof(payload)) != PERVUE_FRAME_OK) {
    return 1;
  }

  if (storage.write_calls != sizeof(expected) ||
      storage.flush_calls != 1U ||
      storage.write_length != sizeof(expected) ||
      memcmp(storage.write_data, expected, sizeof(expected)) != 0) {
    return 1;
  }

  return 0;
}

static int test_oversized_read(void) {
  unsigned char prefix[PERVUE_NATIVE_MESSAGE_PREFIX_SIZE];
  FILE *stream = open_test_file();
  pervue_frame_t frame;
  int result = 1;

  pervue_frame_init(&frame);

  if (stream == NULL) {
    return 1;
  }

  encode_length(
      (uint32_t)PERVUE_NATIVE_MAX_FRAME_SIZE + UINT32_C(1),
      prefix);

  if (fwrite(prefix, 1U, sizeof(prefix), stream) != sizeof(prefix)) {
    goto cleanup;
  }

  if (rewind_test_file(stream) != 0) {
    goto cleanup;
  }

  if (pervue_frame_read(stream, &frame) != PERVUE_FRAME_TOO_LARGE) {
    goto cleanup;
  }

  if (frame.data != NULL || frame.length != 0U) {
    goto cleanup;
  }

  result = 0;

cleanup:
  pervue_frame_destroy(&frame);
  close_test_file(&stream);
  return result;
}

static int test_oversized_write(void) {
  static const unsigned char payload = 0U;
  FILE *stream = open_test_file();
  long size;
  int result = 1;

  if (stream == NULL) {
    return 1;
  }

  if (pervue_frame_write(
          stream,
          &payload,
          (size_t)PERVUE_NATIVE_MAX_FRAME_SIZE + 1U) !=
      PERVUE_FRAME_TOO_LARGE) {
    goto cleanup;
  }

  if (fseek(stream, 0L, SEEK_END) != 0) {
    goto cleanup;
  }

  size = ftell(stream);
  if (size != 0L) {
    goto cleanup;
  }

  result = 0;

cleanup:
  close_test_file(&stream);
  return result;
}

static int test_truncated_header(void) {
  static const unsigned char bytes[] = {0x05U, 0x00U};
  FILE *stream = open_test_file();
  pervue_frame_t frame;
  int result = 1;

  pervue_frame_init(&frame);

  if (stream == NULL) {
    return 1;
  }

  if (fwrite(bytes, 1U, sizeof(bytes), stream) != sizeof(bytes)) {
    goto cleanup;
  }

  if (rewind_test_file(stream) != 0) {
    goto cleanup;
  }

  if (pervue_frame_read(stream, &frame) != PERVUE_FRAME_TRUNCATED) {
    goto cleanup;
  }

  result = 0;

cleanup:
  pervue_frame_destroy(&frame);
  close_test_file(&stream);
  return result;
}

static int test_truncated_payload(void) {
  static const unsigned char payload[] = {0x61U, 0x62U, 0x63U};
  unsigned char prefix[PERVUE_NATIVE_MESSAGE_PREFIX_SIZE];
  FILE *stream = open_test_file();
  pervue_frame_t frame;
  int result = 1;

  pervue_frame_init(&frame);

  if (stream == NULL) {
    return 1;
  }

  encode_length(UINT32_C(5), prefix);

  if (fwrite(prefix, 1U, sizeof(prefix), stream) != sizeof(prefix) ||
      fwrite(payload, 1U, sizeof(payload), stream) != sizeof(payload)) {
    goto cleanup;
  }

  if (rewind_test_file(stream) != 0) {
    goto cleanup;
  }

  if (pervue_frame_read(stream, &frame) != PERVUE_FRAME_TRUNCATED) {
    goto cleanup;
  }

  if (frame.data != NULL || frame.length != 0U) {
    goto cleanup;
  }

  result = 0;

cleanup:
  pervue_frame_destroy(&frame);
  close_test_file(&stream);
  return result;
}

static int test_eof(void) {
  FILE *stream = open_test_file();
  pervue_frame_t frame;
  int result = 1;

  pervue_frame_init(&frame);

  if (stream == NULL) {
    return 1;
  }

  if (pervue_frame_read(stream, &frame) != PERVUE_FRAME_EOF) {
    goto cleanup;
  }

  result = 0;

cleanup:
  pervue_frame_destroy(&frame);
  close_test_file(&stream);
  return result;
}

static int test_invalid_arguments(void) {
  FILE *stream = open_test_file();
  pervue_frame_t frame;
  unsigned char *owned = NULL;
  int result = 1;

  pervue_frame_init(&frame);

  if (stream == NULL) {
    return 1;
  }

  if (pervue_frame_read(NULL, &frame) != PERVUE_FRAME_INVALID_ARGUMENT ||
      pervue_frame_read(stream, NULL) != PERVUE_FRAME_INVALID_ARGUMENT ||
      pervue_frame_write(NULL, NULL, 0U) != PERVUE_FRAME_INVALID_ARGUMENT ||
      pervue_frame_write(stream, NULL, 1U) != PERVUE_FRAME_INVALID_ARGUMENT) {
    goto cleanup;
  }

  owned = (unsigned char *)malloc(1U);
  if (owned == NULL) {
    goto cleanup;
  }

  frame.data = owned;
  frame.length = 1U;

  if (pervue_frame_read(stream, &frame) != PERVUE_FRAME_INVALID_ARGUMENT) {
    goto cleanup;
  }

  result = 0;

cleanup:
  pervue_frame_destroy(&frame);
  close_test_file(&stream);
  return result;
}

int main(void) {
  if (test_round_trip() != 0) {
    fprintf(stderr, "round-trip frame test failed\n");
    return 1;
  }

  if (test_empty_frame() != 0) {
    fprintf(stderr, "empty frame test failed\n");
    return 1;
  }

  if (test_max_frame() != 0) {
    fprintf(stderr, "max frame test failed\n");
    return 1;
  }

  if (test_wire_format_uses_native_byte_order() != 0) {
    fprintf(stderr, "native byte-order wire format test failed\n");
    return 1;
  }

  if (test_short_read_loop() != 0) {
    fprintf(stderr, "short-read loop test failed\n");
    return 1;
  }

  if (test_short_write_loop() != 0) {
    fprintf(stderr, "short-write loop test failed\n");
    return 1;
  }

  if (test_oversized_read() != 0) {
    fprintf(stderr, "oversized read test failed\n");
    return 1;
  }

  if (test_oversized_write() != 0) {
    fprintf(stderr, "oversized write test failed\n");
    return 1;
  }

  if (test_truncated_header() != 0) {
    fprintf(stderr, "truncated header test failed\n");
    return 1;
  }

  if (test_truncated_payload() != 0) {
    fprintf(stderr, "truncated payload test failed\n");
    return 1;
  }

  if (test_eof() != 0) {
    fprintf(stderr, "EOF test failed\n");
    return 1;
  }

  if (test_invalid_arguments() != 0) {
    fprintf(stderr, "invalid argument test failed\n");
    return 1;
  }

  return 0;
}
