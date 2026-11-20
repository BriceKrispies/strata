// Every environment variable the backend and the BFF read is declared in the specification
// (WG-5038; claims RUN-1387 for the backend and GWY-1400 for the gateway list them).
//
// A variable is read when its name is a whole string literal in the source: `var("STRATA_DB", ..)`,
// `number("STRATA_LAB_QUEUE", ..)`, or a name passed as an argument. Comments are not code and name
// nothing. A variable is declared when its name appears as a word in the text of the specification.

const NAME = /"(STRATA_[A-Z0-9_]+)"/g;

// The variables the sources read, as [{ name, path }] in order of first appearance.
export function envVarsRead(sources) {
  const out = [];
  const seen = new Set();
  for (const { path, text } of sources) {
    const code = text.replace(/^\s*\/\/.*$/gm, '');
    for (const m of code.matchAll(NAME)) {
      if (!seen.has(m[1])) {
        seen.add(m[1]);
        out.push({ name: m[1], path });
      }
    }
  }
  return out;
}

// The names in `read` that `specText` does not contain as a whole word.
export function undeclaredEnvVars(read, specText) {
  const declared = new Set([...specText.matchAll(/\bSTRATA_[A-Z0-9_]+\b/g)].map((m) => m[0]));
  return read.map((v) => v.name).filter((name) => !declared.has(name));
}
