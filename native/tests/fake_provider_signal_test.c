#ifndef _WIN32
#define _POSIX_C_SOURCE 200809L

#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static int sleep_ms(long milliseconds) {
  struct timespec duration;
  struct timespec remaining;

  duration.tv_sec = (time_t)(milliseconds / 1000L);
  duration.tv_nsec = (milliseconds % 1000L) * 1000000L;

  while (nanosleep(&duration, &remaining) != 0) {
    if (errno != EINTR) {
      return 1;
    }
    duration = remaining;
  }

  return 0;
}

static int read_ready_line(int fd) {
  static const char expected[] = "{\"type\":\"ready\"}\n";
  char buffer[sizeof(expected)];
  size_t offset = 0U;

  while (offset + 1U < sizeof(buffer)) {
    ssize_t count = read(fd, buffer + offset, 1U);

    if (count == 1) {
      if (buffer[offset] == '\n') {
        offset += 1U;
        break;
      }
      offset += 1U;
      continue;
    }

    if (count < 0 && errno == EINTR) {
      continue;
    }

    return 1;
  }

  buffer[offset] = '\0';
  return strcmp(buffer, expected) == 0 ? 0 : 1;
}

static pid_t spawn_mode(const char *program, const char *mode, int *read_fd) {
  int pipe_fds[2];
  pid_t pid;

  if (pipe(pipe_fds) != 0) {
    return (pid_t)-1;
  }

  pid = fork();
  if (pid < 0) {
    close(pipe_fds[0]);
    close(pipe_fds[1]);
    return (pid_t)-1;
  }

  if (pid == 0) {
    close(pipe_fds[0]);

    if (dup2(pipe_fds[1], STDOUT_FILENO) < 0) {
      _exit(125);
    }

    close(pipe_fds[1]);
    execl(program, program, "--mode", mode, (char *)NULL);
    _exit(126);
  }

  close(pipe_fds[1]);
  *read_fd = pipe_fds[0];
  return pid;
}

static int wait_for_exit(pid_t pid, int timeout_ms, int *status) {
  int elapsed = 0;

  while (elapsed <= timeout_ms) {
    pid_t result = waitpid(pid, status, WNOHANG);

    if (result == pid) {
      return 0;
    }

    if (result < 0) {
      return 1;
    }

    if (sleep_ms(20L) != 0) {
      return 1;
    }

    elapsed += 20;
  }

  return 2;
}

static void force_reap(pid_t pid) {
  int status;

  (void)kill(pid, SIGKILL);
  while (waitpid(pid, &status, 0) < 0 && errno == EINTR) {
  }
}

static int test_hang_honors_sigterm(const char *program) {
  int read_fd = -1;
  int status = 0;
  pid_t pid = spawn_mode(program, "hang", &read_fd);
  int wait_result;

  if (pid < 0) {
    return 1;
  }

  if (read_ready_line(read_fd) != 0) {
    close(read_fd);
    force_reap(pid);
    return 1;
  }

  close(read_fd);

  if (kill(pid, SIGTERM) != 0) {
    force_reap(pid);
    return 1;
  }

  wait_result = wait_for_exit(pid, 1000, &status);
  if (wait_result != 0) {
    force_reap(pid);
    return 1;
  }

  return WIFSIGNALED(status) && WTERMSIG(status) == SIGTERM ? 0 : 1;
}

static int test_ignore_cancel_ignores_sigterm(const char *program) {
  int read_fd = -1;
  int status = 0;
  pid_t pid = spawn_mode(program, "ignore-cancel", &read_fd);
  pid_t wait_result;

  if (pid < 0) {
    return 1;
  }

  if (read_ready_line(read_fd) != 0) {
    close(read_fd);
    force_reap(pid);
    return 1;
  }

  close(read_fd);

  if (kill(pid, SIGTERM) != 0) {
    force_reap(pid);
    return 1;
  }

  if (sleep_ms(300L) != 0) {
    force_reap(pid);
    return 1;
  }

  wait_result = waitpid(pid, &status, WNOHANG);
  if (wait_result != 0) {
    if (wait_result < 0 && errno == EINTR) {
      force_reap(pid);
    }
    return 1;
  }

  if (kill(pid, SIGKILL) != 0) {
    force_reap(pid);
    return 1;
  }

  while (waitpid(pid, &status, 0) < 0) {
    if (errno != EINTR) {
      return 1;
    }
  }

  return WIFSIGNALED(status) && WTERMSIG(status) == SIGKILL ? 0 : 1;
}

int main(int argc, char **argv) {
  if (argc != 2) {
    fprintf(stderr, "usage: %s <fake-provider-path>\n", argv[0]);
    return 64;
  }

  if (test_hang_honors_sigterm(argv[1]) != 0) {
    fprintf(stderr, "hang mode did not terminate on SIGTERM\n");
    return 1;
  }

  if (test_ignore_cancel_ignores_sigterm(argv[1]) != 0) {
    fprintf(stderr, "ignore-cancel mode did not survive SIGTERM\n");
    return 1;
  }

  return 0;
}

#endif
