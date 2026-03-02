# Koa Shell

Koa Shell is a simple, humble command-line interpreter (shell) written entirely in C. I created this project as a personal endeavor and as my final project for my Operating Systems course during my second year of studying Computer Science.

## About the Project

The goal of this project was to understand the fundamental concepts behind how an operating system interacts with the user and manages processes. By building a shell from scratch, I was able to peek under the hood of programs I use every day as a developer.

This shell implements a classic REPL (Read-Evaluate-Print Loop) structure:
1. Prompts the user for input with `-> `.
2. Reads the line of input from standard input.
3. Parses and splits the line into appropriate commands and arguments.
4. Executes the commands either natively (if they are built-in) or by creating new processes.

## Features

- **Built-in commands:**
  - `cd`: Change the current working directory.
  - `help`: Display a simple help snippet about the shell.
  - `exit`: Terminate the shell loop and exit the program.
- **External commands:** Executes standard system programs (like `ls`, `pwd`, or `echo`) by relying on the OS's environment path.
- **Dynamic parsing:** The shell allocates memory dynamically for parsing input strings and argument arrays, expanding the buffer sizes as necessary while the user types.

## What I Learned

Throughout the development of Koa Shell, I gained practical experience with several core C programming and operating system concepts:

- **Process Management:** I learned how to deal with process creation and termination using system calls. Specifically, using `fork()` to create a new child process, `execvp()` to replace the child process's image with a new executable, and `waitpid()` to make the parent process wait for the child's execution to complete.
- **Memory Management:** Parsing the command line input required dynamic memory allocation using `malloc`, `realloc`, and `getline`. This reinforced my understanding of pointers, arrays of strings, and importantly, preventing memory leaks by properly using `free()`.
- **System Calls vs Library Functions:** I gained a better understanding of the boundary between user space and kernel space, specifically differentiating when to use standard C library functions versus making direct system calls.
- **String Manipulation in C:** Working with `strtok()` for parsing and tokenizing user input based on specific delimiters taught me how C handles strings and char arrays at a low level.
- **Modular Code Design:** I structured the project around separating the underlying concerns, keeping reading logic, command execution, and built-ins in their respective areas while using a unity build approach to combine them.

## Compilation and Usage

This project uses a unity build system; all dependencies are `#include`d directly in `main.c` for simplicity.

1. Ensure you have a C compiler like `gcc` installed on your system.
2. Compile the shell by running:
   ```bash
   gcc src/main.c -o myshell
   ```
3. Run the compiled executable:
   ```bash
   ./myshell
   ```

Building Koa Shell started as an academic requirement but quickly became a deeply satisfying puzzle that helped solidify my understanding of operating systems theory. I'm very proud of having my own custom shell running natively.
