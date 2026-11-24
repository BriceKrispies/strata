// View-model derivation: everything the main thread needs to draw, and nothing else.
// Pure and serialisable. The main thread never sees kernel state, only this.
// @ts-check

import { hrefFor } from '../generated/routes.js';
import { SIGN_IN_PATH, SIGN_IN_SCREEN, guardedView, navigation, pageTitle as statedTitle, screenOfView } from '../generated/screens.js';
import { pageBuilderOf } from './page-builders.js';
import { partOf, tenantPage, tenantScreenOfView } from './tenant-ui.js';
import { hrefOf } from './update.js';
// The publication's page builders register themselves; a product's are registered the same way
// (`registerPageBuilder`), by a module the entry point imports, without editing this file.
import './publication-pages.js';

function page(state) {
  const { view, session } = state;
  if (view.status === 'loading' && !view.model) return { kind: 'loading' };
  if (view.status === 'missing') return { kind: 'missing', path: state.route.path };
  if (view.status === 'error') return { kind: 'error', message: view.error?.message ?? 'The page could not be loaded.' };
  const model = view.model;
  const d = model.data;
  const signedIn = session?.authenticated === true;
  const canWrite = signedIn && (session.may?.post ?? []).includes('create');
  // A view a screen of the tenant's interface definition shows is drawn by the interpreter, from
  // the screen's part (D-8250, INT-3257).
  const tenant = tenantScreenOfView(state.ui, model.view);
  if (tenant) {
    if (tenant.auth === 'session' && !signedIn) return view.status === 'loading' ? { kind: 'loading' } : { kind: 'signin_required', next: hrefOf(state.route) };
    const part = partOf(state.ui, tenant);
    if (!part) return { kind: 'loading' };
    return tenantPage(tenant, part, model, { forms: state.tenant.forms, confirm: state.tenant.confirm, signedIn });
  }
  const guarded = guardedView(model.view);
  // While the next page is on its way, a guarded page the visitor just left is not shown again.
  if (guarded && !signedIn) return view.status === 'loading' ? { kind: 'loading' } : { kind: 'signin_required', next: hrefOf(state.route) };

  const build = pageBuilderOf(screenOfView(model.view)?.kind);
  if (build) return build(state, { model, d, session, signedIn, canWrite });
  // The tenant's interface definition is on its way: the view may be one of its screens.
  const asking = Object.values(state.pending ?? {}).some((p) => p.kind === 'ui.manifest');
  return asking ? { kind: 'loading' } : { kind: 'missing', path: state.route.path };
}

/** The titles of the states the page frame shows itself, which no screen states. */
const FRAME_TITLES = {
  loading: '',
  missing: 'Page not found',
  error: 'Something went wrong',
  forbidden: 'Not allowed',
  signin_required: 'Sign in',
};

/**
 * The title of the page: a frame state's, else the page's own (`pageTitle`), else the one the
 * screen of the view states (generated).
 * @param {any} p @param {any} model
 */
function titleOf(p, model) {
  if (p.kind in FRAME_TITLES) return FRAME_TITLES[/** @type {keyof typeof FRAME_TITLES} */ (p.kind)];
  if (typeof p.pageTitle === 'string') return p.pageTitle;
  return statedTitle(model?.view, model) ?? '';
}

/**
 * Derive the view model.
 * @param {ReturnType<typeof import('./update.js').initialState>} state
 */
export function view(state) {
  const built = page(state);
  const { pageTitle: _own, ...p } = built;
  const session = state.session;
  const signedIn = session?.authenticated === true;
  const path = state.route.path;
  const pageTitle = titleOf(built, state.view.model);
  // The generated navigation: the entries this visitor may see, in the generated order.
  const entries = navigation(path, signedIn).map(({ href, label, area, current }) => ({ href, label, area, current }));
  const nav = entries.filter((e) => e.area === 'menu');
  const tools = entries.filter((e) => e.area === 'tools');
  return {
    title: pageTitle ? `${pageTitle} — ${state.site.name}` : state.site.name || 'Strata',
    pageTitle,
    theme: {
      accent: state.theme.accent,
      paper: state.theme.paper,
      theme: state.theme.preset,
      scheme: state.theme.mode === 'auto' ? (state.theme.systemDark ? 'dark' : 'light') : state.theme.mode,
      mode: state.theme.mode,
    },
    shell: {
      siteName: state.site.name,
      tagline: state.site.tagline,
      nav,
      tools,
      menuOpen: state.menuOpen,
      searchQ: path === hrefFor('screen-search') ? state.route.query.q ?? '' : '',
      signedIn,
      userName: signedIn ? session.display_name : '',
      loginHref: SIGN_IN_SCREEN === null ? null : hrefFor(SIGN_IN_SCREEN, path === SIGN_IN_PATH ? {} : { next: hrefOf(state.route) }),
      connection: state.wsStopped ? 'stopped' : state.ws.status,
      refreshing: state.view.status === 'loading' && !!state.view.model,
    },
    page: p,
    toasts: state.toasts,
    announce: state.announce,
    focus: state.focus,
  };
}
