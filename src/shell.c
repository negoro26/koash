#include "../include/shell.h"
#include <stdio.h>
#include <stdlib.h>

void shell_loop() {
  char *line;
  char **args;
  int status = 1;

  do {
    printf("-> ");
    line = read_line();
    args = split_line(line);

    free(line);
    free(args);
  } while (status);
}
