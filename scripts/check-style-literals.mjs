// Components read semantic tokens only (docs/design-system.md, "Layer 1: tokens and themes"). This
// check fails when a stylesheet in web/src/styles, outside web/src/styles/generated, names a colour
// literal (hex, a colour function, a colour name) or a font family that is not a token, and when it
// draws a pill (D-DS-31, "No pills" below).
//
//   node scripts/check-style-literals.mjs [dir]
//
// `transparent`, `currentColor` and `inherit` are keywords, not colours of a theme, and are allowed.
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { basename, dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const stylesDir = join(root, 'web', 'src', 'styles');

const COLOUR_PROPERTY = /^(color|background(-color|-image)?|border(-(inline|block))?(-(start|end))?(-color)?|border-(top|right|bottom|left)(-color)?|outline(-color)?|fill|stroke|text-decoration(-color)?|accent-color|caret-color|column-rule(-color)?|box-shadow|text-shadow|filter)$/;
const COLOUR_FUNCTION = /\b(rgb|rgba|hsl|hsla|hwb|lab|lch|oklab|oklch|color|color-mix)\(/i;
const HEX = /#(?:[0-9a-f]{8}|[0-9a-f]{6}|[0-9a-f]{4}|[0-9a-f]{3})\b/i;
const NAMES = 'aliceblue antiquewhite aqua aquamarine azure beige bisque black blanchedalmond blue blueviolet brown burlywood cadetblue chartreuse chocolate coral cornflowerblue cornsilk crimson cyan darkblue darkcyan darkgray darkgreen darkgrey darkkhaki darkmagenta darkolivegreen darkorange darkorchid darkred darksalmon darkseagreen darkslateblue darkslategray darkturquoise darkviolet deeppink deepskyblue dimgray dimgrey dodgerblue firebrick floralwhite forestgreen fuchsia gainsboro ghostwhite gold goldenrod gray green greenyellow grey honeydew hotpink indianred indigo ivory khaki lavender lavenderblush lawngreen lemonchiffon lightblue lightcoral lightcyan lightgoldenrodyellow lightgray lightgreen lightgrey lightpink lightsalmon lightseagreen lightskyblue lightslategray lightsteelblue lightyellow lime limegreen linen magenta maroon mediumaquamarine mediumblue mediumorchid mediumpurple mediumseagreen mediumslateblue mediumspringgreen mediumturquoise mediumvioletred midnightblue mintcream mistyrose moccasin navajowhite navy oldlace olive olivedrab orange orangered orchid palegoldenrod palegreen paleturquoise palevioletred papayawhip peachpuff peru pink plum powderblue purple rebeccapurple red rosybrown royalblue saddlebrown salmon sandybrown seagreen seashell sienna silver skyblue slateblue slategray snow springgreen steelblue tan teal thistle tomato turquoise violet wheat white whitesmoke yellow yellowgreen'.split(' ');
const NAME = new RegExp(`(?<![\\w-])(${NAMES.join('|')})(?![\\w-])`, 'i');

/** Removes comments and string contents while keeping line numbers. */
function strip(css) {
  return css.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ' '));
}

/** Every literal colour or font family in a stylesheet: [{ line, property, value, kind }]. */
export function findLiterals(css) {
  const found = [];
  const text = strip(css);
  const re = /([a-z-]+)\s*:\s*([^;{}]+)(?=[;}])/gi;
  let m;
  while ((m = re.exec(text))) {
    const property = m[1].toLowerCase();
    const value = m[2].trim();
    const line = text.slice(0, m.index).split('\n').length;
    if (property.startsWith('--')) continue;
    if (property === 'font-family' && !/^var\(--[a-z0-9-]+\)$/.test(value)) {
      found.push({ line, property, value, kind: 'font family' });
      continue;
    }
    if (property === 'font' && /["'][^"']+["']|,/.test(value)) {
      found.push({ line, property, value, kind: 'font family' });
      continue;
    }
    if (!COLOUR_PROPERTY.test(property)) continue;
    const bare = value.replace(/var\([^)]*\)/g, ' ').replace(/url\([^)]*\)/g, ' ');
    if (HEX.test(bare) || COLOUR_FUNCTION.test(bare) || NAME.test(bare)) found.push({ line, property, value, kind: 'colour' });
  }
  return found;
}

function* cssFiles(dir) {
  for (const name of readdirSync(dir).sort()) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) {
      if (name !== 'generated') yield* cssFiles(path);
    } else if (name.endsWith('.css')) yield path;
  }
}

// ---- No pills (D-DS-31) -------------------------------------------------------------------------
//
// The design system has no capsule labels: a state is said in words where it departs from normal.
// So that pills cannot come back, a stylesheet may not give anything a pill radius, and may not
// name the classes a capsule label would have.
//
// - A pill radius: `9999px`, `999px`, `100vmax`, the removed `--radius-pill` token, or `50%` on a
//   rule whose selector does not name an avatar or a dot (a circle is an avatar or a dot, nothing
//   else).
// - The class names `ds-badge`, `ds-chip` and `ds-pill`, with any modifier.
// - In the design system's own stylesheet (ds*.css), any radius but a radius token (radius-1 to
//   radius-3), 0, inherit, or 50% on an avatar or a dot: a radius computed from an element's height
//   would be a pill by another name. (Fieldnotes' publication chrome keeps the rounded ends it was
//   drawn with, as radii of at least half their height; it is not a design-system primitive.)

const PILL_RADIUS = /(^|[\s(,])(9999px|999px|100vmax)(?=$|[\s),;])|var\(\s*--radius-pill\s*\)/i;
const HALF = /(^|[\s(,])50%(?=$|[\s),;])/;
const CIRCLE_SELECTOR = /\.[\w-]*(avatar|dot)\b/i;
const DS_RADIUS = /^(0|inherit|var\(--radius-[123]\))(\s+(0|var\(--radius-[123]\)))*$/;
const PILL_CLASS = /\.(ds-badge|ds-chip|ds-pill)(?![\w])|\.(ds-badge|ds-chip|ds-pill)-/;

/**
 * Every pill in a stylesheet: [{ line, selector, property, value, kind }]. Rules are read with the
 * selector of the block they sit in, through @layer, @media and @container blocks.
 */
export function findPills(css, { designSystem = false } = {}) {
  const found = [];
  const text = strip(css);
  /** The open blocks: the prelude of each, an at-rule or a selector. */
  const stack = [];
  let start = 0;
  const lineAt = (index) => text.slice(0, index).split('\n').length;
  const selector = () => ([...stack].reverse().find((p) => !p.startsWith('@')) ?? '').replace(/\s+/g, ' ');
  for (let i = 0; i < text.length; i += 1) {
    const c = text[i];
    if (c === '{') {
      const prelude = text.slice(start, i).trim();
      if (!prelude.startsWith('@')) {
        const m = PILL_CLASS.exec(prelude);
        if (m) found.push({ line: lineAt(start + text.slice(start, i).indexOf(m[0])), selector: prelude.replace(/\s+/g, ' '), property: 'selector', value: m[0], kind: 'capsule class' });
      }
      stack.push(prelude);
      start = i + 1;
    } else if (c === '}' || c === ';') {
      const declaration = text.slice(start, i);
      const m = /^\s*([a-z-]+)\s*:\s*([\s\S]+?)\s*$/i.exec(declaration);
      if (m && /radius$/i.test(m[1])) {
        const property = m[1].toLowerCase();
        const value = m[2];
        const owner = selector();
        const line = lineAt(start + declaration.search(/\S/));
        if (PILL_RADIUS.test(value)) found.push({ line, selector: owner, property, value, kind: 'pill radius' });
        else if (HALF.test(value)) {
          if (!CIRCLE_SELECTOR.test(owner)) found.push({ line, selector: owner, property, value, kind: 'pill radius (50% on something that is not an avatar or a dot)' });
        } else if (designSystem && !DS_RADIUS.test(value.trim())) found.push({ line, selector: owner, property, value, kind: 'radius that is not a radius token' });
      }
      if (c === '}') stack.pop();
      start = i + 1;
    }
  }
  return found;
}

export function checkStyles(dir = stylesDir) {
  const problems = [];
  for (const file of cssFiles(dir)) {
    const css = readFileSync(file, 'utf8');
    const name = relative(root, file).replaceAll('\\', '/');
    for (const f of findLiterals(css)) {
      problems.push(`${name}:${f.line}: literal ${f.kind} in ${f.property}: ${f.value}`);
    }
    for (const f of findPills(css, { designSystem: /^ds[\w-]*\.css$/.test(basename(file)) })) {
      problems.push(`${name}:${f.line}: ${f.kind} in ${f.selector} (${f.property}: ${f.value}); the design system has no pills (D-DS-31)`);
    }
  }
  return problems;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const problems = checkStyles(process.argv[2] ? resolve(process.argv[2]) : stylesDir);
  if (problems.length) {
    console.error(`${problems.join('\n')}\nstyle literals: components read tokens only and draw no pills; add a token to specifications/ui-catalogue.jsonc, and say a state in words (D-DS-31)`);
    process.exit(1);
  }
  console.log('style literals: none outside the generated token stylesheets, and no pills');
}
