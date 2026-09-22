#include "pervue/host.h"

#include <stdio.h>
#include <string.h>

#define PERVUE_TEST_INPUT_PATH "pervue_host_startup_input.tmp"
#define PERVUE_TEST_OUTPUT_PATH "pervue_host_startup_output.tmp"

static FILE *open_test_file(const char *path, const char *mode) {
#ifdef _MSC_VER
  FILE *stream = NULL;

  if (fopen_s(&stream, path, mode) != 0) {
    return NULL;
  }

  return stream;
#else
  return fopen(path, mode);
#endif
}

static void close_and_remove(FILE **stream, const char *path) {
  if (*stream != NULL) {
    fclose(*stream);
    *stream = NULL;
  }

  remove(path);
}

static int test_version(void) {
  return strcmp(pervue_host_version(), PERVUE_HOST_VERSION) == 0 ? 0 : 1;
}

static int test_stream_lifecycle(void) {
  static const unsigned char sample[] = {
    0x00U, 0x01U, 0x02U, 0x1aU, 0x0aU, 0x7fU, 0xffU
  };
  FILE *input = open_test_file(PERVUE_TEST_INPUT_PATH, "w+b");
  FILE *output = open_test_file(PERVUE_TEST_OUTPUT_PATH, "w+b");
  int result = 1;

  if (input == NULL || output == NULL) {
    goto cleanup;
  }

  if (fwrite(sample, 1U, sizeof(sample), input) != sizeof(sample)) {
    goto cleanup;
  }

  if (fflush(input) != 0) {
    goto cleanup;
  }

  if (fseek(input, 0L, SEEK_SET) != 0) {
    goto cleanup;
  }

  if (pervue_host_run(input, output) != PERVUE_HOST_OK) {
    goto cleanup;
  }

  if (fseek(output, 0L, SEEK_END) != 0) {
    goto cleanup;
  }

  if (ftell(output) != 0L) {
    goto cleanup;
  }

  result = 0;

cleanup:
  close_and_remove(&input, PERVUE_TEST_INPUT_PATH);
  close_and_remove(&output, PERVUE_TEST_OUTPUT_PATH);
  return result;
}

static int test_invalid_streams(void) {
  FILE *stream = open_test_file(PERVUE_TEST_OUTPUT_PATH, "w+b");
  int result = 1;

  if (stream == NULL) {
    return 1;
  }

  if (pervue_host_run(NULL, stream) != PERVUE_HOST_INVALID_ARGUMENT) {
    goto cleanup;
  }

  if (pervue_host_run(stream, NULL) != PERVUE_HOST_INVALID_ARGUMENT) {
    goto cleanup;
  }

  result = 0;

cleanup:
  close_and_remove(&stream, PERVUE_TEST_OUTPUT_PATH);
  return result;
}

int main(void) {
  if (test_version() != 0) {
    fprintf(stderr, "version test failed\n");
    return 1;
  }

  if (test_stream_lifecycle() != 0) {
    fprintf(stderr, "stream lifecycle test failed\n");
    return 1;
  }

  if (test_invalid_streams() != 0) {
    fprintf(stderr, "invalid stream test failed\n");
    return 1;
  }

  return 0;
}
