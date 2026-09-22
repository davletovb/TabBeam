#ifndef _WIN32
#define _POSIX_C_SOURCE 199309L
#endif

#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <windows.h>
#else
#include <errno.h>
#include <time.h>
#endif

#define PERVUE_FAKE_EXIT_NONZERO 42
#define PERVUE_FAKE_LARGE_OUTPUT_SIZE (2U * 1024U * 1024U)
#define PERVUE_FAKE_SLOW_DELAY_MS 700U

static int sleep_ms(unsigned int milliseconds) {
#ifdef _WIN32
  Sleep((DWORD)milliseconds);
  return 0;
#else
  struct timespec duration;
  struct timespec remaining;

  duration.tv_sec = (time_t)(milliseconds / 1000U);
  duration.tv_nsec = (long)((milliseconds % 1000U) * 1000000U);

  while (nanosleep(&duration, &remaining) != 0) {
    if (errno != EINTR) {
      return 1;
    }
    duration = remaining;
  }

  return 0;
#endif
}

static int write_line(FILE *stream, const char *line) {
  if (fputs(line, stream) == EOF ||
      fputc('\n', stream) == EOF ||
      fflush(stream) != 0) {
    return 1;
  }

  return 0;
}

static int stream_normal(int slow) {
  static const char *const lines[] = {
    "{\"type\":\"delta\",\"text\":\"alpha\"}",
    "{\"type\":\"delta\",\"text\":\" beta\"}",
    "{\"type\":\"completed\"}"
  };
  size_t index;

  for (index = 0U; index < sizeof(lines) / sizeof(lines[0]); ++index) {
    if (write_line(stdout, lines[index]) != 0) {
      return 1;
    }

    if (slow && index + 1U < sizeof(lines) / sizeof(lines[0])) {
      if (sleep_ms(PERVUE_FAKE_SLOW_DELAY_MS) != 0) {
        return 1;
      }
    }
  }

  return 0;
}

static int mode_stderr(void) {
  if (write_line(stderr, "fake-provider: deterministic stderr message") != 0) {
    return 1;
  }

  return stream_normal(0);
}

static int mode_exit_nonzero(void) {
  if (write_line(stderr, "fake-provider: exiting with status 42") != 0) {
    return 1;
  }

  return PERVUE_FAKE_EXIT_NONZERO;
}

static int hang_forever(void) {
  for (;;) {
    if (sleep_ms(1000U) != 0) {
      return 1;
    }
  }
}

static int mode_hang(void) {
  if (write_line(stdout, "{\"type\":\"ready\"}") != 0) {
    return 1;
  }

  return hang_forever();
}

static int mode_ignore_cancel(void) {
  char command[64];

  if (signal(SIGTERM, SIG_IGN) == SIG_ERR) {
    return 1;
  }

  if (write_line(stdout, "{\"type\":\"ready\"}") != 0) {
    return 1;
  }

  if (fgets(command, sizeof(command), stdin) != NULL &&
      strncmp(command, "cancel", 6U) == 0) {
    if (write_line(
            stderr,
            "fake-provider: cancellation ignored") != 0) {
      return 1;
    }
  }

  return hang_forever();
}

static int mode_malformed(void) {
  return write_line(stdout, "{not-json");
}

static int mode_large(void) {
  unsigned char block[4096];
  size_t remaining = (size_t)PERVUE_FAKE_LARGE_OUTPUT_SIZE;

  memset(block, 'x', sizeof(block));

  while (remaining > 0U) {
    size_t count = remaining < sizeof(block) ? remaining : sizeof(block);

    if (fwrite(block, 1U, count, stdout) != count) {
      return 1;
    }

    remaining -= count;
  }

  return fflush(stdout) == 0 ? 0 : 1;
}

static int usage(const char *program) {
  fprintf(
      stderr,
      "usage: %s --mode "
      "<normal|slow|stderr|exit-nonzero|hang|ignore-cancel|malformed|large>\n",
      program);
  return 64;
}

int main(int argc, char **argv) {
  const char *mode;

  if (argc != 3 || strcmp(argv[1], "--mode") != 0) {
    return usage(argv[0]);
  }

  mode = argv[2];

  if (strcmp(mode, "normal") == 0) {
    return stream_normal(0);
  }
  if (strcmp(mode, "slow") == 0) {
    return stream_normal(1);
  }
  if (strcmp(mode, "stderr") == 0) {
    return mode_stderr();
  }
  if (strcmp(mode, "exit-nonzero") == 0) {
    return mode_exit_nonzero();
  }
  if (strcmp(mode, "hang") == 0) {
    return mode_hang();
  }
  if (strcmp(mode, "ignore-cancel") == 0) {
    return mode_ignore_cancel();
  }
  if (strcmp(mode, "malformed") == 0) {
    return mode_malformed();
  }
  if (strcmp(mode, "large") == 0) {
    return mode_large();
  }

  return usage(argv[0]);
}
