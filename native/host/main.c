#include "pervue/host.h"

#include <stdio.h>
#include <string.h>

#ifdef _WIN32
#include <fcntl.h>
#include <io.h>
#endif

enum {
  PERVUE_EXIT_USAGE = 64,
  PERVUE_EXIT_IO_ERROR = 74
};

static int print_usage(const char *program_name) {
  fprintf(
      stderr,
      "usage: %s [--version | chrome-extension://<extension-id>/]\n",
      program_name);
  return PERVUE_EXIT_USAGE;
}

static int is_chrome_extension_origin(const char *argument) {
  static const char prefix[] = "chrome-extension://";
  size_t prefix_length = sizeof(prefix) - 1U;

  return strncmp(argument, prefix, prefix_length) == 0 &&
         argument[prefix_length] != '\0';
}

static int configure_binary_stdio(void) {
#ifdef _WIN32
  if (_setmode(_fileno(stdin), _O_BINARY) == -1) {
    fputs("failed to set stdin to binary mode\n", stderr);
    return PERVUE_EXIT_IO_ERROR;
  }

  if (_setmode(_fileno(stdout), _O_BINARY) == -1) {
    fputs("failed to set stdout to binary mode\n", stderr);
    return PERVUE_EXIT_IO_ERROR;
  }
#endif

  return 0;
}

int main(int argc, char **argv) {
  int stdio_result;

  if (argc == 2 && strcmp(argv[1], "--version") == 0) {
    puts(pervue_host_version());
    return 0;
  }

  if (argc == 2) {
    if (!is_chrome_extension_origin(argv[1])) {
      return print_usage(argv[0]);
    }

    /*
     * Chrome supplies the caller origin here. Origin allowlisting is enforced
     * by Native Messaging registration and is hardened further by SEC-01 /
     * packaging work; NAT-02 only accepts the production launch shape.
     */
  } else if (argc != 1) {
    return print_usage(argv[0]);
  }

  stdio_result = configure_binary_stdio();
  if (stdio_result != 0) {
    return stdio_result;
  }

  return (int)pervue_host_run(stdin, stdout);
}
