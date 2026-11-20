// The builders of the pages, by kind. A screen's `kind` in the specification names its renderer
// (`pages.home`, `desk.editor`: the part after the dot is the page kind, which the generated
// `SCREENS` carry); the view derivation (view.js) finds the screen of the model's view, looks the
// screen's kind up here and builds the page model the renderer of that kind draws. A product
// builds a new screen's page by registering a builder, without editing view.js, as it draws it by
// registering a renderer (views/renderers.js). A view whose screen has no builder is the missing
// page.
// @ts-check

/**
 * What a builder is given besides the kernel state: the view model, its data, the session and what
 * the session may do.
 * @typedef {{ model: any, d: any, session: any, signedIn: boolean, canWrite: boolean }} PageContext
 * @typedef {(state: any, context: PageContext) => any} BuildPage
 */

/** @type {Map<string, BuildPage>} */
const BUILDERS = new Map();

/**
 * Register the builder of a page kind, replacing one already registered.
 * @param {string} kind
 * @param {BuildPage} build
 */
export function registerPageBuilder(kind, build) {
  BUILDERS.set(kind, build);
}

/**
 * The builder of a page kind; null when none is registered.
 * @param {string | null | undefined} kind
 * @returns {BuildPage | null}
 */
export function pageBuilderOf(kind) {
  return BUILDERS.get(kind ?? '') ?? null;
}

/** The kinds that have a builder, sorted: for a test that every screen's kind has one. */
export function registeredBuilderKinds() {
  return [...BUILDERS.keys()].sort();
}
