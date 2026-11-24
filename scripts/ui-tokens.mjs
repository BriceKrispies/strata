// The token layer of the design system (docs/design-system.md, "Layer 1: tokens and themes").
//
//   node scripts/ui-tokens.mjs            write web/src/styles/generated/tokens-<preset>.css
//   node scripts/ui-tokens.mjs --check    exit 1 when a written file differs from what the catalogue gives
//
// `npm run generate` calls writeTokens() from scripts/regenerate.mjs.
//
// Reads specifications/ui-catalogue.jsonc (sections tokens, contrastPairs, themes). For each preset it
// resolves the full token set of every scheme and every variant combination, refuses the preset when a
// contrast pair falls under its minimum (naming pair, scheme, variant and ratio), and writes one
// stylesheet. The preset named `defaultTheme` applies when <html data-theme> is absent; any preset
// applies under <html data-theme="id">. Scoping is by :where() so every rule keeps the specificity of
// a plain :root, and a stylesheet keeps its cascade (base, variants, dark scheme, dark variants).
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseJsonc } from './jsonc.mjs';

const here = dirname(fileURLToPath(import.meta.url));
export const root = resolve(here, '..');
export const catalogueFile = join(root, 'specifications', 'ui-catalogue.jsonc');
export const outDir = join(root, 'web', 'src', 'styles', 'generated');

export function loadCatalogue(file = catalogueFile) {
  return parseJsonc(readFileSync(file, 'utf8'));
}

/** Expands `from`: a preset derived from another, with its own sections laid over the base's. */
export function expandPresets(catalogue) {
  const byId = new Map();
  const out = [];
  for (const theme of catalogue.themes) {
    let full = theme;
    if (theme.from) {
      const base = byId.get(theme.from);
      if (!base) throw new Error(`theme ${theme.id}: from names an unknown preset ${theme.from}`);
      full = {
        ...base,
        ...theme,
        shared: { ...base.shared, ...theme.shared },
        light: { ...base.light, ...theme.light },
        dark: { ...base.dark, ...theme.dark },
        variants: theme.variants ?? base.variants,
      };
    }
    byId.set(theme.id, full);
    out.push(full);
  }
  return out;
}

// ---- colour arithmetic (WCAG 2.x) ----

export function parseHex(value) {
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(value);
  if (!m) return null;
  const h = m[1].length === 3 ? [...m[1]].map((c) => c + c).join('') : m[1];
  return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16));
}

export function luminance(rgb) {
  const [r, g, b] = rgb.map((v) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function ratio(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

// ---- resolution ----

const REF = /^\{([a-z0-9-]+)\}$/;

/** The dimensions of a preset's variants, each with its default and the names it offers. */
function dimensions(theme) {
  return Object.entries(theme.variants ?? {}).map(([dim, names]) => ({
    dim,
    base: theme.defaults?.[dim] ?? null,
    names: Object.keys(names),
  }));
}

/** Follows {token} references to a final value within one resolved token map. */
function follow(values, id, seen = new Set()) {
  let v = values[id];
  while (typeof v === 'string' && REF.test(v)) {
    const next = REF.exec(v)[1];
    if (seen.has(next)) throw new Error(`token ${id}: reference cycle through ${next}`);
    seen.add(next);
    v = values[next];
  }
  return v;
}

/** The effective values of a scheme under one choice of variants ({dim: name | null}), as the cascade gives them. */
export function effective(theme, scheme, choice) {
  const values = { ...theme.shared, ...theme.light };
  const layers = (s) => Object.entries(choice).map(([dim, name]) => (name ? theme.variants[dim][name]?.[s] : null) ?? {});
  Object.assign(values, ...layers('light'));
  if (scheme === 'dark') {
    Object.assign(values, theme.dark);
    Object.assign(values, ...layers('dark'));
  }
  return values;
}

function* choices(theme) {
  const dims = dimensions(theme);
  const walk = function* (i, acc) {
    if (i === dims.length) {
      yield acc;
      return;
    }
    const { dim, names } = dims[i];
    for (const name of [null, ...names]) yield* walk(i + 1, { ...acc, [dim]: name });
  };
  yield* walk(0, {});
}

/** Structural problems of a preset against the token list: missing tokens, unknown tokens, bad references. */
export function structure(catalogue, theme) {
  const problems = [];
  const known = new Set(catalogue.tokens.map((t) => t.id));
  for (const section of ['shared', 'light', 'dark']) {
    for (const id of Object.keys(theme[section] ?? {})) {
      if (!known.has(id)) problems.push(`${theme.id}: ${section} gives unknown token ${id}`);
    }
  }
  for (const token of catalogue.tokens) {
    if (token.type === 'colour') {
      for (const s of ['light', 'dark']) {
        if (!(token.id in (theme[s] ?? {})) && !(token.id in (theme.shared ?? {}))) problems.push(`${theme.id}: colour token ${token.id} has no ${s} value`);
      }
    } else if (!(token.id in (theme.shared ?? {})) && !(token.id in (theme.light ?? {}))) {
      problems.push(`${theme.id}: token ${token.id} has no value`);
    }
  }
  for (const [dim, names] of Object.entries(theme.variants ?? {})) {
    if (!theme.defaults?.[dim]) problems.push(`${theme.id}: variant dimension ${dim} has no default`);
    for (const [name, schemes] of Object.entries(names)) {
      for (const s of ['light', 'dark']) {
        for (const id of Object.keys(schemes[s] ?? {})) if (!known.has(id)) problems.push(`${theme.id}: variant ${dim}=${name} gives unknown token ${id}`);
      }
    }
  }
  for (const scheme of ['light', 'dark']) {
    for (const choice of choices(theme)) {
      const values = effective(theme, scheme, choice);
      for (const [id, v] of Object.entries(values)) {
        if (typeof v === 'string' && REF.test(v) && !(REF.exec(v)[1] in values)) problems.push(`${theme.id}: ${id} refers to missing token ${v}`);
      }
    }
  }
  return problems;
}

function describe(choice) {
  const named = Object.entries(choice).filter(([, name]) => name);
  return named.length ? named.map(([dim, name]) => `${dim}=${name}`).join(', ') : 'default variant';
}

/**
 * Checks every contrast pair of the catalogue on a preset, in both schemes and every variant
 * combination. Returns the refusals: { pair, scheme, variant, ratio, min }.
 */
export function contrastFailures(catalogue, theme) {
  const failures = [];
  const accepted = new Map((theme.acceptedShortfalls ?? []).map((a) => [`${a.pair}|${a.scheme}`, a]));
  const used = new Set();
  for (const scheme of ['light', 'dark']) {
    for (const choice of choices(theme)) {
      const values = effective(theme, scheme, choice);
      for (const { fg, on, min } of catalogue.contrastPairs) {
        for (const bg of on) {
          const a = parseHex(follow(values, fg));
          const b = parseHex(follow(values, bg));
          const pair = `${fg} on ${bg}`;
          if (!a || !b) {
            failures.push({ pair, scheme, variant: describe(choice), ratio: NaN, min, note: 'not a #rrggbb colour' });
            continue;
          }
          const r = ratio(a, b);
          if (r >= min) continue;
          const key = `${pair}|${scheme}`;
          const waiver = accepted.get(key);
          if (waiver && r >= waiver.floor) {
            used.add(key);
            continue;
          }
          failures.push({ pair, scheme, variant: describe(choice), ratio: r, min, ...(waiver ? { note: `below its accepted floor ${waiver.floor}` } : {}) });
        }
      }
    }
  }
  // One refusal per pair and scheme: the worst ratio over the variants, so the message stays short.
  const worst = new Map();
  for (const f of failures) {
    const key = `${f.pair}|${f.scheme}`;
    const seen = worst.get(key);
    if (!seen || f.ratio < seen.ratio) worst.set(key, f);
  }
  failures.splice(0, failures.length, ...worst.values());
  for (const [key, waiver] of accepted) {
    if (!used.has(key)) failures.push({ pair: waiver.pair, scheme: waiver.scheme, variant: 'n/a', ratio: NaN, min: 0, note: 'accepted shortfall no longer needed; remove it' });
  }
  return failures;
}

export function formatFailure(theme, f) {
  const r = Number.isNaN(f.ratio) ? '' : ` is ${f.ratio.toFixed(2)}:1, minimum ${f.min}:1`;
  return `ui-tokens: preset ${theme.id}: ${f.pair} (${f.scheme} scheme, ${f.variant})${r}${f.note ? ` [${f.note}]` : ''}`;
}

/** Every refusal for the catalogue's presets, as lines. Empty when all presets are sound. */
export function refusals(catalogue) {
  const lines = [];
  for (const theme of expandPresets(catalogue)) {
    const problems = structure(catalogue, theme);
    if (problems.length) {
      lines.push(...problems.map((p) => `ui-tokens: ${p}`));
      continue;
    }
    lines.push(...contrastFailures(catalogue, theme).map((f) => formatFailure(theme, f)));
  }
  return lines;
}

// ---- rendering ----

function decl(id, value) {
  const m = REF.exec(value);
  return `--${id}: ${m ? `var(--${m[1]})` : value};`;
}

function block(selector, entries, extra = []) {
  const lines = [...entries.map(([id, v]) => decl(id, v)), ...extra];
  return `  ${selector} {\n${lines.map((l) => `    ${l}`).join('\n')}\n  }\n`;
}

/** The stylesheet of one preset. */
export function renderTheme(catalogue, theme) {
  const isDefault = catalogue.defaultTheme === theme.id;
  const scope = isDefault ? `:root:where(:not([data-theme]), [data-theme='${theme.id}'])` : `:root:where([data-theme='${theme.id}'])`;
  const order = new Map(catalogue.tokens.map((t, i) => [t.id, i]));
  const sorted = (obj) => Object.entries(obj).sort(([a], [b]) => order.get(a) - order.get(b));
  const dims = dimensions(theme);
  const out = [];
  out.push(`/* @generated by scripts/ui-tokens.mjs from specifications/ui-catalogue.jsonc (preset ${theme.id}). Do not edit; change the catalogue and run npm run generate. */\n`);
  out.push(`/* ${theme.summary} */\n`);
  out.push('@layer tokens {');
  out.push(block(scope, sorted({ ...theme.shared, ...theme.light }), ['color-scheme: light;']));
  for (const { dim, names } of dims) {
    for (const name of names) {
      const v = theme.variants[dim][name].light;
      if (v && Object.keys(v).length) out.push(block(`${scope}[data-${dim}='${name}']`, sorted(v)));
    }
  }
  out.push(`  /* Dark scheme. */\n${block(`${scope}[data-scheme='dark']`, sorted(theme.dark), ['color-scheme: dark;'])}`);
  for (const { dim, names } of dims) {
    for (const name of names) {
      const v = theme.variants[dim][name].dark;
      if (v && Object.keys(v).length) out.push(block(`${scope}[data-scheme='dark'][data-${dim}='${name}']`, sorted(v)));
    }
  }
  out.push('  /* Before the kernel has spoken, follow the device. */');
  out.push('  @media (prefers-color-scheme: dark) {');
  const fallback = [block(`${scope}:not([data-scheme])`, sorted(theme.dark), ['color-scheme: dark;'])];
  for (const { dim, names } of dims) {
    for (const name of names) {
      const v = theme.variants[dim][name].dark;
      if (v && Object.keys(v).length) fallback.push(block(`${scope}:not([data-scheme])[data-${dim}='${name}']`, sorted(v)));
    }
  }
  out.push(fallback.join('\n').trimEnd().replace(/^/gm, '  ').replace(/^ {2}$/gm, ''));
  out.push('  }\n');
  const reduced = catalogue.tokens.filter((t) => t.reduced !== undefined);
  if (reduced.length) {
    out.push('  @media (prefers-reduced-motion: reduce) {');
    out.push(block(scope, reduced.map((t) => [t.id, t.reduced])).trimEnd().replace(/^/gm, '  ').replace(/^ {2}$/gm, ''));
    out.push('  }');
  }
  out.push('}');
  return `${out.join('\n')}\n`.replace(/\n\n\n+/g, '\n\n');
}

/** The files a catalogue gives: { path: text }. Throws, listing every refusal, when a preset is refused. */
export function filesFor(catalogue, dir = outDir) {
  const lines = refusals(catalogue);
  if (lines.length) throw new Error(lines.join('\n'));
  const files = {};
  for (const theme of expandPresets(catalogue)) files[join(dir, `tokens-${theme.id}.css`)] = renderTheme(catalogue, theme);
  return files;
}

export function writeTokens({ catalogue = loadCatalogue(), dir = outDir, check = false } = {}) {
  const files = filesFor(catalogue, dir);
  const stale = [];
  for (const [path, text] of Object.entries(files)) {
    const current = existsSync(path) ? readFileSync(path, 'utf8') : null;
    if (current === text) continue;
    stale.push(path);
    if (!check) {
      mkdirSync(dirname(path), { recursive: true });
      writeFileSync(path, text);
    }
  }
  return stale;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const check = process.argv.includes('--check');
  try {
    const stale = writeTokens({ check });
    if (check && stale.length) {
      console.error(`ui-tokens: out of date: ${stale.join(', ')}; run npm run generate`);
      process.exit(1);
    }
    console.log(check ? 'ui-tokens: up to date' : stale.length ? `ui-tokens: wrote ${stale.length} stylesheet(s)` : 'ui-tokens: unchanged');
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
}
