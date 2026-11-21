// Every table the SQLite adapters create has a shipped record in the specification (WG-5038, from
// audit finding 6 of reports/drift.md: persisted tables outside the retention, erasure and copy rules).
//
// A table is created by a `CREATE TABLE` statement in crates/storage-sqlite/src/lib.rs or
// crates/runtime/src/sqlite.rs. A shipped record (`realisation: shipped`) names it when its id is
// `tbl-<name>` with the underscores of the name as hyphens, or its description starts a sentence
// with `Table <name>` (for example "Table login_throttle of a tenant file: ...").

// The tables the sources create, as [{ name, path }].
export function createdTables(sources) {
  const out = [];
  for (const { path, text } of sources) {
    const code = text.replace(/^\s*\/\/.*$/gm, '');
    for (const m of code.matchAll(/\bCREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?([A-Za-z_][A-Za-z_0-9]*)/gi)) {
      out.push({ name: m[1], path });
    }
  }
  return out;
}

// True when the record names the table.
export function recordNamesTable(record, table) {
  if (record.realisation !== 'shipped') return false;
  if (record.id === `tbl-${table.replace(/_/g, '-')}`) return true;
  const escaped = table.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return new RegExp(`(^|[.!?]\\s+)Table ${escaped}\\b`).test(record.description ?? '');
}

// The names of the created tables no shipped record names, and how many tables there are.
export function tablesWithoutRecord(tables, records) {
  const names = [...new Set(tables.map((t) => t.name))];
  return { total: names.length, missing: names.filter((name) => !records.some((r) => recordNamesTable(r, name))) };
}
