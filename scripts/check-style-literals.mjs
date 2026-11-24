// Components read semantic tokens only (docs/design-system.md, "Layer 1: tokens and themes"). This
// check fails when a stylesheet in web/src/styles, outside web/src/styles/generated, names a colour
// literal (hex, a colour function, a colour name) or a font family that is not a token.
//
//   node scripts/check-style-literals.mjs
//
// `transparent`, `currentColor` and `inherit` are keywords, not colours of a theme, and are allowed.
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
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

export function checkStyles(dir = stylesDir) {
  const problems = [];
  for (const file of cssFiles(dir)) {
    for (const f of findLiterals(readFileSync(file, 'utf8'))) {
      problems.push(`${relative(root, file).replaceAll('\\', '/')}:${f.line}: literal ${f.kind} in ${f.property}: ${f.value}`);
    }
  }
  return problems;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const problems = checkStyles();
  if (problems.length) {
    console.error(`${problems.join('\n')}\nstyle literals: components read tokens only; add a token to specifications/ui-catalogue.jsonc`);
    process.exit(1);
  }
  console.log('style literals: none outside the generated token stylesheets');
}
