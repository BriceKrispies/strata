// View-model derivation: everything the main thread needs to draw, and nothing else.
// Pure and serialisable. The main thread never sees kernel state, only this.
// @ts-check

import { labView } from './lab.js';

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
  return row ? { name: row.fields.name, href: `/authors/${row.fields.slug}`, role: row.fields.title ?? '' } : null;
}

function topic(row) {
  return row ? { name: row.fields.name, href: `/topics/${row.fields.slug}`, description: row.fields.description ?? '' } : null;
}

/** An article as a list entry. */
function entry(row, optimistic = {}) {
  const f = row.fields;
  return {
    id: row.id,
    version: row.version,
    href: `/posts/${f.slug}`,
    editHref: `/desk/edit/${f.slug}`,
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

function field(form, name) {
  return { value: form.values[name], error: form.errors?.[name] ?? '' };
}

function page(state) {
  const { view, session, forms } = state;
  if (view.status === 'loading' && !view.model) return { kind: 'loading' };
  if (view.status === 'missing') return { kind: 'missing', path: state.route.path };
  if (view.status === 'error') return { kind: 'error', message: view.error?.message ?? 'The page could not be loaded.' };
  const model = view.model;
  const d = model.data;
  const signedIn = session?.authenticated === true;
  const canWrite = signedIn && (session.may?.post ?? []).includes('create');
  const guarded = ['desk', 'desk_new', 'desk_edit', 'desk_settings', 'lab'].includes(model.view);
  // While the next page is on its way, a guarded page the visitor just left is not shown again.
  if (guarded && !signedIn) return view.status === 'loading' ? { kind: 'loading' } : { kind: 'signin_required', next: state.route.path };

  switch (model.view) {
    case 'home':
      return {
        kind: 'home',
        tagline: d.site?.fields.tagline ?? '',
        featured: d.featured ? entry(d.featured) : null,
        recent: d.recent.filter((r) => r.id !== d.featured?.id).map((r) => entry(r)),
        topics: d.categories.map(topic),
      };
    case 'post': {
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
    }
    case 'topics':
      return {
        kind: 'topics',
        topics: d.categories.map((c) => ({ ...topic(c), count: d.recent.filter((p) => p.fields.category === c.id).length })),
      };
    case 'topic':
      if (!d.category) return { kind: 'missing', path: state.route.path };
      return { kind: 'topic', topic: topic(d.category), posts: d.posts.map((r) => entry(r)), topics: d.categories.map(topic) };
    case 'authors':
      return { kind: 'authors', authors: d.authors.map((a) => ({ ...person(a), bio: a.fields.bio ?? '' })) };
    case 'author':
      if (!d.author) return { kind: 'missing', path: state.route.path };
      return { kind: 'author', author: { ...person(d.author), bio: d.author.fields.bio ?? '' }, posts: d.posts.map((r) => entry(r)) };
    case 'search':
      return { kind: 'search', q: state.route.query.q ?? '', results: d.results.map((r) => entry(r)) };
    case 'about':
      return {
        kind: 'about',
        name: d.site?.fields.name ?? '',
        blocks: d.site?.blocks?.about ?? [],
        colophon: d.site?.blocks?.colophon ?? [],
        authors: d.authors.map((a) => ({ ...person(a), bio: a.fields.bio ?? '' })),
      };
    case 'login':
      return {
        kind: 'login',
        signedIn,
        name: session?.display_name ?? '',
        login: forms.login.values.login,
        password: forms.login.values.password,
        busy: forms.login.busy,
        error: forms.login.error,
      };
    case 'desk':
      return {
        kind: 'desk',
        name: session.display_name,
        canWrite,
        canSettings: (session.may?.publication ?? []).includes('update'),
        canLab: session.may_manage_schema,
        posts: d.posts
          .map((r) => ({ ...entry(r, state.optimistic), confirming: state.confirmDelete === r.id }))
          .filter((p) => p.status !== 'deleted'),
      };
    case 'desk_new':
    case 'desk_edit': {
      const editor = forms.editor;
      if (!editor) return model.view === 'desk_edit' ? { kind: 'missing', path: state.route.path } : { kind: 'loading' };
      if (!canWrite) return { kind: 'forbidden' };
      return {
        kind: 'editor',
        isNew: editor.id == null,
        status: editor.status,
        dirty: editor.dirty,
        busy: editor.busy,
        rev: editor.rev,
        formError: editor.formError,
        remoteChanged: editor.remoteChanged,
        viewHref: editor.id != null ? `/posts/${editor.key}` : null,
        fields: Object.fromEntries(Object.keys(editor.values).map((name) => [name, field(editor, name)])),
        authors: d.authors.map((a) => ({ value: String(a.id), label: a.fields.name })),
        topics: [{ value: '', label: 'No topic' }, ...d.categories.map((c) => ({ value: String(c.id), label: c.fields.name }))],
        covers: COVERS,
        canChooseAuthor: session.roles.includes('editor') || session.roles.includes('owner'),
        tab: editor.tab,
        preview: editor.preview ? { blocks: editor.preview.blocks, pending: editor.preview.request !== 0, failed: editor.preview.failed } : null,
      };
    }
    case 'desk_settings': {
      const settings = forms.settings;
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
    }
    case 'lab':
      if (!session.may_manage_schema) return { kind: 'forbidden' };
      return labView(state);
    default:
      return { kind: 'missing', path: state.route.path };
  }
}

const PAGE_TITLES = {
  loading: '',
  missing: 'Page not found',
  error: 'Something went wrong',
  forbidden: 'Not allowed',
  signin_required: 'Sign in',
  home: '',
  topics: 'Topics',
  authors: 'Authors',
  search: 'Search',
  about: 'About',
  login: 'Sign in',
  desk: 'Writing desk',
  settings: 'Publication settings',
  lab: 'Impact Lab',
};

function titleOf(p) {
  switch (p.kind) {
    case 'post':
      return p.title;
    case 'topic':
      return p.topic.name;
    case 'author':
      return p.author.name;
    case 'editor':
      return p.isNew ? 'New article' : `Editing: ${p.fields.title.value || 'Untitled'}`;
    default:
      return PAGE_TITLES[p.kind] ?? '';
  }
}

/**
 * Derive the view model.
 * @param {ReturnType<typeof import('./update.js').initialState>} state
 */
export function view(state) {
  const p = page(state);
  const session = state.session;
  const signedIn = session?.authenticated === true;
  const path = state.route.path;
  const pageTitle = titleOf(p);
  const nav = [
    { href: '/', label: 'Essays', current: path === '/' || path.startsWith('/posts/') },
    { href: '/topics', label: 'Topics', current: path.startsWith('/topics') },
    { href: '/authors', label: 'Authors', current: path.startsWith('/authors') },
    { href: '/about', label: 'About', current: path === '/about' },
  ];
  return {
    title: pageTitle ? `${pageTitle} — ${state.site.name}` : state.site.name || 'Strata',
    pageTitle,
    theme: {
      accent: state.theme.accent,
      paper: state.theme.paper,
      scheme: state.theme.mode === 'auto' ? (state.theme.systemDark ? 'dark' : 'light') : state.theme.mode,
      mode: state.theme.mode,
    },
    shell: {
      siteName: state.site.name,
      tagline: state.site.tagline,
      nav,
      menuOpen: state.menuOpen,
      searchQ: path === '/search' ? state.route.query.q ?? '' : '',
      signedIn,
      userName: signedIn ? session.display_name : '',
      deskCurrent: path.startsWith('/desk') || path === '/lab',
      loginHref: path === '/login' ? '/login' : `/login?next=${encodeURIComponent(path)}`,
      connection: state.ws.status,
      refreshing: state.view.status === 'loading' && !!state.view.model,
    },
    page: p,
    toasts: state.toasts,
    announce: state.announce,
    focus: state.focus,
  };
}
