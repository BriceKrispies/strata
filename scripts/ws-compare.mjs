#!/usr/bin/env node
// Compare ws-bench reports: one table per scenario (and store), one column per mode.
//
//   node scripts/ws-compare.mjs report1.json report2.json dir-of-reports ...
//   node scripts/ws-compare.mjs --json reports/      (the same data as JSON)
//
// A column is a (mode, seed) pair. The `answers` row says whether the run's answer digest equals
// the digest of the `none` run of the same scenario, store and seed (a coherent mode must match);
// a mode with divergences is marked in the header.

import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

const MODE_ORDER = ["none", "header", "full", "stale"];

function collect(paths) {
  const files = [];
  for (const p of paths) {
    if (statSync(p).isDirectory()) {
      for (const name of readdirSync(p).sort()) {
        if (name.endsWith(".json")) files.push(join(p, name));
      }
    } else {
      files.push(p);
    }
  }
  return files;
}

function load(files) {
  const reports = [];
  for (const file of files) {
    let r;
    try {
      r = JSON.parse(readFileSync(file, "utf8"));
    } catch (e) {
      console.error(`skipping ${file}: ${e.message}`);
      continue;
    }
    if (!r || typeof r.scenario !== "string" || !r.timing) {
      console.error(`skipping ${file}: not a ws-bench report`);
      continue;
    }
    r.file = file;
    reports.push(r);
  }
  return reports;
}

const us = (ns) => (ns == null ? "-" : (ns / 1000).toFixed(ns < 10_000 ? 2 : 1));
const pct = (x) => (x == null ? "-" : (100 * x).toFixed(1) + "%");
const num = (x) => (x == null ? "-" : Number(x).toLocaleString("en-US", { maximumFractionDigits: 1 }));
const kb = (x) => (x == null ? "-" : (x / 1024).toFixed(0) + " KiB");

function rows(r, baseline) {
  const lat = r.timing.overall;
  const get = r.timing.by_kind?.get;
  const s = r.store_side;
  const w = r.wrapper;
  const c = r.coherence;
  const need = Object.entries(s.loads_by_need ?? {})
    .map(([k, v]) => `${k} ${v}`)
    .join(", ");
  return [
    ["p50 us (all ops)", us(lat.p50_ns)],
    ["p95 us", us(lat.p95_ns)],
    ["p99 us", us(lat.p99_ns)],
    ["max us", us(lat.max_ns)],
    ["get p50 / p99 us", get ? `${us(get.p50_ns)} / ${us(get.p99_ns)}` : "-"],
    ["throughput ops/s", num(r.timing.throughput_ops_per_s)],
    ["store loads", num(s.loads)],
    ["loads by need", need || "-"],
    ["load_complete", num(s.load_completes)],
    ["commits", num(s.commits)],
    ["schema decodes", s.schema_decodes == null ? "-" : num(s.schema_decodes)],
    ["bytes read", s.bytes_read == null ? "-" : kb(s.bytes_read)],
    ["resident bytes", w.placeholder ? "placeholder" : kb(w.resident_bytes)],
    ["hit rate (count)", w.placeholder ? "placeholder" : pct(w.hit_rate_by_count)],
    ["hit rate (cost)", w.placeholder ? "placeholder" : pct(w.hit_rate_by_cost)],
    ["policy refusals", num(r.refusals?.Unauthorized ?? 0)],
    ["refusals (all kinds)", Object.entries(r.refusals ?? {}).map(([k, v]) => `${k} ${v}`).join(", ") || "-"],
    ["faults ineffective", num(r.faults_ineffective ?? 0)],
    ["retries", num(r.conflicts.retries)],
    ["contended conflicts", num(r.conflicts.contended_conflicts)],
    ["coherence checked", num(c.checked)],
    ["divergences", num(c.divergences)],
    ["post-fault divergences", num(c.post_fault_divergences)],
    ["plaintext leaks", num(c.plaintext_leaks)],
    [
      "answers = none",
      baseline == null ? "-" : baseline === r.determinism.answer_digest ? "yes" : "NO",
    ],
  ];
}

function table(header, labels, columns) {
  const widths = [Math.max(...labels.map((l) => l.length), 6)];
  columns.forEach((col, i) =>
    widths.push(Math.max(header[i + 1].length, ...col.map((c) => String(c).length))),
  );
  const line = (cells) => cells.map((c, i) => String(c).padEnd(widths[i])).join("  ").trimEnd();
  const out = [line(header), line(widths.map((w) => "-".repeat(w)))];
  labels.forEach((label, r) => out.push(line([label, ...columns.map((col) => col[r])])));
  return out.join("\n");
}

function main() {
  const args = process.argv.slice(2);
  const asJson = args.includes("--json");
  const paths = args.filter((a) => a !== "--json");
  if (paths.length === 0) {
    console.error("usage: ws-compare.mjs [--json] <report.json | directory> ...");
    process.exit(2);
  }
  const reports = load(collect(paths));
  const groups = new Map();
  for (const r of reports) {
    const key = `${r.scenario}\u0000${r.store}`;
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(r);
  }
  const result = [];
  for (const [key, list] of [...groups].sort()) {
    const [scenario, store] = key.split("\u0000");
    list.sort(
      (a, b) =>
        a.seed - b.seed || MODE_ORDER.indexOf(a.mode) - MODE_ORDER.indexOf(b.mode),
    );
    const none = new Map(list.filter((r) => r.mode === "none").map((r) => [r.seed, r]));
    const header = ["measure"];
    const columns = [];
    let labels = null;
    for (const r of list) {
      const base = none.get(r.seed)?.determinism.answer_digest;
      const body = rows(r, r.mode === "none" ? null : base);
      labels ??= body.map((b) => b[0]);
      const flag =
        (r.coherence.divergences > 0 ? " (DIVERGES)" : "") +
        ((r.faults_ineffective ?? 0) > 0 ? " (FAULT NO-OP)" : "");
      header.push(`${r.mode}#${r.seed}${flag}`);
      columns.push(body.map((b) => b[1]));
    }
    const first = list[0];
    result.push({ scenario, store, first, header, labels, columns });
  }
  if (asJson) {
    console.log(
      JSON.stringify(
        result.map(({ scenario, store, header, labels, columns }) => ({
          scenario,
          store,
          columns: header.slice(1),
          rows: labels.map((l, i) => ({ measure: l, values: columns.map((c) => c[i]) })),
        })),
        null,
        2,
      ),
    );
    return;
  }
  for (const { scenario, store, first, header, labels, columns } of result) {
    console.log(`\n${scenario}  [${store}]  scale ${first.scale}, ${first.length} ops, ${first.clients} clients`);
    if (first.refutation) {
      console.log(`  refutes if (${first.refutation.criterion}) ${first.refutation.result}`);
    }
    console.log(
      table(
        header,
        labels,
        columns,
      )
        .split("\n")
        .map((l) => "  " + l)
        .join("\n"),
    );
    const aliased = Object.keys(first.object_aliases ?? {}).length;
    if (aliased) console.log(`  note: ${aliased} object type(s) ran as another type (stand-in tenant)`);
  }
}

main();
