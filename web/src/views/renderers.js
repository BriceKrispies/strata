// The renderers of the pages, by kind. A screen's `kind` in the specification names its renderer
// (`pages.home`, `desk.editor`: the part after the dot is the page kind the view model carries);
// the page frame (shell.js) looks the page's kind up here and draws what it finds, so a product
// draws a new screen by registering a renderer, without editing the frame.
// @ts-check

import * as pages from './pages.js';
import * as desk from './desk.js';
import { lab } from './lab.js';
import * as tenant from './tenant.js';

/**
 * @typedef {(page: any, shell: any) => any} Render
 * @typedef {{ render: Render, wide: boolean }} Renderer
 */

/** @type {Map<string, Renderer>} */
const RENDERERS = new Map();

/**
 * Register the renderer of a page kind, replacing one already registered. `wide` asks the frame
 * for the full-width main region.
 * @param {string} kind
 * @param {Render} render
 * @param {{ wide?: boolean }} [options]
 */
export function registerRenderer(kind, render, options = {}) {
  RENDERERS.set(kind, { render, wide: options.wide === true });
}

/**
 * The renderer of a page kind; null when none is registered.
 * @param {string} kind
 * @returns {Renderer | null}
 */
export function rendererOf(kind) {
  return RENDERERS.get(kind) ?? null;
}

/** The kinds that have a renderer, sorted: for a test that every screen's kind has one. */
export function registeredKinds() {
  return [...RENDERERS.keys()].sort();
}

// The publication's pages.
registerRenderer('home', (page, shell) => pages.home(page, shell));
registerRenderer('post', (page) => pages.post(page));
registerRenderer('topics', (page) => pages.topics(page));
registerRenderer('topic', (page) => pages.topic(page));
registerRenderer('authors', (page) => pages.authors(page));
registerRenderer('author', (page) => pages.author(page));
registerRenderer('search', (page) => pages.search(page));
registerRenderer('about', (page) => pages.about(page));
registerRenderer('login', (page) => pages.login(page));
registerRenderer('desk', (page) => desk.desk(page));
registerRenderer('editor', (page) => desk.editor(page));
registerRenderer('settings', (page) => desk.settings(page));
registerRenderer('lab', (page) => lab(page), { wide: true });
// Every screen of the tenant's interface definition, drawn by the one interpreter (D-8250).
registerRenderer('tenant', (page) => tenant.screen(page));
// The states the frame shows itself, which no screen has.
registerRenderer('signin_required', (page) => pages.signinRequired(page));
registerRenderer('forbidden', () => pages.forbidden());
registerRenderer('error', (page) => pages.failure(page));
registerRenderer('loading', () => pages.loading());
registerRenderer('missing', (page) => pages.missing(page));
