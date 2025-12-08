#include <stdio.h>
#include <stdlib.h>

char *read_line() {
  char *line = NULL;
  size_t bufsize = 0;

  if (-1 == getline(&line, &bufsize, stdin)) {
    if (feof(stdin)) {
      exit(EXIT_SUCCESS);
    } else {
      perror("readLine");
      exit(EXIT_FAILURE);
    }
  }

  return line;
}
