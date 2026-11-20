// Runs one child process for the gate: an argument array, no shell (`shell` is for `npm` on
// Windows only, with constant arguments), and a timeout that kills it. `ok` is true only when the
// command started, was not killed, did not time out and exited with 0. A timeout is reported as
// `timedOut` and is never a pass.
import { spawnSync } from 'node:child_process';

export function runCommand(command, commandArgs, { cwd, env, timeoutMs, shell = false }) {
  const done = spawnSync(command, commandArgs, {
    cwd,
    env,
    encoding: 'utf8',
    maxBuffer: 1 << 28,
    shell: shell === true && process.platform === 'win32',
    timeout: timeoutMs,
    killSignal: 'SIGKILL',
  });
  const timedOut = done.error?.code === 'ETIMEDOUT';
  const error = done.error ? (timedOut ? `timed out after ${Math.round(timeoutMs / 1000)}s` : done.error.message) : null;
  const ok = error === null && done.signal === null && done.status === 0;
  return {
    ok,
    code: done.status,
    signal: done.signal,
    error,
    timedOut,
    stdout: done.stdout ?? '',
    out: `${done.stdout ?? ''}${done.stderr ?? ''}`,
  };
}
