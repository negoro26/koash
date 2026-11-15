# Interactive job control

At the `koa>` prompt, Ctrl-C interrupts the foreground command and leaves the shell running with `$?` set to `130`. Ctrl-Z stops it, `jobs` lists it as `Stopped`, and `fg` resumes it. Tab completes command and file names.

## Sub-features

- `jc-interrupt` stops a foreground external with Ctrl-C.
- `jc-interrupt-pipeline` stops a pipeline that mixes a builtin and an external.
- `jc-suspend` suspends with Ctrl-Z and lists the job.
- `jc-resume` resumes with `fg`.
- `jc-complete` completes a command name with Tab.

## How to get to it (user POV)

- Run `koa` with no arguments in a terminal and type at the `koa>` prompt.

## Driving it with koa-verify

Preconditions:

- None beyond the baseline. The helper gives each pty drive its own `HOME`.

- **Interrupt.** Run `$V pty jc-interrupt 'send:/bin/sleep 30\r' 'sleep:0.5' 'send:\x03' 'send:echo st=$?\r'`. `grep -c 'st=130' target/verify/$KOA_VERIFY_RUN/jc-interrupt/transcript.txt` prints `1` or more and `koa_alive_after_steps` is `true`.
- **Interrupt a mixed pipeline.** Run `$V pty jc-interrupt-pipeline 'send:cat f | /bin/sleep 30\r' 'sleep:0.5' 'send:\x03' 'send:echo st=$?\r' --seed f='x\n'`. The transcript contains `st=130`.
- **Suspend, list, resume.** Run `$V pty jc-suspend 'send:/bin/sleep 30\r' 'sleep:0.5' 'send:\x1a' 'send:jobs\r' 'send:fg\r' 'sleep:0.5' 'send:\x03' 'send:echo st=$?\r'`. The transcript contains `Stopped` and then `st=130`.
- **Tab completion.** Run `$V pty jc-complete 'send:disco\t' 'sleep:0.3'`. The transcript contains `discover`.

## Gotchas

- Send `\r` to submit a line. `\n` is not Enter in raw mode.
- Give a command time to start (`sleep:0.5`) before sending Ctrl-C or Ctrl-Z, or the signal lands on the prompt instead.
- The transcript repeats the prompt for every keystroke because the editor redraws the line. Assert with `grep -c`, not by line position.
- A builtin-only pipeline cannot be suspended. Ctrl-Z on it prints `pipelines with builtin stages cannot be suspended yet`.
