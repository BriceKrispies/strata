// Runs one child process for the gate: an argument array, no shell (`shell` is for `npm` on
// Windows only, with constant arguments), and a timeout that kills it. `ok` is true only when the
// command started, was not killed, did not time out and exited with 0. A timeout is reported as
// `timedOut` and is never a pass.
//
// `interleaved` sends stdout and stderr to one file, so the text keeps the order the child wrote it
// in. cargo test writes its `Running` lines to stderr and its results to stdout; pairing them needs
// that order. An interleaved run returns the combined text as both `stdout` and `out`.
import { spawnSync } from 'node:child_process';
import { closeSync, mkdtempSync, openSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

export function runCommand(command, commandArgs, { cwd, env, timeoutMs, shell = false, interleaved = false }) {
  const options = {
    cwd,
    env,
    timeout: timeoutMs,
    killSignal: 'SIGKILL',
    shell: shell === true && process.platform === 'win32',
  };
  let done;
  let combined = null;
  if (interleaved) {
    const dir = mkdtempSync(join(tmpdir(), 'strata-gate-'));
    const file = join(dir, 'output.txt');
    const fd = openSync(file, 'w');
    try {
      done = spawnSync(command, commandArgs, { ...options, stdio: ['ignore', fd, fd] });
    } finally {
      closeSync(fd);
      combined = readFileSync(file, 'utf8');
      rmSync(dir, { recursive: true, force: true });
    }
  } else {
    done = spawnSync(command, commandArgs, { ...options, encoding: 'utf8', maxBuffer: 1 << 28 });
  }
  const timedOut = done.error?.code === 'ETIMEDOUT';
  const error = done.error ? (timedOut ? `timed out after ${Math.round(timeoutMs / 1000)}s` : done.error.message) : null;
  const ok = error === null && done.signal === null && done.status === 0;
  return {
    ok,
    code: done.status,
    signal: done.signal,
    error,
    timedOut,
    stdout: combined ?? done.stdout ?? '',
    out: combined ?? `${done.stdout ?? ''}${done.stderr ?? ''}`,
  };
}
