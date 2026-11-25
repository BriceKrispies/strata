// Reads the output of `specc check-all`: one process that runs every freshness check of the gate.
// Each pass writes `specc-pass <id> begin` and `specc-pass <id> end <exit code>` on both standard
// output and standard error; this splits the two streams into one result per pass, shaped like the
// result of a separate `specc` run (`ok`, `code`, `error`, `signal`, `timedOut`, `stdout`, `out`),
// so the gate prints the same line for each check as it did when each ran as its own process.

const MARK = /^specc-pass (\S+) (begin|end)(?: (\d+))?\s*$/;

// The lines of `text` between the markers of each pass: id -> { lines, code }.
function segments(text) {
  const passes = new Map();
  let current = null;
  for (const line of text.split(/\r?\n/)) {
    const m = MARK.exec(line);
    if (m && m[2] === 'begin') {
      current = { lines: [], code: null };
      passes.set(m[1], current);
    } else if (m && m[2] === 'end') {
      if (current) current.code = Number(m[3]);
      current = null;
    } else if (current) {
      current.lines.push(line);
    }
  }
  return passes;
}

const joined = (segment) => (segment && segment.lines.length ? `${segment.lines.join('\n')}\n` : '');

/**
 * The result of each pass id in `ids` from the whole `specc check-all` result `whole`
 * (`{ ok, code, error, signal, timedOut, stdout, stderr }`; `stderr` is what the gate's runner kept
 * apart). A pass that never reported an end line (the process crashed or timed out first) fails,
 * with the whole run's reason.
 */
export function splitPasses(whole, ids) {
  const out = segments(whole.stdout ?? '');
  const err = segments(whole.stderr ?? '');
  const results = new Map();
  for (const id of ids) {
    const stdoutPass = out.get(id);
    const code = stdoutPass?.code ?? null;
    const stdout = joined(stdoutPass);
    const text = `${stdout}${joined(err.get(id))}`;
    if (code === null) {
      results.set(id, {
        ok: false,
        code: null,
        error: whole.error ?? `specc check-all ended (exit ${whole.code}) before pass ${id} reported a result`,
        signal: whole.signal ?? null,
        timedOut: whole.timedOut === true,
        stdout,
        out: text,
      });
    } else {
      results.set(id, { ok: code === 0, code, error: null, signal: null, timedOut: false, stdout, out: text });
    }
  }
  return results;
}
