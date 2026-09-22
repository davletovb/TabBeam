#include "pervue/frame.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define PERVUE_FRAME_TEST_PATH "pervue_frame_test.tmp"

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
    unsigned long length,
    unsigned char prefix[PERVUE_NATIVE_MESSAGE_PREFIX_SIZE]) {
  prefix[0] = (unsigned char)(length & 0xffUL);
  prefix[1] = (unsigned char)((length >> 8U) & 0xffUL);
  prefix[2] = (unsigned char)((length >> 16U) & 0xffUL);
  prefix[3] = (unsigned char)((length >> 24U) & 0xffUL);
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

static int test_wire_format_is_little_endian(void) {
  static const unsigned char payload[] = {0x41U};
  static const unsigned char expected[] = {
    0x01U, 0x00U, 0x00U, 0x00U, 0x41U
  };
  unsigned char actual[sizeof(expected)];
  FILE *stream = open_test_file();
  int result = 1;

  if (stream == NULL) {
    return 1;
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

  if (memcmp(actual, expected, sizeof(expected)) != 0) {
    goto cleanup;
  }

  result = 0;

cleanup:
  close_test_file(&stream);
  return result;
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
      (unsigned long)PERVUE_NATIVE_MAX_FRAME_SIZE + 1UL,
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

  encode_length(5UL, prefix);

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

  if (test_wire_format_is_little_endian() != 0) {
    fprintf(stderr, "wire format test failed\n");
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
