// The page frame: skip link, masthead, main region, footer, announcements.
// @ts-check

import { h } from '../components/dom.js';
import { hrefFor } from '../generated/routes.js';
import { missing } from './pages.js';
import { rendererOf } from './renderers.js';

const THEME_LABEL = { auto: 'Theme: device', light: 'Theme: light', dark: 'Theme: dark' };
const CONNECTION = {
  open: 'Live: changes appear as they are published.',
  connecting: 'Connecting for live updates…',
  waiting: 'Offline. Reconnecting; the page will catch up.',
  stopped: 'Live updates have stopped. Reload the page to reconnect.',
  idle: '',
};

/** What the page's kind draws; a kind nothing is registered for is a page that is not there. */
function body(page, shell) {
  const renderer = rendererOf(page.kind);
  return renderer ? renderer.render(page, shell) : missing(page);
}

/** The whole document body for a view model. */
export function shell(model) {
  const s = model.shell;
  const wide = rendererOf(model.page.kind)?.wide === true;
  return [
    h('a', { class: 'skip', href: '#main' }, 'Skip to the content'),
    h(
      'header',
      { class: 'masthead' },
      h(
        'div',
        { class: 'masthead__row' },
        h('a', { class: 'masthead__name', href: hrefFor('screen-front-page'), 'aria-current': model.page.kind === 'home' ? 'page' : null }, s.siteName || 'Strata'),
        h(
          'button',
          { class: 'masthead__toggle', type: 'button', 'aria-expanded': s.menuOpen ? 'true' : 'false', 'aria-controls': 'site-menu', 'data-action': 'menu' },
          s.menuOpen ? 'Close menu' : 'Menu',
        ),
        h(
          'div',
          { class: s.menuOpen ? 'masthead__menu masthead__menu--open' : 'masthead__menu', id: 'site-menu' },
          h(
            'nav',
            { 'aria-label': 'Sections' },
            h(
              'ul',
              { class: 'masthead__nav' },
              s.nav.map((item) => h('li', { key: item.href }, h('a', { href: item.href, 'aria-current': item.current ? 'page' : null }, item.label))),
            ),
          ),
          h(
            'form',
            { class: 'masthead__search', role: 'search', 'data-form': 'search', action: hrefFor('screen-search') },
            h('label', { class: 'visually-hidden', for: 'site-q' }, 'Search the essays'),
            h('input', { id: 'site-q', name: 'q', type: 'search', placeholder: 'Search', value: s.searchQ, 'data-rev': s.searchQ, autocomplete: 'off' }),
            h('button', { class: 'visually-hidden', type: 'submit' }, 'Search'),
          ),
          h(
            'div',
            { class: 'masthead__tools' },
            h('button', { class: 'link-button', type: 'button', 'data-action': 'theme', 'aria-label': `${THEME_LABEL[model.theme.mode]}. Change theme` }, THEME_LABEL[model.theme.mode]),
            s.tools.map((item) => h('a', { key: item.href, href: item.href, 'aria-current': item.current ? 'page' : null }, item.label)),
            s.signedIn
              ? h('button', { class: 'link-button', type: 'button', 'data-action': 'logout' }, 'Sign out')
              : s.loginHref
                ? h('a', { href: s.loginHref }, 'Sign in')
                : null,
          ),
        ),
      ),
    ),
    h(
      'main',
      { id: 'main', tabindex: '-1', 'aria-label': 'Content', class: wide ? 'main main--wide' : 'main', 'aria-busy': s.refreshing ? 'true' : 'false', 'data-page': model.page.kind },
      body(model.page, s),
    ),
    h(
      'footer',
      { class: 'colophon' },
      h('p', { class: 'colophon__name' }, s.siteName),
      s.tagline ? h('p', null, s.tagline) : null,
      h('p', { class: 'colophon__live', role: 'status', 'aria-label': 'Live connection', 'data-connection': s.connection }, CONNECTION[s.connection] ?? ''),
    ),
    h('div', { class: 'visually-hidden', role: 'status', 'aria-live': 'polite', id: 'announcer' }, model.announce),
    h(
      'div',
      { class: 'toasts', role: 'region', 'aria-label': 'Messages' },
      model.toasts.map((t) =>
        h(
          'div',
          { key: t.id, class: `toast toast--${t.kind}` },
          h('p', null, t.text),
          h('button', { class: 'link-button', type: 'button', 'data-action': 'dismiss', 'data-arg': String(t.id), 'aria-label': 'Dismiss this message' }, 'Dismiss'),
        ),
      ),
    ),
  ];
}
