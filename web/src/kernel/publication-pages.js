// The publication's page builders: for each page kind its screens carry, the page model the
// renderer of that kind draws (views/renderers.js). They are registered here, in one module, so a
// product registers its own builders (`registerPageBuilder`) without editing the view derivation
// (view.js). Every address a page model holds is the generated route of a screen (`hrefFor`).
// @ts-check

import * as actions from '../generated/actions.js';
import { hrefFor } from '../generated/routes.js';
import { rowActionsOn } from '../generated/screens.js';
import { labView } from './lab.js';
import { registerPageBuilder } from './page-builders.js';

/** Cover images bundled with the application. Tenant data may only name one of these. */
export const COVERS = [
  { value: '', label: 'No cover' },
  { value: '/media/jig.svg', label: 'Fixture and chair leg' },
  { value: '/media/margin.svg', label: 'Open book with margins' },
  { value: '/media/kiln.svg', label: 'Kiln mouth and pots' },
  { value: '/media/slow.svg', label: 'Growth rings' },
  { value: '/media/drawing.svg', label: 'Three sketched doors' },
  { value: '/media/hours.svg', label: 'Row of cylinders' },
];

const ACCENTS = ['moss', 'clay', 'ink', 'ochre', 'plum'];
const PAPERS = ['linen', 'stone', 'chalk'];

function inlineText(inlines) {
  return (inlines ?? []).map((i) => (i.children ? inlineText(i.children) : i.text ?? '')).join('');
}

/** The first paragraph of an article, for when it has no excerpt of its own. */
function firstParagraph(blocks) {
  const paragraph = (blocks ?? []).find((b) => b.type === 'paragraph');
  const text = paragraph ? inlineText(paragraph.inlines) : '';
  if (text.length <= 230) return text;
  const cut = text.slice(0, 230);
  return `${cut.slice(0, cut.lastIndexOf(' '))}…`;
}

function person(row) {
  return row ? { name: row.fields.name, href: hrefFor('screen-author', { slug: row.fields.slug }), role: row.fields.title ?? '' } : null;
}

function topic(row) {
  return row ? { name: row.fields.name, href: hrefFor('screen-topic', { slug: row.fields.slug }), description: row.fields.description ?? '' } : null;
}

/** An article as a list entry. */
function entry(row, optimistic = {}) {
  const f = row.fields;
  return {
    id: row.id,
    version: row.version,
    href: hrefFor('screen-article', { slug: f.slug }),
    editHref: hrefFor('screen-desk-edit', { slug: f.slug }),
    title: f.title,
    excerpt: f.excerpt || firstParagraph(row.blocks?.body),
    date: f.published_at ?? null,
    updated: row.updated_at,
    author: person(row.refs?.author),
    topic: topic(row.refs?.category),
    minutes: row.reading_minutes ?? null,
    cover: f.cover ?? null,
    coverAlt: f.cover_alt ?? '',
    status: optimistic[row.id] ?? f.status,
    pending: row.id in optimistic,
  };
}

/**
 * The names the generated actions give a row's buttons: for each row action of the view's screen
 * (`ROW_ACTIONS`), the row of the view model the action runs on, filled into the action's label
 * under its label key and, for a confirmed action, its confirmation under `confirm` and the key
 * capitalised.
 * @param {any} model @param {any} row
 */
function rowLabels(model, row) {
  /** @type {Record<string, string>} */
  const labels = {};
  for (const button of rowActionsOn(model.view)) {
    const item = actions.rows(button.action, model).find((/** @type {any} */ r) => r[button.rowKey] === row.id) ?? { title: row.fields.title };
    labels[button.labelKey] = actions.label(button.action, item);
    if (button.confirm) labels[`confirm${button.labelKey.charAt(0).toUpperCase()}${button.labelKey.slice(1)}`] = actions.confirmText(button.action, item) ?? '';
  }
  return labels;
}

function field(form, name) {
  return { value: form.values[name], error: form.errors?.[name] ?? '' };
}

registerPageBuilder('home', (_state, { d }) => ({
  kind: 'home',
  tagline: d.site?.fields.tagline ?? '',
  featured: d.featured ? entry(d.featured) : null,
  recent: d.recent.filter((r) => r.id !== d.featured?.id).map((r) => entry(r)),
  topics: d.categories.map(topic),
}));

registerPageBuilder('post', (state, { d, session, canWrite }) => {
  if (!d.post) return { kind: 'missing', path: state.route.path };
  const f = d.post.fields;
  return {
    kind: 'post',
    ...entry(d.post),
    isDraft: f.status !== 'published',
    blocks: d.post.blocks?.body ?? [],
    tags: f.tags ?? [],
    authorBio: d.post.refs?.author?.fields.bio ?? '',
    canEdit: canWrite && (session.principal === d.post.created_by || session.roles.includes('editor') || session.roles.includes('owner')),
    more: d.recent.filter((r) => r.id !== d.post.id).slice(0, 3).map((r) => entry(r)),
  };
});

registerPageBuilder('topics', (_state, { d }) => ({
  kind: 'topics',
  topics: d.categories.map((c) => ({ ...topic(c), count: d.recent.filter((p) => p.fields.category === c.id).length })),
}));

registerPageBuilder('topic', (state, { d }) => {
  if (!d.category) return { kind: 'missing', path: state.route.path };
  return { kind: 'topic', topic: topic(d.category), posts: d.posts.map((r) => entry(r)), topics: d.categories.map(topic) };
});

registerPageBuilder('authors', (_state, { d }) => ({ kind: 'authors', authors: d.authors.map((a) => ({ ...person(a), bio: a.fields.bio ?? '' })) }));

registerPageBuilder('author', (state, { d }) => {
  if (!d.author) return { kind: 'missing', path: state.route.path };
  return { kind: 'author', author: { ...person(d.author), bio: d.author.fields.bio ?? '' }, posts: d.posts.map((r) => entry(r)) };
});

registerPageBuilder('search', (state, { d }) => ({ kind: 'search', q: state.route.query.q ?? '', results: d.results.map((r) => entry(r)) }));

registerPageBuilder('about', (_state, { d }) => ({
  kind: 'about',
  name: d.site?.fields.name ?? '',
  blocks: d.site?.blocks?.about ?? [],
  colophon: d.site?.blocks?.colophon ?? [],
  authors: d.authors.map((a) => ({ ...person(a), bio: a.fields.bio ?? '' })),
}));

registerPageBuilder('login', (state, { session, signedIn }) => {
  const { forms } = state;
  return {
    kind: 'login',
    signedIn,
    name: session?.display_name ?? '',
    login: forms.login.values.login,
    password: forms.login.values.password,
    busy: forms.login.busy,
    error: forms.login.error,
    passkeys: { supported: state.passkeys.supported, busy: state.passkeys.signingIn, error: state.passkeys.signInError },
    link: { ...state.links, landing: state.links.landingStatus },
    oidc: { ...state.oidc, landing: state.oidc.landingStatus },
    // The owner's invitation: the form that chooses a password while the link is held, and the
    // refusal once the link cannot be used. Neither holds the secrets of the link.
    invite: {
      open: state.invites.landing !== null,
      failed: state.invites.failed,
      busy: state.invites.busy,
      password: state.invites.password,
      fieldError: state.invites.fieldError,
      error: state.invites.error,
    },
    second: state.factors.prompt ? { ...state.factors.prompt, code: state.factors.fields.code, passkeys: state.passkeys.supported } : null,
  };
});

registerPageBuilder('desk', (state, { model, d, session, canWrite }) => ({
  kind: 'desk',
  name: session.display_name,
  canWrite,
  canSettings: (session.may?.publication ?? []).includes('update'),
  canLab: session.may_manage_schema,
  sessions: state.sessions,
  passkeys: state.passkeys,
  factors: state.factors,
  posts: d.posts
    .map((r) => ({ ...entry(r, state.optimistic), confirming: state.confirmDelete === r.id, labels: rowLabels(model, r) }))
    .filter((p) => p.status !== 'deleted'),
}));

// The new-article and the edit screens share one renderer, so one builder serves both views.
registerPageBuilder('editor', (state, { model, d, session, canWrite }) => {
  const editor = state.forms.editor;
  if (!editor) return model.view === 'desk_edit' ? { kind: 'missing', path: state.route.path } : { kind: 'loading' };
  if (!canWrite) return { kind: 'forbidden' };
  return {
    kind: 'editor',
    // The title follows what is typed, so it is the page's own and not the screen's stated one.
    pageTitle: editor.id == null ? 'New article' : `Editing: ${editor.values.title || 'Untitled'}`,
    isNew: editor.id == null,
    status: editor.status,
    dirty: editor.dirty,
    busy: editor.busy,
    rev: editor.rev,
    formError: editor.formError,
    remoteChanged: editor.remoteChanged,
    viewHref: editor.id != null ? hrefFor('screen-article', { slug: editor.key }) : null,
    fields: Object.fromEntries(Object.keys(editor.values).map((name) => [name, field(editor, name)])),
    authors: d.authors.map((a) => ({ value: String(a.id), label: a.fields.name })),
    topics: [{ value: '', label: 'No topic' }, ...d.categories.map((c) => ({ value: String(c.id), label: c.fields.name }))],
    covers: COVERS,
    canChooseAuthor: session.roles.includes('editor') || session.roles.includes('owner'),
    tab: editor.tab,
    preview: editor.preview ? { blocks: editor.preview.blocks, pending: editor.preview.request !== 0, failed: editor.preview.failed } : null,
  };
});

registerPageBuilder('settings', (state, { session }) => {
  const settings = state.forms.settings;
  if (!settings) return { kind: 'loading' };
  if (!(session.may?.publication ?? []).includes('update')) return { kind: 'forbidden' };
  return {
    kind: 'settings',
    busy: settings.busy,
    dirty: settings.dirty,
    rev: settings.rev,
    formError: settings.formError,
    fields: Object.fromEntries(Object.keys(settings.values).map((name) => [name, field(settings, name)])),
    accents: ACCENTS,
    papers: PAPERS,
  };
});

registerPageBuilder('lab', (state, { session }) => {
  if (!session.may_manage_schema) return { kind: 'forbidden' };
  return labView(state);
});
