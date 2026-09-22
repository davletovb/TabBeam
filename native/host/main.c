#include "pervue/host.h"

#include <stdio.h>
#include <string.h>

static int print_usage(const char *program_name) {
  fprintf(stderr, "usage: %s [--version]\n", program_name);
  return 64;
}

int main(int argc, char **argv) {
  if (argc == 2) {
    if (strcmp(argv[1], "--version") == 0) {
      puts(pervue_host_version());
      return 0;
    }

    return print_usage(argv[0]);
  }

  if (argc != 1) {
    return print_usage(argv[0]);
  }

  return (int)pervue_host_run(stdin, stdout);
}
