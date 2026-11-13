# RPC sessions

Agents run `koa --rpc` and send one JSON request per line. A session keeps its working directory, environment, undo records and sub-kernels between `exec` calls. Each `exec` streams its output as events and ends with a `completed` event carrying the exit code.

## Sub-features

- `rpc-create` returns a session id.
- `rpc-exec` streams stdout and stderr as `stream_chunk` events and ends with `completed`.
- `rpc-status` reports the exit code of the last command and keeps `||` recovery.
- `rpc-timeout` stops a command after `timeout_ms` with exit code `124`.
- `rpc-isolation` keeps external program output inside protocol messages.
- `rpc-state` keeps `cd` and variables between `exec` calls.

## How to get to it (user POV)

- Start `koa --rpc` and write JSON lines to its stdin.
- `koa --socket PATH` speaks the same protocol on a Unix socket.

## Driving it with koa-verify

Preconditions:

- None beyond the baseline.

- **Create and exec.** Run `$V rpc rpc-exec '{"id":1,"method":"session.create"}' '{"id":2,"method":"exec","params":{"session_id":"$SID","command":"/bin/echo hi; cat nope.txt || echo recovered"}}'`. Replies contain `{"id":1,"result":{"session_id":"sess-1"}}`, a `stream_chunk` with `stdout` `hi\nrecovered\n`, a `stream_chunk` with `stderr` containing `E_CAT`, and `completed` with `exit_code` `0`.
- **No raw output on the stream.** In the same `result.json`, no reply is a `NON_JSON_LINE` object.
- **Timeout.** Run `$V rpc rpc-timeout '{"id":1,"method":"session.create"}' '{"id":2,"method":"exec","params":{"session_id":"$SID","command":"sleep 5","timeout_ms":200}}'`. The `completed` event has `exit_code` `124` and `duration_ms` under `1000`.
- **State between calls.** Run `$V rpc rpc-state '{"id":1,"method":"session.create"}' '{"id":2,"method":"exec","params":{"session_id":"$SID","command":"cd sub; export X=7"}}' '{"id":3,"method":"exec","params":{"session_id":"$SID","command":"pwd; echo $X"}}' --seed sub/f='x\n'`. The id `3` `process_spawned` event has a `cwd` ending in `/sub`, a `stream_chunk` carries `records` with that `path`, and another carries `stdout` `7\n`.

## Gotchas

- `session.create` takes no params. The first session in a process is `sess-1`.
- An `exec` on a session whose previous `exec` timed out but has not finished returns `E_SESSION_BUSY`. Wait or use a new session.
- `policy` with any restriction blocks lines that need the `sh -c` fallback (`$(...)`, loops). An `exec` without `policy` is unrestricted.
- The helper waits up to 30 s per request. A longer `exec` needs its own `timeout_ms`.
- Builtins that return tables send `stream_chunk` events with a `records` array instead of `stdout`. Check both keys.
