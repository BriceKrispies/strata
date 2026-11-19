// A local SAML identity provider for the browser tests. It reads the authentication request the
// way a provider does (HTTP-Redirect binding: base64, raw DEFLATE, XML), checks what a real
// provider checks of the service (the address the response goes to, the destination, who is
// asking), makes a response for it, signs the assertion with a test key, and answers the browser
// with the page every provider has: a form that posts the response to the service's assertion
// consumer address at once.
//
//   node tests/e2e/mock-saml.mjs [port]
//
// A test steers it through `POST /__control` with the fields of the next sign-in:
//   sub, email, name                 who signs in
//   audience, recipient, issuer      what is wrong with the assertion, for the refusals
//   exp_offset                       seconds from now to the end of the assertion (negative: expired)
//   unsigned, signer, tamper         no signature, a signature by another key, a name changed after
//                                    the signature was made
//   deny                             a response that reports a failure
//   assertion_id                     the identifier to use, for the replay of one
//   reset                            forget all of the above
//
// The assertion is written in exclusive canonical form (attributes in order, no empty-element tags,
// no white space between elements), so that the digest and the signature are made over exactly the
// bytes a verifier canonicalises it to. The test key and certificate are tests/e2e/saml/idp-*.pem,
// which sign nothing outside the tests.
import { createHash, createSign, randomBytes } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { inflateRawSync } from 'node:zlib';

const PORT = Number(process.argv[2] ?? 4291);
const IDP = `http://127.0.0.1:${PORT}`;
const IDP_ENTITY = `${IDP}/metadata`;
const SSO = `${IDP}/sso`;
// The service the browser tests run against, and what it calls itself for the `mock` provider.
const SITE = 'http://127.0.0.1:4200';
const ACS = `${SITE}/federation/saml/acs`;
const SP_ENTITY = `${SITE}/federation/saml/metadata/mock`;

const SAML = 'urn:oasis:names:tc:SAML:2.0:assertion';
const SAMLP = 'urn:oasis:names:tc:SAML:2.0:protocol';
const DS = 'http://www.w3.org/2000/09/xmldsig#';
const SUCCESS = 'urn:oasis:names:tc:SAML:2.0:status:Success';

const key = (name) => readFileSync(new URL(`./saml/${name}-key.pem`, import.meta.url), 'utf8');
const KEYS = { idp: key('idp'), other: key('other') };

const DEFAULTS = { sub: 'mock-saml-subject-1', email: 'person@saml.example', name: 'Sam Saml' };
let control = { ...DEFAULTS };

/** What has been asked of this provider, for tests that look. */
const seen = { sso: [] };

const iso = (ms) => new Date(ms).toISOString().replace(/\.\d{3}Z$/, 'Z');
// Canonical XML escapes text and attribute values differently.
const text = (value) => String(value).replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;');
const attr = (value) => String(value).replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('"', '&quot;');
const sha256 = (data) => createHash('sha256').update(data, 'utf8').digest('base64');

function readRequest(url) {
  const message = url.searchParams.get('SAMLRequest') ?? '';
  const xml = inflateRawSync(Buffer.from(message, 'base64')).toString('utf8');
  const find = (name) => xml.match(new RegExp(`\\s${name}="([^"]*)"`))?.[1]?.replaceAll('&amp;', '&');
  return {
    xml,
    id: find('ID'),
    destination: find('Destination'),
    acs: find('AssertionConsumerServiceURL'),
    binding: find('ProtocolBinding'),
    issuer: xml.match(/<saml:Issuer>([^<]*)<\/saml:Issuer>/)?.[1],
    relay: url.searchParams.get('RelayState') ?? '',
  };
}

/** The signature element for the assertion, over its canonical text. */
function signature(canonical, id, signer) {
  const signedInfo =
    `<ds:SignedInfo xmlns:ds="${DS}">` +
    `<ds:CanonicalizationMethod Algorithm="http://www.w3.org/2001/10/xml-exc-c14n#"></ds:CanonicalizationMethod>` +
    `<ds:SignatureMethod Algorithm="http://www.w3.org/2001/04/xmldsig-more#rsa-sha256"></ds:SignatureMethod>` +
    `<ds:Reference URI="#${id}"><ds:Transforms>` +
    `<ds:Transform Algorithm="http://www.w3.org/2000/09/xmldsig#enveloped-signature"></ds:Transform>` +
    `<ds:Transform Algorithm="http://www.w3.org/2001/10/xml-exc-c14n#"></ds:Transform></ds:Transforms>` +
    `<ds:DigestMethod Algorithm="http://www.w3.org/2001/04/xmlenc#sha256"></ds:DigestMethod>` +
    `<ds:DigestValue>${sha256(canonical)}</ds:DigestValue></ds:Reference></ds:SignedInfo>`;
  const value = createSign('RSA-SHA256').update(signedInfo, 'utf8').sign(KEYS[signer], 'base64');
  // In the message the signed info inherits the prefix from the signature; canonicalised in its
  // place it gains the declaration, which is the text that was signed.
  const inherited = signedInfo.replace(` xmlns:ds="${DS}"`, '');
  return `<ds:Signature xmlns:ds="${DS}">${inherited}<ds:SignatureValue>${value}</ds:SignatureValue></ds:Signature>`;
}

function buildResponse(request, who) {
  const now = Date.now();
  const issued = iso(now);
  const end = iso(now + (who.exp_offset ?? 300) * 1000);
  const id = who.assertion_id ?? `_a${randomBytes(12).toString('hex')}`;
  const issuer = `<saml:Issuer>${text(who.issuer ?? IDP_ENTITY)}</saml:Issuer>`;
  const head = `<saml:Assertion xmlns:saml="${SAML}" ID="${id}" IssueInstant="${issued}" Version="2.0">`;
  const body =
    `<saml:Subject><saml:NameID Format="urn:oasis:names:tc:SAML:2.0:nameid-format:persistent">${text(who.sub)}</saml:NameID>` +
    `<saml:SubjectConfirmation Method="urn:oasis:names:tc:SAML:2.0:cm:bearer">` +
    `<saml:SubjectConfirmationData InResponseTo="${attr(request.id)}" NotOnOrAfter="${end}" Recipient="${attr(who.recipient ?? ACS)}"></saml:SubjectConfirmationData>` +
    `</saml:SubjectConfirmation></saml:Subject>` +
    `<saml:Conditions NotBefore="${iso(now - 5000)}" NotOnOrAfter="${end}"><saml:AudienceRestriction>` +
    `<saml:Audience>${text(who.audience ?? SP_ENTITY)}</saml:Audience></saml:AudienceRestriction></saml:Conditions>` +
    `<saml:AuthnStatement AuthnInstant="${issued}"><saml:AuthnContext><saml:AuthnContextClassRef>` +
    `urn:oasis:names:tc:SAML:2.0:ac:classes:PasswordProtectedTransport</saml:AuthnContextClassRef></saml:AuthnContext></saml:AuthnStatement>` +
    `<saml:AttributeStatement>` +
    `<saml:Attribute Name="email"><saml:AttributeValue>${text(who.email)}</saml:AttributeValue></saml:Attribute>` +
    `<saml:Attribute Name="name"><saml:AttributeValue>${text(who.name)}</saml:AttributeValue></saml:Attribute>` +
    `</saml:AttributeStatement>`;
  const tail = '</saml:Assertion>';
  const canonical = head + issuer + body + tail;
  let assertion = who.unsigned
    ? canonical
    : head + issuer + signature(canonical, id, who.signer ?? 'idp') + body + tail;
  if (who.tamper) assertion = assertion.replace(`>${text(who.sub)}<`, '>someone-else<');
  const status = who.deny ? 'urn:oasis:names:tc:SAML:2.0:status:Responder' : SUCCESS;
  return (
    `<samlp:Response xmlns:samlp="${SAMLP}" xmlns:saml="${SAML}" Destination="${attr(request.acs)}" ` +
    `ID="_r${randomBytes(12).toString('hex')}" InResponseTo="${attr(request.id)}" IssueInstant="${issued}" Version="2.0">` +
    `<saml:Issuer>${text(IDP_ENTITY)}</saml:Issuer><samlp:Status><samlp:StatusCode Value="${status}"></samlp:StatusCode></samlp:Status>` +
    `${assertion}</samlp:Response>`
  );
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

/** The provider's own page: the form that posts the response to the service at once. */
function sso(url, response) {
  const request = readRequest(url);
  seen.sso.push({ id: request.id, issuer: request.issuer, destination: request.destination, acs: request.acs, binding: request.binding });
  const valid =
    request.id?.startsWith('_') &&
    request.issuer === SP_ENTITY &&
    request.destination === SSO &&
    request.acs === ACS &&
    request.binding === 'urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST' &&
    /^[0-9a-f]{64}$/.test(request.relay);
  if (!valid) return send(response, 400, { error: 'invalid_request', request });
  const message = Buffer.from(buildResponse(request, control), 'utf8').toString('base64');
  const page =
    `<!doctype html><html><body onload="document.forms[0].submit()"><form method="post" action="${attr(request.acs)}">` +
    `<input type="hidden" name="SAMLResponse" value="${message}"><input type="hidden" name="RelayState" value="${attr(request.relay)}">` +
    `<noscript><button>Continue</button></noscript></form></body></html>`;
  response.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
  response.end(page);
}

async function controlRequest(request, response) {
  const body = JSON.parse((await readBody(request)) || '{}');
  if (body.reset) {
    control = { ...DEFAULTS };
    seen.sso = [];
  }
  const { reset: _reset, ...fields } = body;
  control = { ...control, ...fields };
  return send(response, 200, { control });
}

createServer(async (request, response) => {
  const url = new URL(request.url ?? '/', IDP);
  try {
    if (url.pathname === '/metadata') {
      // Enough of a provider's metadata to be found, for the test runner's readiness check.
      response.writeHead(200, { 'content-type': 'application/samlmetadata+xml' });
      return response.end(
        `<?xml version="1.0"?><md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata" entityID="${IDP_ENTITY}">` +
          `<md:IDPSSODescriptor protocolSupportEnumeration="${SAMLP}"><md:SingleSignOnService ` +
          `Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect" Location="${SSO}"/></md:IDPSSODescriptor></md:EntityDescriptor>`,
      );
    }
    if (url.pathname === '/sso' && request.method === 'GET') return sso(url, response);
    if (url.pathname === '/__control' && request.method === 'POST') return await controlRequest(request, response);
    if (url.pathname === '/__seen') return send(response, 200, seen);
    return send(response, 404, { error: 'not_found' });
  } catch (error) {
    return send(response, 500, { error: String(error) });
  }
}).listen(PORT, '127.0.0.1', () => {
  console.log(`mock SAML provider at ${IDP}`);
});
