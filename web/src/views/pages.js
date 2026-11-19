// The reader's pages. Each function takes its part of the view model and returns elements.
// @ts-check

import { h } from '../components/dom.js';
import { date, index, marginNotes, prose } from '../components/parts.js';

function topicList(topics, label) {
  if (!topics.length) return null;
  return h(
    'nav',
    { class: 'topics', 'aria-label': label },
    h('ul', null, topics.map((t) => h('li', { key: t.href }, h('a', { href: t.href }, t.name)))),
  );
}

export function home(page, shell) {
  const lead = page.featured;
  return [
    h('h1', { class: 'visually-hidden' }, shell.siteName),
    page.tagline ? h('p', { class: 'standfirst' }, page.tagline) : null,
    lead
      ? h(
          'section',
          { class: 'lead', 'aria-labelledby': 'lead-title' },
          h(
            'div',
            { class: 'lead__text' },
            h('h2', { class: 'lead__title', id: 'lead-title' }, h('a', { href: lead.href }, lead.title)),
            lead.excerpt ? h('p', { class: 'lead__excerpt' }, lead.excerpt) : null,
            marginNotes(lead),
          ),
          lead.cover ? h('img', { class: 'lead__art', src: lead.cover, alt: lead.coverAlt, width: 640, height: 480 }) : null,
        )
      : null,
    h(
      'section',
      { class: 'section', 'aria-labelledby': 'recent-title' },
      h('h2', { class: 'section__title', id: 'recent-title' }, lead ? 'Earlier essays' : 'Essays'),
      page.recent.length || lead
        ? index(page.recent)
        : h('p', { class: 'empty' }, 'Nothing has been published yet. When an essay is, it appears here without a reload.'),
    ),
    h(
      'section',
      { class: 'section section--quiet', 'aria-labelledby': 'topics-title' },
      h('h2', { class: 'section__title', id: 'topics-title' }, 'Browse by topic'),
      topicList(page.topics, 'Topics'),
    ),
  ];
}

export function post(page) {
  return [
    h(
      'article',
      { class: 'essay' },
      page.isDraft
        ? h('p', { class: 'banner', role: 'note' }, 'This is a draft. Only you and the editors can see it.')
        : null,
      h(
        'header',
        { class: 'essay__head' },
        h('h1', { class: 'essay__title' }, page.title),
        page.cover ? h('img', { class: 'essay__cover', src: page.cover, alt: page.coverAlt, width: 640, height: 480 }) : null,
      ),
      h(
        'div',
        { class: 'essay__grid' },
        h(
          'aside',
          { class: 'essay__margin', 'aria-label': 'About this essay' },
          marginNotes(page),
          page.tags.length ? h('p', { class: 'tags' }, page.tags.join(', ')) : null,
          page.canEdit ? h('p', null, h('a', { class: 'button button--quiet', href: page.editHref }, 'Edit this essay')) : null,
        ),
        h('div', { class: 'prose', role: 'region', 'aria-label': 'Essay text' }, prose(page.blocks)),
      ),
      page.author
        ? h(
            'footer',
            { class: 'essay__foot' },
            h('h2', { class: 'essay__byline' }, h('a', { href: page.author.href }, page.author.name)),
            page.authorBio ? h('p', null, page.authorBio) : null,
          )
        : null,
    ),
    page.more.length
      ? h(
          'section',
          { class: 'section', 'aria-labelledby': 'more-title' },
          h('h2', { class: 'section__title', id: 'more-title' }, 'More essays'),
          index(page.more),
        )
      : null,
  ];
}

export function topics(page) {
  return [
    h('h1', { class: 'page-title' }, 'Topics'),
    h(
      'ul',
      { class: 'ledger' },
      page.topics.map((t) =>
        h(
          'li',
          { key: t.href, class: 'ledger__item' },
          h('h2', { class: 'ledger__name' }, h('a', { href: t.href }, t.name)),
          h('p', { class: 'ledger__text' }, t.description),
          h('p', { class: 'ledger__count' }, t.count === 1 ? '1 essay' : `${t.count} essays`),
        ),
      ),
    ),
  ];
}

export function topic(page) {
  return [
    h('h1', { class: 'page-title' }, page.topic.name),
    page.topic.description ? h('p', { class: 'standfirst' }, page.topic.description) : null,
    page.posts.length ? index(page.posts, 'h2') : h('p', { class: 'empty' }, 'No essays are filed under this topic yet.'),
    h(
      'section',
      { class: 'section section--quiet', 'aria-labelledby': 'other-topics' },
      h('h2', { class: 'section__title', id: 'other-topics' }, 'All topics'),
      topicList(page.topics, 'All topics'),
    ),
  ];
}

function people(authors, level) {
  return h(
    'ul',
    { class: 'ledger' },
    authors.map((a) =>
      h(
        'li',
        { key: a.href, class: 'ledger__item' },
        h(level, { class: 'ledger__name' }, h('a', { href: a.href }, a.name)),
        h('p', { class: 'ledger__text' }, a.bio),
        a.role ? h('p', { class: 'ledger__count' }, a.role) : null,
      ),
    ),
  );
}

export function authors(page) {
  return [h('h1', { class: 'page-title' }, 'Authors'), people(page.authors, 'h2')];
}

export function author(page) {
  return [
    h('h1', { class: 'page-title' }, page.author.name),
    page.author.role ? h('p', { class: 'page-kicker' }, page.author.role) : null,
    page.author.bio ? h('p', { class: 'standfirst' }, page.author.bio) : null,
    h(
      'section',
      { class: 'section', 'aria-labelledby': 'author-essays' },
      h('h2', { class: 'section__title', id: 'author-essays' }, 'Essays'),
      page.posts.length ? index(page.posts) : h('p', { class: 'empty' }, 'Nothing published yet.'),
    ),
  ];
}

export function search(page) {
  const count = page.results.length;
  const summary = !page.q
    ? 'Type a word or two. Titles, excerpts and the text of every essay are searched.'
    : page.q.length < 2
      ? 'Type at least two characters.'
      : count === 0
        ? `Nothing mentions “${page.q}”. Try a shorter or different word.`
        : `${count === 1 ? '1 essay mentions' : `${count} essays mention`} “${page.q}”.`;
  return [
    h('h1', { class: 'page-title' }, 'Search'),
    h(
      'form',
      { class: 'search-form', role: 'search', 'data-form': 'search', action: '/search' },
      h('label', { for: 'search-page-q' }, 'Search the essays'),
      h(
        'div',
        { class: 'search-form__row' },
        h('input', { id: 'search-page-q', name: 'q', type: 'search', value: page.q, autocomplete: 'off', 'data-rev': page.q }),
        h('button', { class: 'button', type: 'submit' }, 'Search'),
      ),
    ),
    h('p', { class: 'search-summary', role: 'status' }, summary),
    count ? index(page.results, 'h2') : null,
  ];
}

export function about(page) {
  return [
    h('h1', { class: 'page-title' }, `About ${page.name}`),
    h('div', { class: 'prose prose--page' }, prose(page.blocks)),
    h(
      'section',
      { class: 'section', 'aria-labelledby': 'about-people' },
      h('h2', { class: 'section__title', id: 'about-people' }, 'Who writes here'),
      people(page.authors, 'h3'),
    ),
    page.colophon.length
      ? h(
          'section',
          { class: 'section section--quiet', 'aria-labelledby': 'colophon' },
          h('h2', { class: 'section__title', id: 'colophon' }, 'Colophon'),
          h('div', { class: 'prose prose--small' }, prose(page.colophon)),
        )
      : null,
  ];
}

export function login(page) {
  if (page.signedIn) {
    return [
      h('h1', { class: 'page-title' }, 'You are signed in'),
      h('p', { class: 'standfirst' }, `Signed in as ${page.name}.`),
      h('p', null, h('a', { class: 'button', href: '/desk' }, 'Go to the writing desk')),
    ];
  }
  return [
    h('h1', { class: 'page-title' }, 'Sign in'),
    h('p', { class: 'standfirst' }, 'For the people who write and edit here. Readers do not need an account.'),
    h(
      'form',
      { class: 'form form--narrow', 'data-form': 'login', novalidate: true },
      page.error ? h('p', { class: 'form__error', role: 'alert', id: 'login-error' }, page.error) : null,
      h(
        'div',
        { class: 'field' },
        h('label', { for: 'login-name' }, 'Name'),
        h('input', { id: 'login-name', name: 'login', type: 'text', value: page.login, autocomplete: 'username', autocapitalize: 'none', spellcheck: 'false', required: true }),
      ),
      h(
        'div',
        { class: 'field' },
        h('label', { for: 'login-password' }, 'Password'),
        h('input', {
          id: 'login-password', name: 'password', type: 'password', value: page.password, autocomplete: 'current-password', required: true,
          'aria-describedby': page.error ? 'login-error' : null, 'data-rev': page.error,
        }),
      ),
      h('button', { class: 'button', type: 'submit', disabled: page.busy }, page.busy ? 'Signing in…' : 'Sign in'),
    ),
    page.passkeys?.supported
      ? h(
          'div',
          { class: 'form form--narrow', 'data-passkey-signin': true },
          page.passkeys.error ? h('p', { class: 'form__error', role: 'alert', id: 'passkey-error' }, page.passkeys.error) : null,
          h('button', { class: 'button button--quiet', type: 'button', disabled: page.passkeys.busy, 'data-action': 'passkeys.sign_in' }, page.passkeys.busy ? 'Waiting for your passkey…' : 'Sign in with a passkey'),
        )
      : null,
  ];
}

export function missing(page) {
  return [
    h('h1', { class: 'page-title' }, 'There is nothing at this address'),
    h('p', { class: 'standfirst' }, `No page lives at ${page.path}. It may have been unpublished, or the link may be mistyped.`),
    h('p', null, h('a', { class: 'button', href: '/' }, 'Read the latest essays')),
  ];
}

export function failure(page) {
  return [
    h('h1', { class: 'page-title' }, 'This page could not be loaded'),
    h('p', { class: 'standfirst' }, page.message),
    h('p', null, h('button', { class: 'button', type: 'button', 'data-action': 'reload' }, 'Try again')),
  ];
}

export function signinRequired(page) {
  return [
    h('h1', { class: 'page-title' }, 'Sign in to continue'),
    h('p', { class: 'standfirst' }, 'This part of the site is for the people who write and edit here.'),
    h('p', null, h('a', { class: 'button', href: `/login?next=${encodeURIComponent(page.next)}` }, 'Sign in')),
  ];
}

export function forbidden() {
  return [
    h('h1', { class: 'page-title' }, 'Your account cannot open this page'),
    h('p', { class: 'standfirst' }, 'It needs a role you do not hold. An editor or the site owner can change that.'),
    h('p', null, h('a', { class: 'button', href: '/desk' }, 'Back to the writing desk')),
  ];
}

export function loading() {
  return [h('p', { class: 'loading', role: 'status' }, 'Loading…')];
}

export { date };
