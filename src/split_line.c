#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define BUFFER_SIZE 64
#define TOK_DELIM " \t\r\n\a"

char **split_line(char *line) {
  int bufsize = BUFFER_SIZE, position = 0;
  char **tokens = malloc(bufsize * sizeof(char *));
  char *token;

  if (!tokens) {
    fprintf(stderr, "buffer allocation error");
    exit(EXIT_FAILURE);
  }

  token = strtok(line, TOK_DELIM);
  while (token != NULL) {
    tokens[position] = token;
    position++;

    if (position >= bufsize) {
      bufsize += BUFFER_SIZE;
      tokens = realloc(tokens, bufsize * sizeof(char *));

      if (!tokens) {
        fprintf(stderr, "buffer allocation error");
        exit(EXIT_FAILURE);
      }
    }
    token = strtok(NULL, TOK_DELIM);
  }
  tokens[position] = NULL;

  return tokens;
}

// cred ca trebuie folosit get delim aici ca sa separam tokenurile??
// asta daca functioneaza asa, daca nu ne trebuie ceva cu strtok (proababil)
// UPDATE : getdelim n are nicio treaba, trebuie sa invat sa descifrez
// documentatie
