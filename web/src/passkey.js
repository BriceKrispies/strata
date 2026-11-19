// Passkeys at the browser: the one place `navigator.credentials` is called.
//
// The kernel asks for a ceremony as an effect and gets the result back as a message; this file
// is the part that touches the platform. The server speaks base64url for every byte string, and
// the browser's API takes and gives buffers, so the conversion is all this file does besides
// the call. Nothing here decides anything: whether an assertion is right is the server's.
// @ts-check

/** @param {string} text */
function toBuffer(text) {
  const padded = text.replace(/-/g, '+').replace(/_/g, '/').padEnd(Math.ceil(text.length / 4) * 4, '=');
  const binary = atob(padded);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes.buffer;
}

/** @param {ArrayBuffer | null | undefined} buffer */
function toText(buffer) {
  if (!buffer) return null;
  let binary = '';
  for (const byte of new Uint8Array(buffer)) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/** The server leaves absent options as null; the browser's API wants them absent. */
function withoutNulls(value) {
  return JSON.parse(JSON.stringify(value), (_key, member) => (member === null ? undefined : member));
}

/** @param {any[] | undefined} list */
function descriptors(list) {
  return (list ?? []).map((item) => ({ ...item, id: toBuffer(item.id) }));
}

/**
 * Whether passkeys can be offered here: the browser has them, and the site is reached by a name.
 * A passkey belongs to a host name, and WebAuthn does not allow an IP address in its place.
 */
export function passkeysSupported() {
  const host = window.location.hostname;
  const address = /^[\d.]+$/.test(host) || host.includes(':');
  return typeof window.PublicKeyCredential === 'function' && !!navigator.credentials && !address;
}

/**
 * Make a passkey for the options the server issued.
 * @param {any} options
 */
async function create(options) {
  const given = withoutNulls(options).publicKey;
  const publicKey = {
    ...given,
    challenge: toBuffer(given.challenge),
    user: { ...given.user, id: toBuffer(given.user.id) },
    excludeCredentials: descriptors(given.excludeCredentials),
    // Signing in names no one first, so the passkey has to be one the authenticator can offer
    // without being told which: a discoverable credential.
    authenticatorSelection: { ...given.authenticatorSelection, residentKey: 'required', requireResidentKey: true },
  };
  const credential = /** @type {PublicKeyCredential | null} */ (await navigator.credentials.create({ publicKey }));
  if (!credential) throw new Error('no credential');
  const response = /** @type {AuthenticatorAttestationResponse} */ (credential.response);
  return {
    id: credential.id,
    rawId: toText(credential.rawId),
    type: credential.type,
    response: {
      attestationObject: toText(response.attestationObject),
      clientDataJSON: toText(response.clientDataJSON),
      transports: typeof response.getTransports === 'function' ? response.getTransports() : undefined,
    },
    extensions: credential.getClientExtensionResults(),
  };
}

/**
 * Answer the challenge the server issued with a passkey the person picks.
 * @param {any} options
 */
async function get(options) {
  const given = withoutNulls(options).publicKey;
  // The server asks for the conditional form that fills a login field; this is a button the
  // person pressed, so the browser shows its own picker.
  const publicKey = { ...given, challenge: toBuffer(given.challenge), allowCredentials: descriptors(given.allowCredentials) };
  const credential = /** @type {PublicKeyCredential | null} */ (await navigator.credentials.get({ publicKey }));
  if (!credential) throw new Error('no credential');
  const response = /** @type {AuthenticatorAssertionResponse} */ (credential.response);
  return {
    id: credential.id,
    rawId: toText(credential.rawId),
    type: credential.type,
    response: {
      authenticatorData: toText(response.authenticatorData),
      clientDataJSON: toText(response.clientDataJSON),
      signature: toText(response.signature),
      userHandle: toText(response.userHandle),
    },
    extensions: credential.getClientExtensionResults(),
  };
}

/**
 * Carry out a ceremony the kernel asked for and say how it went.
 * @param {{ op: 'create' | 'get', id: number, options: any }} effect
 * @returns {Promise<{ type: 'passkey_result', id: number, ok: boolean, credential?: any, cancelled?: boolean }>}
 */
export async function runPasskey(effect) {
  try {
    const credential = effect.op === 'create' ? await create(effect.options) : await get(effect.options);
    return { type: 'passkey_result', id: effect.id, ok: true, credential };
  } catch (error) {
    // The person closing the browser's prompt is not a failure to report as one.
    const cancelled = error instanceof DOMException && error.name === 'NotAllowedError';
    return { type: 'passkey_result', id: effect.id, ok: false, cancelled };
  }
}
