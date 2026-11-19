// A local OpenID Connect provider for the browser tests. It serves discovery, signing keys, the
// authorisation endpoint (which approves at once, as whoever the test says) and the token endpoint,
// and signs identity tokens with a key made when it starts. It checks what a real provider checks
// of the client (client id and secret, redirect address, the proof key) so that a flow that
// leaves one out fails here as it would there.
//
//   node tests/e2e/mock-oidc.mjs [port]
//
// A test steers it through `POST /__control` with the fields of the next sign-in:
//   sub, email, email_verified, name         who signs in
//   aud, nonce, exp_offset, kid, deny        what is wrong with the token, for the refusals
//   rotate                                   publish a new signing key and sign with it
//   reset                                    forget all of the above
import { createHash, createPublicKey, createSign, generateKeyPairSync, randomBytes } from 'node:crypto';
import { createServer } from 'node:http';

const PORT = Number(process.argv[2] ?? 4290);
const ISSUER = `http://127.0.0.1:${PORT}`;
const CLIENT_ID = 'strata-e2e';
const CLIENT_SECRET = 'e2e-secret';

const b64url = (buffer) => Buffer.from(buffer).toString('base64url');

/** A signing key and the entry that publishes it. */
function makeKey(kid) {
  const { privateKey, publicKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
  const jwk = { ...createPublicKey(publicKey).export({ format: 'jwk' }), kid, alg: 'RS256', use: 'sig' };
  return { kid, privateKey, jwk };
}

let keys = [makeKey('mock-1')];
let signingKey = keys[0];

const DEFAULTS = { sub: 'mock-person-1', email: 'person@bound.example', email_verified: true, name: 'Pat Person' };
let control = { ...DEFAULTS };

/** What has been asked of this provider, for tests that look. */
const seen = { authorize: [], token: [], jwks: 0 };
const codes = new Map();

function sign(claims, kid, key = signingKey) {
  const header = { alg: 'RS256', typ: 'JWT', kid };
  const input = `${b64url(JSON.stringify(header))}.${b64url(JSON.stringify(claims))}`;
  const signature = createSign('RSA-SHA256').update(input).sign(key.privateKey);
  return `${input}.${b64url(signature)}`;
}

function readBody(request) {
  return new Promise((resolve) => {
    const chunks = [];
    request.on('data', (chunk) => chunks.push(chunk));
    request.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
  });
}

function send(response, status, body, headers = {}) {
  response.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store', ...headers });
  response.end(typeof body === 'string' ? body : JSON.stringify(body));
}

function authorize(url, response) {
  const query = url.searchParams;
  seen.authorize.push(Object.fromEntries(query));
  const redirect = query.get('redirect_uri') ?? '';
  const valid =
    query.get('client_id') === CLIENT_ID &&
    query.get('response_type') === 'code' &&
    query.get('code_challenge_method') === 'S256' &&
    (query.get('code_challenge') ?? '').length >= 43 &&
    (query.get('state') ?? '').length > 0 &&
    (query.get('nonce') ?? '').length > 0 &&
    /^https?:\/\//.test(redirect);
  if (!valid) return send(response, 400, { error: 'invalid_request' });
  const back = new URL(redirect);
  back.searchParams.set('state', query.get('state'));
  if (control.deny) {
    back.searchParams.set('error', 'access_denied');
  } else {
    const code = randomBytes(16).toString('hex');
    codes.set(code, {
      nonce: query.get('nonce'),
      challenge: query.get('code_challenge'),
      redirect,
      who: { ...control },
    });
    back.searchParams.set('code', code);
  }
  response.writeHead(302, { location: back.toString(), 'cache-control': 'no-store' });
  response.end();
}

async function token(request, response) {
  const form = new URLSearchParams(await readBody(request));
  seen.token.push(Object.fromEntries([...form].filter(([name]) => name !== 'client_secret' && name !== 'code_verifier')));
  const pending = codes.get(form.get('code') ?? '');
  codes.delete(form.get('code') ?? '');
  const verifier = form.get('code_verifier') ?? '';
  const challenge = createHash('sha256').update(verifier).digest('base64url');
  const ok =
    pending &&
    form.get('grant_type') === 'authorization_code' &&
    form.get('client_id') === CLIENT_ID &&
    form.get('client_secret') === CLIENT_SECRET &&
    form.get('redirect_uri') === pending.redirect &&
    challenge === pending.challenge;
  if (!ok) return send(response, 400, { error: 'invalid_grant' });
  const who = pending.who;
  const now = Math.floor(Date.now() / 1000);
  const claims = {
    iss: ISSUER,
    sub: who.sub,
    aud: who.aud ?? CLIENT_ID,
    exp: now + (who.exp_offset ?? 300),
    iat: now,
    nonce: who.nonce ?? pending.nonce,
    email: who.email,
    email_verified: who.email_verified,
    name: who.name,
  };
  const key = who.rotate ? keys[keys.length - 1] : signingKey;
  return send(response, 200, { token_type: 'Bearer', access_token: randomBytes(8).toString('hex'), id_token: sign(claims, who.kid ?? key.kid, key) });
}

async function controlRequest(request, response) {
  const body = JSON.parse((await readBody(request)) || '{}');
  if (body.reset) {
    control = { ...DEFAULTS };
    keys = [keys[0]];
    signingKey = keys[0];
    seen.authorize = [];
    seen.token = [];
    seen.jwks = 0;
    codes.clear();
  }
  if (body.rotate) {
    const next = makeKey(`mock-${keys.length + 1}`);
    keys = [...keys, next];
  }
  const { reset: _reset, ...fields } = body;
  control = { ...control, ...fields };
  return send(response, 200, { control, keys: keys.map((key) => key.kid) });
}

createServer(async (request, response) => {
  const url = new URL(request.url ?? '/', ISSUER);
  try {
    if (url.pathname === '/.well-known/openid-configuration') {
      return send(response, 200, {
        issuer: ISSUER,
        authorization_endpoint: `${ISSUER}/authorize`,
        token_endpoint: `${ISSUER}/token`,
        jwks_uri: `${ISSUER}/jwks`,
        response_types_supported: ['code'],
        subject_types_supported: ['public'],
        id_token_signing_alg_values_supported: ['RS256'],
        code_challenge_methods_supported: ['S256'],
      });
    }
    if (url.pathname === '/jwks') {
      seen.jwks += 1;
      return send(response, 200, { keys: keys.map((key) => key.jwk) });
    }
    if (url.pathname === '/authorize' && request.method === 'GET') return authorize(url, response);
    if (url.pathname === '/token' && request.method === 'POST') return await token(request, response);
    if (url.pathname === '/__control' && request.method === 'POST') return await controlRequest(request, response);
    if (url.pathname === '/__seen') return send(response, 200, seen);
    return send(response, 404, { error: 'not_found' });
  } catch (error) {
    return send(response, 500, { error: String(error) });
  }
}).listen(PORT, '127.0.0.1', () => {
  console.log(`mock OpenID Connect provider at ${ISSUER}`);
});
