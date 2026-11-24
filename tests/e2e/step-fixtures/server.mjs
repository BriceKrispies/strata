// The pages and endpoints the step fixtures run against (tests/e2e/step-fixtures/spec.jsonc).
// Plain Node, no dependencies:
//
//   node tests/e2e/step-fixtures/server.mjs [port]     (default 4300)
//
// Pages are drawn at /p/<name>, /other and /live, each with the application's page marker
// (main[data-page]) except /p/noready. Any other address answers 404 with a drawn page, as the
// application's catch-all screen does. The endpoints stand in for a stack's: /api/view (the
// view fetch), /api/session and /api/dev/mail-outbox (the development mail outbox), /api/notify (sends a live notice to every open WebSocket on /ws), /api/act
// (writes a backend and a BFF log line under var/step-fixtures, with variants that break one
// property each), /api/json, /api/echo and /api/login (cookies).
import { createHash } from 'node:crypto';
import { appendFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { fileURLToPath } from 'node:url';

const port = Number(process.argv[2] ?? 4300);
const logs = fileURLToPath(new URL('../../../var/step-fixtures/', import.meta.url));
mkdirSync(logs, { recursive: true });
// Each run starts with empty logs, so an absence check cannot match a line an earlier run wrote.
for (const name of ['backend.jsonl', 'bff.jsonl']) writeFileSync(`${logs}${name}`, '');

function page(name, title, body, script = '', { ready = true, html = '' } = {}) {
  return `<!doctype html>
<html lang="en" data-mode="calm"${html}>
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>${title}</title></head>
<body>
<main${ready ? ` data-page="${name}"` : ''}>
${body}
</main>
${script ? `<script>${script}</script>` : ''}
</body>
</html>
`;
}

// A page whose marker is set by its script once it has done what the step reads.
const later = (name, title, body, script) => page(name, title, body, script).replace(` data-page="${name}"`, ' data-page="loading"');

const PAGES = {
  basic: page('basic', 'Basic', `
<h1>Basic page</h1>
<nav aria-label="Links"><a href="/other">Other</a></nav>
<p>Hello there</p>
<p hidden>Secret note</p>
<section aria-label="Snapshot"><p>Revision 7 of the notes</p></section>
<article><h2>One</h2></article>
<article><h2>Two</h2></article>
<article><h2>Three</h2></article>
<label for="name">Name</label> <input id="name">
<button type="button">Stay</button>
<button type="button" disabled>Disabled</button>
<input type="checkbox" id="agree" checked> <label for="agree">Agree</label>
<input type="checkbox" id="spam"> <label for="spam">Spam</label>
<section aria-label="Hidden panel" hidden><p>Not shown</p></section>`),
  noready: page('noready', 'Never drawn', '<h1>Never drawn</h1>', '', { ready: false }),
  form: page('form', 'Form', `
<h1>Form</h1>
<form onsubmit="return false">
<label for="name">Name</label> <input id="name">
<input type="checkbox" id="remember"> <label for="remember">Remember me</label>
<label for="topic">Topic</label> <select id="topic"><option value="d">Design</option><option value="c">Craft</option></select>
<div role="radiogroup" aria-label="Shade">
<input type="radio" name="shade" id="dk" value="dk"> <label for="dk">Dark</label>
<input type="radio" name="shade" id="lt" value="lt"> <label for="lt">Light</label>
</div>
<div role="radiogroup" aria-label="Accent">
<input type="radio" name="accent" id="plum" value="plum"> <label for="plum">Plum</label>
<input type="radio" name="accent" id="moss" value="moss"> <label for="moss">Moss</label>
</div>
<label for="count">Count</label> <input id="count" type="number">
<button type="button" id="go">Sign in</button>
</form>
<p id="out"></p>`, `document.getElementById('go').onclick = () => {
  document.getElementById('out').textContent = 'Submitted ' + document.getElementById('name').value;
};`),
  desk: page('desk', 'Desk', `
<h1>Desk</h1>
<table><tr><td>Kiln</td><td><button type="button" id="delete">Delete Kiln</button></td></tr></table>
<button type="button" id="confirm" hidden>Confirm deleting Kiln</button>
<section aria-label="Messages"><p id="msg"></p></section>
<button type="button" id="out">Sign out</button>`, `const msg = document.getElementById('msg');
const confirmButton = document.getElementById('confirm');
document.getElementById('delete').onclick = () => { confirmButton.hidden = false; };
confirmButton.onclick = () => { msg.textContent = 'Deleted.'; };
document.getElementById('out').onclick = () => { msg.textContent = 'Signed out.'; };`),
  fragment: page('fragment', 'Fragment', '<h1>Fragment</h1><p id="h"></p>', `const show = () => { document.getElementById('h').textContent = 'Fragment ' + location.hash; }; show(); addEventListener('hashchange', show);`),
  cleaned: page('cleaned', 'Cleaned', '<h1>Cleaned</h1><p id="h"></p>', `const held = location.hash.slice(1);
history.replaceState(null, '', location.pathname);
document.getElementById('h').textContent = 'Cleaned ' + held;`),
  cookies: page('cookies', 'Cookies', '<h1>Cookies</h1><p id="c"></p>', `document.getElementById('c').textContent = document.cookie;`),
  scheme: page('scheme', 'Scheme', '<h1>Scheme</h1>', `document.documentElement.dataset.scheme = matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
document.documentElement.dataset.touch = navigator.maxTouchPoints > 0 ? 'yes' : 'no';`),
  wide: page('wide', 'Wide', '<h1>Wide</h1><div style="width: 2000px">A line far wider than any screen.</div>'),
  responsive: page('responsive', 'Responsive', '<h1>Responsive</h1><div style="width: 500px; height: 20px; background: #ddd">A box 500 pixels wide.</div>'),
  side: page('side', 'Side', `
<h1>Side</h1>
<div style="display: flex; gap: 20px">
<aside aria-label="Notes" style="width: 200px">Notes in the margin.</aside>
<section aria-label="Text" style="flex: 1">The text itself.</section>
</div>`),
  online: page('online', 'Online', '<h1>Online</h1>', `const set = () => { document.documentElement.dataset.online = String(navigator.onLine); };
addEventListener('online', set);
addEventListener('offline', set);
set();`),
  inaccessible: page('inaccessible', 'Inaccessible', '<h1>Inaccessible</h1><img src="/missing.png"><input>'),
  worker: later('worker', 'Worker', '<h1>Worker</h1>', `const worker = new Worker('/worker.js');
worker.onmessage = () => { document.querySelector('main').dataset.page = 'worker'; };`),
  mainfetch: later('mainfetch', 'Main thread', '<h1>Main thread</h1>', `fetch('/api/view?path=' + encodeURIComponent(location.pathname)).then(() => { document.querySelector('main').dataset.page = 'mainfetch'; });`),
  act: page('act', 'Act', `
<h1>Act</h1>
<button type="button" data-variant="">Act</button>
<button type="button" data-variant="unlinked">Act unlinked</button>
<button type="button" data-variant="nobff">Act alone</button>
<button type="button" data-variant="extra">Act extra</button>
<button type="button" data-variant="quietly">Act quietly</button>
<button type="button" data-variant="leak">Act leaking</button>
<p id="out"></p>`, `for (const button of document.querySelectorAll('button[data-variant]')) {
  button.onclick = async () => {
    const correlation = 'c-' + Math.random().toString(16).slice(2, 10);
    await fetch('/api/act?' + button.dataset.variant, { method: 'POST', headers: { 'x-correlation-id': correlation, 'content-type': 'application/json' }, body: '{}' });
    document.getElementById('out').textContent = 'Done: ' + button.textContent;
  };
}`),
};

const OTHER = page('other', 'Other', '<h1>Other page</h1>');
const LOST = page('lost', 'Nothing here', '<h1>There is nothing at this address</h1>');
const LIVE = later('live', 'Live', '<h1>Live</h1><a href="/other">Elsewhere</a><p id="state">connecting</p>', `const refresh = !new URLSearchParams(location.search).has('norefresh');
const view = () => fetch('/api/view?path=' + encodeURIComponent(location.pathname)).then((r) => r.text());
const socket = new WebSocket('ws://' + location.host + '/ws');
socket.onmessage = (message) => {
  if (JSON.parse(message.data).type === 'event' && refresh) view();
};
Promise.all([view(), new Promise((resolve) => { socket.onopen = resolve; })]).then(() => {
  document.getElementById('state').textContent = 'open';
  document.querySelector('main').dataset.page = 'live';
});`);
const WORKER = `fetch('/api/view?path=/p/worker').then(() => {
  const socket = new WebSocket('ws://' + self.location.host + '/ws');
  socket.onopen = () => self.postMessage('ready');
});
`;

// The development mail outbox, oldest message first, as the application's answers it to a signed-in
// owner: POST /api/dev/mail-outbox with an allowed origin and the request token of /api/session.
const MAIL_CSRF = 'tok-mail';
const MAIL = [
  { to: 'ines@fieldnotes.example', subject: 'Your sign-in link', text: 'Open https://site.example/login#c=0a0a&t=0b0b to sign in.' },
  { to: 'tomasz@fieldnotes.example', subject: 'Your sign-in link', text: 'Open https://site.example/login#c=ffff&t=eeee to sign in.' },
  { to: 'Ines@Fieldnotes.example', subject: 'Your sign-in link', text: 'Open https://site.example/login#c=ab12&t=cd34 to sign in.' },
];

// The WebSockets of /ws, each sent a notice by /api/notify.
const sockets = new Set();

function frame(text) {
  const payload = Buffer.from(text);
  return Buffer.concat([Buffer.from([0x81, payload.length]), payload]);
}

let acts = 0;
function act(variant, correlation) {
  acts += 1;
  const ts = Date.now();
  const id = variant === 'unlinked' ? `srv-${acts}` : correlation;
  if (variant !== 'nobff') appendFileSync(`${logs}bff.jsonl`, `${JSON.stringify({ ts, level: 'info', service: 'bff', event: 'http.request', correlation: id, path: '/api/act' })}\n`);
  const line = { ts, level: variant === 'quietly' ? 'debug' : 'info', service: 'backend', event: 'thing.done', correlation: id, tenant: 't1', object: 'post' };
  if (variant === 'extra') line.body = 'the whole payload';
  if (variant === 'leak') line.object = 'leak-abc123';
  appendFileSync(`${logs}backend.jsonl`, `${JSON.stringify(line)}\n`);
}

function send(res, status, type, body, headers = {}) {
  res.writeHead(status, { 'content-type': type, 'cache-control': 'no-store', ...headers });
  res.end(body);
}

const json = (res, status, value, headers) => send(res, status, 'application/json', JSON.stringify(value), headers);

const server = createServer((req, res) => {
  const url = new URL(req.url, `http://${req.headers.host}`);
  const chunks = [];
  req.on('data', (c) => chunks.push(c));
  req.on('end', () => {
    const text = Buffer.concat(chunks).toString('utf8');
    const html = (status, body) => send(res, status, 'text/html; charset=utf-8', body);
    const p = url.pathname;
    if (p === '/api/view') return json(res, 200, { view: url.searchParams.get('path') });
    if (p === '/api/notify') {
      for (const s of sockets) s.write(frame(JSON.stringify({ type: 'event', seq: 1, notice: { kind: 'upsert' } })));
      return json(res, 200, { notified: sockets.size });
    }
    if (p === '/api/act') {
      act([...url.searchParams.keys()][0] ?? '', String(req.headers['x-correlation-id'] ?? ''));
      return json(res, 200, { ok: true });
    }
    if (p === '/api/session') return json(res, 200, { session: { authenticated: true, csrf_token: MAIL_CSRF } });
    if (p === '/api/dev/mail-outbox') {
      if (req.method !== 'POST' || !req.headers.origin || req.headers['x-csrf-token'] !== MAIL_CSRF) return json(res, 403, { code: 'forbidden' });
      return json(res, 200, { messages: MAIL });
    }
    if (p === '/api/json') return json(res, 200, { session: { csrf_token: 'tok-123' }, list: [{ name: 'a' }] });
    if (p === '/api/echo') {
      let body = null;
      try {
        body = text ? JSON.parse(text) : null;
      } catch {
        body = text;
      }
      return json(res, 200, { body, headers: { 'x-token': req.headers['x-token'] ?? null } });
    }
    if (p === '/api/login') {
      return json(res, 200, { ok: true }, {
        'x-token': 'abc',
        'set-cookie': ['__Host-sid=sid-123; Path=/; Secure; HttpOnly; SameSite=Strict', 'plain=1; Path=/'],
      });
    }
    if (p === '/worker.js') return send(res, 200, 'text/javascript', WORKER);
    if (p === '/other') return html(200, OTHER);
    if (p === '/live') return html(200, LIVE);
    const named = /^\/p\/([a-z]+)$/.exec(p);
    if (named && Object.hasOwn(PAGES, named[1])) return html(200, PAGES[named[1]]);
    return html(404, LOST);
  });
});

server.on('upgrade', (req, socket) => {
  if (new URL(req.url, 'http://x').pathname !== '/ws') return socket.destroy();
  const accept = createHash('sha1').update(`${req.headers['sec-websocket-key']}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`).digest('base64');
  socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
  sockets.add(socket);
  // Frames from the page are not read; a closed or failed connection is forgotten.
  socket.on('data', () => {});
  socket.on('close', () => sockets.delete(socket));
  socket.on('error', () => sockets.delete(socket));
});

server.listen(port, '127.0.0.1', () => console.log(`step fixtures on http://127.0.0.1:${port}`));
