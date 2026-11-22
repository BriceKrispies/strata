// The one Trusted Types policy the page creates, for the address of the kernel worker.
//
// The page's content security policy says `require-trusted-types-for 'script'` and
// `trusted-types strata-worker`: a worker's script address is a script sink, so a plain string
// cannot reach `new Worker`. This policy is the only way a string becomes a script address, and
// it accepts only an address on the page's own origin under `/assets/` (the built files), or
// under `/src/` while the development server runs. No `default` policy is created.
// @ts-check

export const WORKER_POLICY_NAME = 'strata-worker';

/**
 * The address as an absolute same-origin URL, or null when it is any other address.
 *
 * @param {string} value
 * @param {string} origin The page's origin.
 * @param {boolean} dev True while the development server serves the page.
 * @returns {string | null}
 */
export function acceptWorkerUrl(value, origin, dev) {
  let url;
  try {
    url = new URL(String(value), origin);
  } catch {
    return null;
  }
  if (url.origin !== origin || url.username !== '' || url.password !== '') return null;
  const prefixes = dev ? ['/assets/', '/src/'] : ['/assets/'];
  if (!prefixes.some((prefix) => url.pathname.startsWith(prefix))) return null;
  // A dot segment is already resolved by the parser; an encoded one is not a path step, but is refused all the same.
  if (/%2e|%2f|%5c/i.test(url.pathname) || url.pathname.includes(String.fromCharCode(92))) return null;
  return url.href;
}

let policy = null;

/**
 * Makes the policy from the browser's Trusted Types factory.
 *
 * @param {any} factory `window.trustedTypes`
 * @param {string} origin
 * @param {boolean} dev
 */
export function createWorkerPolicy(factory, origin, dev) {
  return factory.createPolicy(WORKER_POLICY_NAME, {
    createScriptURL(value) {
      const accepted = acceptWorkerUrl(value, origin, dev);
      if (accepted === null) throw new TypeError(`The address is not a script of this page: ${String(value)}`);
      return accepted;
    },
  });
}

/**
 * The worker's address as the type `new Worker` takes under the policy. A browser without
 * Trusted Types gets the checked string.
 *
 * @param {string} value
 * @returns {any}
 */
export function workerScriptUrl(value) {
  const dev = import.meta.env?.DEV === true;
  const factory = /** @type {any} */ (globalThis).trustedTypes;
  if (!factory) {
    const accepted = acceptWorkerUrl(value, globalThis.location.origin, dev);
    if (accepted === null) throw new TypeError(`The address is not a script of this page: ${String(value)}`);
    return accepted;
  }
  policy ??= createWorkerPolicy(factory, globalThis.location.origin, dev);
  return policy.createScriptURL(value);
}
