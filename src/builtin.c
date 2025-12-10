#include <stdio.h>

char *builtin_str[] = {"cd", "help", "exit"};

int shell_num_builtins() { return sizeof(builtin_str) / sizeof(char *); };

int shell_cd(char **args) {
  if (args[1] == NULL) {
    fprintf(stderr, "shell: expected argument to \"cd\"\n");
  } else {
    if (0 != chdir(args[1])) {
      perror("shell");
    }
  }
  return 1;
}

int shell_help(char **args) {
  printf("Koa Shell\n");
  printf("Simple. Humble.\n");
  printf("The following are built in:\n");

  for (int i = 0; i < shell_num_builtins(); i++) {
    printf(" %s\n", builtin_str[i]);
  }

  return 1;
}

int shell_exit(char **args) { return 0; }
