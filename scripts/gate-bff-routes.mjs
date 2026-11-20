// The routes the BFF mounts (crates/bff/src/routes.rs, `fn router`) and the contracts that name them.
//
// A contract names its BFF route in `bffRoute`: "<METHOD> <path> [what the route does]", for
// example "POST /api/members/assign-role". A route the router mounts has a contract when some
// bffRoute has the same method and a path the mounted pattern matches (`:name` matches one segment,
// `*name` the rest).

const METHODS = ['get', 'post', 'put', 'patch', 'delete', 'any'];

// The index just after the parenthesis that closes the one at `open`, or -1. String literals are
// skipped, so a parenthesis inside one does not count.
function closeOf(text, open) {
  let depth = 0;
  for (let i = open; i < text.length; i += 1) {
    const c = text[i];
    if (c === '"') {
      i += 1;
      while (i < text.length && text[i] !== '"') i += text[i] === '\\' ? 2 : 1;
    } else if (c === '(') {
      depth += 1;
    } else if (c === ')') {
      depth -= 1;
      if (depth === 0) return i + 1;
    }
  }
  return -1;
}

function stripComments(source) {
  return source.replace(/^\s*\/\/.*$/gm, '');
}

// Every route the function `router` mounts, as { method, path, line }. `method` is GET, POST and so
// on, or ANY. A chain that is nested (`.nest("/api", api)`) has the prefix on its paths. Static file
// services (`nest_service`) and the fallback mount no route and are not listed.
export function mountedRoutes(source) {
  const start = source.search(/\bfn\s+router\s*\(/);
  if (start < 0) return [];
  const body = stripComments(source.slice(start));
  const chains = [...body.matchAll(/(?:let\s+(\w+)\s*=\s*)?Router::new\(\)/g)].map((m, i, all) => ({
    name: m[1] ?? null,
    from: m.index + m[0].length,
    to: i + 1 < all.length ? all[i + 1].index : body.length,
  }));
  const prefixes = new Map();
  for (const chain of chains) {
    const text = body.slice(chain.from, chain.to);
    for (const nest of text.matchAll(/\.nest\(\s*"([^"]*)"\s*,\s*(\w+)\s*\)/g)) prefixes.set(nest[2], nest[1]);
  }
  const out = [];
  for (const chain of chains) {
    const text = body.slice(chain.from, chain.to);
    const prefix = chain.name ? (prefixes.get(chain.name) ?? '') : '';
    for (const call of text.matchAll(/\.route\(/g)) {
      const open = call.index + call[0].length - 1;
      const end = closeOf(text, open);
      if (end < 0) continue;
      const args = text.slice(open + 1, end - 1);
      const path = args.match(/^\s*"([^"]*)"\s*,/);
      if (!path) continue;
      const rest = args.slice(path[0].length);
      const methods = [...rest.matchAll(new RegExp(`\\b(${METHODS.join('|')})\\s*\\(`, 'g'))].map((m) => m[1].toUpperCase());
      const line = body.slice(0, chain.from + call.index).split('\n').length;
      for (const method of [...new Set(methods)]) out.push({ method, path: `${prefix}${path[1]}` || '/', line });
    }
  }
  return out;
}

// "POST /api/records records create post" as { method, path }, or null when it has no method and path.
export function parseBffRoute(text) {
  const m = String(text ?? '').trim().match(/^([A-Za-z]+)\s+(\/\S*)/);
  return m ? { method: m[1].toUpperCase(), path: m[2] } : null;
}

// True when the mounted pattern (with `:name` and `*name` segments) matches the concrete path.
export function patternMatches(pattern, path) {
  const want = pattern.split('/');
  const have = path.split('/');
  for (let i = 0; i < want.length; i += 1) {
    if (want[i].startsWith('*')) return have.length > i;
    if (i >= have.length) return false;
    if (!want[i].startsWith(':') && want[i] !== have[i]) return false;
  }
  return want.length === have.length;
}

// True when a mounted route serves the route a contract names.
export function servesRoute(mounted, named) {
  return (mounted.method === 'ANY' || mounted.method === named.method) && patternMatches(mounted.path, named.path);
}

export function routeKey(route) {
  return `${route.method} ${route.path}`;
}

// The mounted routes no contract names, as "METHOD path" keys.
export function routesWithoutContract(mounted, contracts) {
  const named = contracts.map((c) => parseBffRoute(c.bffRoute)).filter(Boolean);
  return mounted.filter((route) => !named.some((n) => servesRoute(route, n))).map(routeKey);
}
