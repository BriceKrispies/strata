// Start the whole of Strata locally: backend, BFF and (unless --no-vite) the Vite dev server.
//
//   node scripts/dev.mjs                 development, with hot module replacement on :5173
//   node scripts/dev.mjs --no-vite       the BFF serves the built frontend from web/dist
//   node scripts/dev.mjs --fresh         delete the development database first
//   node scripts/dev.mjs --no-vite --lan let other devices on the local network reach the BFF
//
// Options: --backend-port N, --bff-port N, --vite-port N, --db PATH, --store sqlite|memory,
// --no-build (use the binaries already in target/debug), --quiet (no request log on screen).
//
// A fresh service token is generated on every start and handed to both services through their
// environment; it is never written to disk. Ctrl+C stops everything.

import { spawn, spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { createWriteStream, existsSync, mkdirSync, rmSync } from 'node:fs';
import { networkInterfaces } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
const flag = (name) => args.includes(`--${name}`);
const option = (name, fallback) => {
  const at = args.indexOf(`--${name}`);
  return at >= 0 && args[at + 1] ? args[at + 1] : fallback;
};

const ports = {
  backend: Number(option('backend-port', 4101)),
  bff: Number(option('bff-port', 4100)),
  vite: Number(option('vite-port', 5173)),
};
const withVite = !flag('no-vite');
const db = resolve(root, option('db', 'var/strata.db'));
const exe = process.platform === 'win32' ? '.exe' : '';
const children = [];
let stopping = false;

mkdirSync(join(root, 'var', 'logs'), { recursive: true });

function say(text) {
  process.stdout.write(`${text}\n`);
}

function stop(code = 0) {
  if (stopping) return;
  stopping = true;
  for (const child of children) {
    if (child.exitCode !== null) continue;
    if (process.platform === 'win32') spawnSync('taskkill', ['/pid', String(child.pid), '/T', '/F'], { stdio: 'ignore' });
    else child.kill('SIGTERM');
  }
  say('\nStrata stopped.');
  process.exit(code);
}

process.on('SIGINT', () => stop(0));
process.on('SIGTERM', () => stop(0));

/** One log line for the screen: time, service, event and the fields worth a glance. */
function brief(name, line) {
  try {
    const entry = JSON.parse(line);
    if (flag('quiet') && entry.level === 'info') return null;
    if (entry.event === 'event.published' && String(entry.correlation).startsWith('install:')) return null;
    if (entry.event === 'kernel.accepted' && String(entry.correlation).startsWith('install:')) return null;
    const { ts, level, service, event, correlation, ...rest } = entry;
    const fields = Object.entries(rest).map(([k, v]) => `${k}=${typeof v === 'object' ? JSON.stringify(v) : v}`).join(' ');
    return `${new Date(ts).toISOString().slice(11, 23)} ${String(service ?? name).padEnd(7)} ${level === 'info' ? '' : `${level.toUpperCase()} `}${event}${correlation ? ` [${correlation}]` : ''} ${fields}`;
  } catch {
    return `${name.padEnd(8)}${line}`;
  }
}

function start(name, command, commandArgs, env) {
  const child = spawn(command, commandArgs, { cwd: root, env: { ...process.env, ...env }, stdio: ['ignore', 'pipe', 'pipe'] });
  const file = createWriteStream(join(root, 'var', 'logs', `${name}.jsonl`), { flags: 'a' });
  const pipe = (stream) => {
    let buffered = '';
    stream.on('data', (chunk) => {
      buffered += chunk.toString();
      const lines = buffered.split(/\r?\n/);
      buffered = lines.pop() ?? '';
      for (const line of lines) {
        if (!line.trim()) continue;
        file.write(`${line}\n`);
        const shown = brief(name, line);
        if (shown) say(shown);
      }
    });
  };
  pipe(child.stdout);
  pipe(child.stderr);
  child.on('exit', (code) => {
    if (!stopping) {
      say(`${name} exited with code ${code}. Stopping the rest.`);
      stop(1);
    }
  });
  children.push(child);
  return child;
}

async function waitFor(url, what) {
  for (let attempt = 0; attempt < 240; attempt += 1) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
    } catch {
      // Not listening yet.
    }
    await new Promise((done) => setTimeout(done, 250));
  }
  say(`${what} did not come up at ${url}.`);
  stop(1);
}

if (!flag('no-build')) {
  say('Building the backend and the BFF…');
  const built = spawnSync('cargo', ['build', '-p', 'strata-backend', '-p', 'strata-bff'], { cwd: root, stdio: 'inherit' });
  if (built.status !== 0) process.exit(built.status ?? 1);
}
if (!withVite && !existsSync(join(root, 'web', 'dist', 'index.html'))) {
  say('Building the frontend…');
  const built = spawnSync(process.execPath, [join(root, 'node_modules', 'vite', 'bin', 'vite.js'), 'build', 'web'], { cwd: root, stdio: 'inherit' });
  if (built.status !== 0) process.exit(built.status ?? 1);
}
if (flag('fresh')) {
  for (const suffix of ['', '-wal', '-shm']) rmSync(`${db}${suffix}`, { force: true });
}

const token = randomBytes(32).toString('hex');
const hosts = ['127.0.0.1', 'localhost', 'tidewater.localhost'];
// --lan: the BFF (never the backend) listens on every interface, and this machine's local
// network addresses are added as Fieldnotes sites and allowed origins.
const lanAddresses = flag('lan')
  ? Object.values(networkInterfaces()).flat().filter((a) => a && a.family === 'IPv4' && !a.internal).map((a) => a.address)
  : [];
hosts.push(...lanAddresses);
const sites = ['127.0.0.1=fieldnotes', 'localhost=fieldnotes', 'tidewater.localhost=tidewater', ...lanAddresses.map((a) => `${a}=fieldnotes`)].join(',');
const origins = hosts.flatMap((host) => [`http://${host}:${ports.bff}`, `http://${host}:${ports.vite}`]).join(',');

start('backend', join(root, 'target', 'debug', `strata-backend${exe}`), [], {
  STRATA_BACKEND_ADDR: `127.0.0.1:${ports.backend}`,
  STRATA_SERVICE_TOKEN: token,
  STRATA_STORE: option('store', 'sqlite'),
  STRATA_DB: db,
  STRATA_TENANTS: join(root, 'tenants'),
  STRATA_LEAN_DIR: join(root, 'lean'),
  STRATA_DEV: '1',
});
await waitFor(`http://127.0.0.1:${ports.backend}/internal/health`, 'The backend');

start('bff', join(root, 'target', 'debug', `strata-bff${exe}`), [], {
  STRATA_BFF_ADDR: `${flag('lan') ? '0.0.0.0' : '127.0.0.1'}:${ports.bff}`,
  STRATA_SITES: sites,
  STRATA_BACKEND_URL: `http://127.0.0.1:${ports.backend}`,
  STRATA_SERVICE_TOKEN: token,
  STRATA_ORIGINS: origins,
  STRATA_WEB_DIST: join(root, 'web', 'dist'),
  STRATA_DEV: '1',
});
await waitFor(`http://127.0.0.1:${ports.bff}/api/session`, 'The BFF');

if (withVite) {
  start('vite', process.execPath, [join(root, 'node_modules', 'vite', 'bin', 'vite.js'), 'web', '--port', String(ports.vite)], {
    STRATA_BFF_URL: `http://127.0.0.1:${ports.bff}`,
    STRATA_VITE_PORT: String(ports.vite),
  });
  await waitFor(`http://127.0.0.1:${ports.vite}/`, 'Vite');
}

const front = withVite ? ports.vite : ports.bff;
say(`
Strata is running.

  Fieldnotes          http://127.0.0.1:${front}/
  Writing desk        http://127.0.0.1:${front}/desk
  Impact Lab          http://127.0.0.1:${front}/lab            (sign in as admin)
  Second tenant       http://tidewater.localhost:${front}/
  ${withVite ? `BFF (built assets)  http://127.0.0.1:${ports.bff}/\n  ` : ''}Backend             127.0.0.1:${ports.backend}  (loopback only, service token required)

  Development logins, defined in tenants/fieldnotes/accounts.dev.json:
    admin  / strata-dev-admin     owner: settings, schema, Impact Lab
    ines   / strata-dev-ines      editor
    tomasz / strata-dev-tomasz    author
    mirela / strata-dev-mirela    author

  Database  ${option('store', 'sqlite') === 'memory' ? 'in memory' : db}
  Logs      var/logs/*.jsonl
${lanAddresses.length ? `
  From another device on this network:
${lanAddresses.map((a) => `    http://${a}:${ports.bff}/`).join('\n')}
  The development logins above work from there too, so use this only on a network you trust.
` : ''}
Press Ctrl+C to stop.
`);
