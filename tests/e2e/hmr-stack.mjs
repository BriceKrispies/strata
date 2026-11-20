// The development stack of the hot-reload test (tests/e2e/hmr.spec.js), served from a throwaway
// copy of the frontend. The test edits files while the browser watches; it edits the copy in
// var/hmr-web, never the tracked files under web/, so an interrupted run leaves the tree clean.
//
//   node tests/e2e/hmr-stack.mjs <the arguments of scripts/dev.mjs>
import { spawn } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const copy = join(root, 'var', 'hmr-web');

rmSync(copy, { recursive: true, force: true });
mkdirSync(copy, { recursive: true });
for (const name of ['src', 'public', 'index.html', 'vite.config.js']) {
  const from = join(root, 'web', name);
  if (existsSync(from)) cpSync(from, join(copy, name), { recursive: true });
}

const child = spawn(process.execPath, [join(root, 'scripts', 'dev.mjs'), ...process.argv.slice(2), '--web-dir', join('var', 'hmr-web')], { cwd: root, stdio: 'inherit' });
for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => child.kill(signal));
child.on('exit', (code) => process.exit(code ?? 1));
