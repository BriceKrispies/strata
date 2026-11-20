// Which addresses are the page's own. Shared by the main thread's click handler and the tests.
// @ts-check

import { matchRoute, route } from './generated/routes.js';

/**
 * True when the specification's routes name this address (the catch-all, which answers every
 * other path, does not count). Anything else (/keys, /scim, /api, a file) is a resource of the
 * server that the browser fetches itself, not a page to open in place.
 * @param {string} pathname
 * @param {string} [search]
 */
export function isPageAddress(pathname, search = '') {
  const known = matchRoute(pathname, search);
  return known !== null && route(known.screen)?.catchAll !== true;
}
