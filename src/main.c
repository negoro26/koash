/* UNITY BUILD
 *   Include all source files directly here
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

// Include helper functions
#include "read_line.c"
#include "split_line.c"

// Include builtins (the implementations)
#include "builtin.c" // <--- You'll create this

// Include execution logic (uses builtins)
#include "execute.c"

// Include main loop (uses execute, read_line, split_line)
#include "shell.c"

int main(int argc, char **argv) {
  shell_loop();
  return EXIT_SUCCESS;
}
