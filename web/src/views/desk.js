// The writing desk: the author's list, the article editor, publication settings.
// @ts-check

import { h } from '../components/dom.js';
import { hrefFor } from '../generated/routes.js';
import { control, date, loadMore, prose, qrCode } from '../components/parts.js';

const STATUS = { draft: 'Draft', published: 'Published' };

/** The signed-in person's own sessions, with a way to end each of the others. */
function sessionsSection(sessions) {
  if (!sessions?.loaded && !sessions?.error) return null;
  const others = sessions.list.filter((s) => !s.current).length;
  return h(
    'section',
    { class: 'sessions', 'data-sessions': true, 'aria-labelledby': 'sessions-title' },
    h('h2', { id: 'sessions-title', class: 'section-title' }, 'Where you are signed in'),
    sessions.error ? h('p', { class: 'form__error', role: 'alert' }, sessions.error) : null,
    h(
      'table',
      { class: 'table' },
      h('caption', { class: 'visually-hidden' }, 'Your sessions'),
      h('thead', null, h('tr', null, h('th', { scope: 'col' }, 'Session'), h('th', { scope: 'col' }, 'Signed in'), h('th', { scope: 'col' }, 'Last used'), h('th', { scope: 'col' }, 'Actions'))),
      h(
        'tbody',
        null,
        sessions.list.map((s) =>
          h(
            'tr',
            { key: s.id, 'data-session': s.id, 'data-current': s.current ? 'true' : 'false' },
            h('th', { scope: 'row' }, s.current ? 'This device' : 'Another device'),
            h('td', null, date(s.created_at, 'stamp')),
            h('td', null, date(s.last_seen_at, 'stamp')),
            h(
              'td',
              { class: 'table__actions' },
              s.current
                ? null
                : h('button', { class: 'button button--small button--quiet', type: 'button', disabled: sessions.busy, 'data-action': 'sessions.revoke', 'data-arg': arg({ id: s.id }), 'aria-label': 'Sign out that session' }, 'Sign out'),
            ),
          ),
        ),
      ),
    ),
    others
      ? h('p', { class: 'toolbar' }, h('button', { class: 'button button--quiet', type: 'button', disabled: sessions.busy, 'data-action': 'sessions.revoke_others' }, 'Sign out all other sessions'))
      : h('p', { class: 'empty' }, 'This is the only place you are signed in.'),
  );
}

/** The signed-in person's own passkeys: add one on this device, remove any. */
function passkeysSection(passkeys) {
  if (!passkeys?.supported || (!passkeys.loaded && !passkeys.error)) return null;
  return h(
    'section',
    { class: 'sessions', 'data-passkeys': true, 'aria-labelledby': 'passkeys-title' },
    h('h2', { id: 'passkeys-title', class: 'section-title' }, 'Passkeys'),
    passkeys.error ? h('p', { class: 'form__error', role: 'alert' }, passkeys.error) : null,
    passkeys.list.length
      ? h(
          'table',
          { class: 'table' },
          h('caption', { class: 'visually-hidden' }, 'Your passkeys'),
          h('thead', null, h('tr', null, h('th', { scope: 'col' }, 'Passkey'), h('th', { scope: 'col' }, 'Added'), h('th', { scope: 'col' }, 'Last used'), h('th', { scope: 'col' }, 'Actions'))),
          h(
            'tbody',
            null,
            passkeys.list.map((p) =>
              h(
                'tr',
                { key: p.id, 'data-passkey': p.id },
                h('th', { scope: 'row' }, p.name),
                h('td', null, date(p.created_at, 'stamp')),
                h('td', null, p.last_used_at ? date(p.last_used_at, 'stamp') : 'Never'),
                h(
                  'td',
                  { class: 'table__actions' },
                  h('button', { class: 'button button--small button--quiet', type: 'button', disabled: passkeys.busy, 'data-action': 'passkeys.remove', 'data-arg': arg({ id: p.id }), 'aria-label': `Remove ${p.name}` }, 'Remove'),
                ),
              ),
            ),
          ),
        )
      : h('p', { class: 'empty' }, 'You have no passkeys. Add one to sign in without a password.'),
    h('p', { class: 'toolbar' }, h('button', { class: 'button button--quiet', type: 'button', disabled: passkeys.busy, 'data-action': 'passkeys.add' }, 'Add a passkey')),
  );
}

/** The step-up prompt: an operation was refused for want of a fresh authentication. */
function stepUpPrompt(f) {
  const code = f.summary?.totp;
  return h(
    'form',
    { class: 'form form--narrow', 'data-form': 'stepup', role: 'alertdialog', 'aria-labelledby': 'stepup-title', novalidate: true },
    h('h3', { id: 'stepup-title', class: 'section-title' }, 'Confirm that it is you'),
    h('p', null, code ? 'Enter the code your authenticator shows to go on.' : 'Enter your password to go on.'),
    f.stepUp.error ? h('p', { class: 'form__error', role: 'alert' }, f.stepUp.error) : null,
    h(
      'div',
      { class: 'field' },
      h('label', { for: 'stepup-secret' }, code ? 'Authenticator code' : 'Password'),
      h('input', { id: 'stepup-secret', name: 'secret', type: code ? 'text' : 'password', value: f.fields.secret, autocomplete: code ? 'one-time-code' : 'current-password', inputmode: code ? 'numeric' : null, required: true }),
    ),
    h(
      'p',
      { class: 'toolbar' },
      h('button', { class: 'button', type: 'submit', disabled: f.stepUp.busy }, 'Confirm'),
      h('button', { class: 'button button--quiet', type: 'button', 'data-action': 'factors.stepup_cancel' }, 'Cancel'),
    ),
  );
}

/** The signed-in person's second factors: an authenticator, recovery codes, and the prompts. */
function factorsSection(f) {
  if (!f?.loaded && !f?.error) return null;
  const s = f.summary;
  return h(
    'section',
    { class: 'sessions', 'data-factors': true, 'aria-labelledby': 'factors-title' },
    h('h2', { id: 'factors-title', class: 'section-title' }, 'Second factor'),
    f.error ? h('p', { class: 'form__error', role: 'alert' }, f.error) : null,
    f.stepUp ? stepUpPrompt(f) : null,
    f.codes
      ? h(
          'div',
          { class: 'banner', role: 'status', 'data-recovery-codes': true },
          h('p', null, 'Keep these recovery codes somewhere safe. Each one works once, and they are not shown again.'),
          h('ul', { class: 'codes' }, f.codes.map((c) => h('li', { key: c }, h('code', null, c)))),
          h('p', { class: 'toolbar' }, h('button', { class: 'button button--small', type: 'button', 'data-action': 'factors.dismiss_codes' }, 'I have saved them')),
        )
      : null,
    f.enrol
      ? h(
          'form',
          { class: 'form form--narrow', 'data-form': 'totpconfirm', novalidate: true },
          h('p', null, 'Scan this code with an authenticator application, then enter the code it shows.'),
          qrCode(f.enrol.qr, 'QR code for your authenticator'),
          h('p', null, 'Or type this secret: ', h('code', { 'data-totp-secret': true }, f.enrol.secret)),
          f.enrol.error ? h('p', { class: 'form__error', role: 'alert' }, f.enrol.error) : null,
          h(
            'div',
            { class: 'field' },
            h('label', { for: 'totp-confirm' }, 'Code from your authenticator'),
            h('input', { id: 'totp-confirm', name: 'confirm', type: 'text', value: f.fields.confirm, autocomplete: 'one-time-code', inputmode: 'numeric', required: true }),
          ),
          h(
            'p',
            { class: 'toolbar' },
            h('button', { class: 'button', type: 'submit', disabled: f.busy }, 'Turn on'),
            h('button', { class: 'button button--quiet', type: 'button', 'data-action': 'factors.enrol_cancel' }, 'Cancel'),
          ),
        )
      : s?.totp
        ? h(
            'div',
            null,
            h('p', { role: 'status' }, 'An authenticator is set up.'),
            h('p', { 'data-recovery-left': s.recovery_codes }, s.recovery_codes === 1 ? '1 recovery code left.' : `${s.recovery_codes} recovery codes left.`),
            h(
              'p',
              { class: 'toolbar' },
              h('button', { class: 'button button--quiet', type: 'button', disabled: f.busy, 'data-action': 'factors.new_codes' }, 'Make new recovery codes'),
              h('button', { class: 'button button--quiet', type: 'button', disabled: f.busy, 'data-action': 'factors.remove_totp' }, 'Remove the authenticator'),
            ),
          )
        : h(
            'div',
            null,
            h('p', { class: 'empty' }, 'You sign in with one factor. Add an authenticator for a second.'),
            h('p', { class: 'toolbar' }, h('button', { class: 'button button--quiet', type: 'button', disabled: f.busy, 'data-action': 'factors.enrol_start' }, 'Set up an authenticator')),
          ),
  );
}

function arg(value) {
  return JSON.stringify(value);
}

export function desk(page) {
  return [
    h('h1', { class: 'page-title' }, 'Writing desk'),
    h('p', { class: 'standfirst' }, `Signed in as ${page.name}.`),
    h(
      'p',
      { class: 'toolbar' },
      page.canWrite ? h('a', { class: 'button', href: hrefFor('screen-desk-new') }, 'Write a new article') : null,
      page.canSettings ? h('a', { class: 'button button--quiet', href: hrefFor('screen-desk-settings') }, 'Publication settings') : null,
      page.canLab ? h('a', { class: 'button button--quiet', href: hrefFor('screen-lab') }, 'Impact Lab') : null,
    ),
    page.posts.length
      ? h(
          'table',
          { class: 'table' },
          h('caption', { class: 'visually-hidden' }, 'Your articles'),
          h(
            'thead',
            null,
            h('tr', null, h('th', { scope: 'col' }, 'Article'), h('th', { scope: 'col' }, 'Status'), h('th', { scope: 'col' }, 'Last changed'), h('th', { scope: 'col' }, 'Actions')),
          ),
          h(
            'tbody',
            null,
            page.posts.map((p) =>
              h(
                'tr',
                { key: p.id, 'data-post': p.id },
                h('th', { scope: 'row' }, h('a', { href: p.editHref }, p.title), p.author ? h('span', { class: 'table__sub' }, p.author.name) : null),
                h('td', null, h('span', { class: `status status--${p.status}` }, STATUS[p.status] ?? p.status)),
                h('td', null, date(p.updated, 'stamp')),
                h(
                  'td',
                  { class: 'table__actions' },
                  p.status === 'published'
                    ? h('a', { class: 'button button--small button--quiet', href: p.href }, 'View')
                    : null,
                  p.status === 'published'
                    ? h('button', { class: 'button button--small button--quiet', type: 'button', disabled: p.pending, 'data-action': 'post.unpublish', 'data-arg': arg({ id: p.id, version: p.version }), 'aria-label': p.labels.unpublish }, 'Unpublish')
                    : h('button', { class: 'button button--small', type: 'button', disabled: p.pending, 'data-action': 'post.publish', 'data-arg': arg({ id: p.id, version: p.version }), 'aria-label': p.labels.publish }, 'Publish'),
                  p.confirming
                    ? h('button', { class: 'button button--small button--danger', type: 'button', 'data-action': 'post.delete', 'data-arg': arg({ id: p.id, version: p.version }), 'aria-label': p.labels.confirmRemove }, 'Confirm delete')
                    : h('button', { class: 'button button--small button--quiet', type: 'button', 'data-action': 'post.ask_delete', 'data-arg': arg({ id: p.id }), 'aria-label': p.labels.remove }, 'Delete'),
                ),
              ),
            ),
          ),
        )
      : h('p', { class: 'empty' }, 'You have not written anything yet. Start with a draft; nobody sees it until you publish.'),
    loadMore(page.more),
    sessionsSection(page.sessions),
    passkeysSection(page.passkeys),
    factorsSection(page.factors),
  ];
}

const MARKS = [
  ['bold', 'Bold', 'Make the selected text bold'],
  ['italic', 'Italic', 'Make the selected text italic'],
  ['heading', 'Heading', 'Turn the current line into a heading'],
  ['quote', 'Quote', 'Turn the current line into a quotation'],
  ['list', 'List', 'Turn the current line into a list item'],
  ['link', 'Link', 'Turn the selected text into a link'],
];

function select(name, field, options, extra = {}) {
  return h(
    'select',
    { name, value: field.value, ...extra },
    options.map((o) => h('option', { key: o.value, value: o.value, selected: o.value === field.value }, o.label)),
  );
}

export function editor(page) {
  const f = page.fields;
  const errors = Object.values(f).filter((field) => field.error).length;
  const writing = page.tab === 'write';
  return [
    h('h1', { class: 'page-title' }, page.isNew ? 'New article' : 'Edit article'),
    h(
      'p',
      { class: 'editor__state', role: 'group', 'aria-label': 'Article state' },
      h('span', { class: `status status--${page.status}` }, STATUS[page.status] ?? page.status),
      page.dirty ? h('span', { class: 'editor__dirty' }, 'Unsaved changes') : page.isNew ? null : h('span', { class: 'editor__dirty' }, 'All changes saved'),
      page.viewHref && page.status === 'published' ? h('a', { href: page.viewHref }, 'View the published article') : null,
      page.viewHref && page.status !== 'published' ? h('a', { href: page.viewHref }, 'Preview as a page') : null,
    ),
    page.remoteChanged
      ? h(
          'p',
          { class: 'banner', role: 'alert' },
          'Someone else saved this article while you were editing. Saving will be refused until you load their version. ',
          h('button', { class: 'button button--small', type: 'button', 'data-action': 'editor.reload' }, 'Discard my changes and reload'),
        )
      : null,
    h(
      'form',
      { class: 'form editor', 'data-form': 'editor', 'data-rev': page.rev, novalidate: true },
      page.formError || errors
        ? h(
            'p',
            { class: 'form__error', role: 'alert' },
            page.formError || (errors === 1 ? 'One field needs attention before this can be saved.' : `${errors} fields need attention before this can be saved.`),
          )
        : null,
      control('editor-title', 'Title', f.title, h('input', { name: 'title', type: 'text', value: f.title.value, 'data-rev': page.rev, maxlength: 160, required: true })),
      control(
        'editor-slug', 'Address', f.slug,
        h('input', { name: 'slug', type: 'text', value: f.slug.value, 'data-rev': `${page.rev}:${page.isNew ? f.slug.value : ''}`, spellcheck: 'false', autocapitalize: 'none', required: true }),
        'The last part of the article’s web address. Lower-case letters, digits and hyphens.',
      ),
      control(
        'editor-excerpt', 'Excerpt', f.excerpt,
        h('textarea', { name: 'excerpt', rows: 3, value: f.excerpt.value, 'data-rev': page.rev }),
        'One or two sentences shown in lists. Leave empty to use the opening paragraph.',
      ),
      h(
        'div',
        { class: f.body.error ? 'field field--error' : 'field' },
        h('label', { for: 'editor-body' }, 'Text'),
        h(
          'p',
          { class: 'field__hint', id: 'editor-body-hint' },
          'Written in Markdown: blank lines between paragraphs, ## for headings, > for quotations, - for lists, *italic*, **bold**, [text](address).',
        ),
        h(
          'div',
          { class: 'editor__bar' },
          h(
            'div',
            { class: 'segmented', role: 'group', 'aria-label': 'Editor mode' },
            h('button', { type: 'button', class: 'segmented__item', 'aria-pressed': writing ? 'true' : 'false', 'data-action': 'editor.tab', 'data-arg': arg('write') }, 'Write'),
            h('button', { type: 'button', class: 'segmented__item', 'aria-pressed': writing ? 'false' : 'true', 'data-action': 'editor.tab', 'data-arg': arg('preview') }, 'Preview'),
          ),
          writing
            ? h(
                'div',
                { class: 'marks', role: 'toolbar', 'aria-label': 'Formatting', 'aria-controls': 'editor-body' },
                MARKS.map(([mark, label, title]) => h('button', { key: mark, type: 'button', class: 'marks__item', 'data-mark': mark, title }, label)),
              )
            : null,
        ),
        h('textarea', {
          id: 'editor-body', name: 'body', rows: 22, class: 'editor__text', hidden: !writing, value: f.body.value, 'data-rev': page.rev,
          'aria-describedby': f.body.error ? 'editor-body-hint editor-body-error' : 'editor-body-hint', 'aria-invalid': f.body.error ? 'true' : null, required: true,
        }),
        writing
          ? null
          : h(
              'div',
              { class: 'editor__preview prose', role: 'region', 'aria-label': 'Preview', 'aria-live': 'polite', 'aria-busy': page.preview?.pending ? 'true' : 'false' },
              page.preview?.failed
                ? h('p', { class: 'empty' }, 'The preview could not be produced. Your text is safe; switch back to Write.')
                : page.preview?.blocks
                  ? page.preview.blocks.length
                    ? prose(page.preview.blocks)
                    : h('p', { class: 'empty' }, 'Nothing to preview yet.')
                  : h('p', { class: 'empty' }, 'Preparing the preview…'),
            ),
        f.body.error ? h('p', { class: 'field__error', id: 'editor-body-error' }, f.body.error) : null,
      ),
      h(
        'fieldset',
        { class: 'fieldset' },
        h('legend', null, 'Filing'),
        h(
          'div',
          { class: 'field-row' },
          control('editor-category', 'Topic', f.category, select('category', f.category, page.topics)),
          page.canChooseAuthor
            ? control('editor-author', 'Author', f.author, select('author', f.author, page.authors))
            : h('input', { type: 'hidden', name: 'author', value: f.author.value }),
        ),
        control('editor-tags', 'Tags', f.tags, h('input', { name: 'tags', type: 'text', value: f.tags.value, 'data-rev': page.rev }), 'Separated by commas.'),
        h(
          'div',
          { class: 'field field--check' },
          h('input', { id: 'editor-featured', name: 'featured', type: 'checkbox', checked: f.featured.value === true }),
          h('label', { for: 'editor-featured' }, 'Feature at the top of the front page'),
        ),
      ),
      h(
        'fieldset',
        { class: 'fieldset' },
        h('legend', null, 'Cover image'),
        control('editor-cover', 'Image', f.cover, select('cover', f.cover, page.covers), 'Chosen from the publication’s own drawings.'),
        f.cover.value ? h('img', { class: 'editor__cover', src: f.cover.value, alt: '', width: 320, height: 240 }) : null,
        f.cover.value || f.cover_alt.error
          ? control(
              'editor-cover_alt', 'Description of the image', f.cover_alt,
              h('textarea', { name: 'cover_alt', rows: 2, value: f.cover_alt.value, 'data-rev': page.rev }),
              'Read aloud to people who cannot see the image. Say what is in it.',
            )
          : null,
      ),
      h(
        'div',
        { class: 'form__actions' },
        h('button', { class: page.status === 'published' ? 'button' : 'button button--quiet', type: 'submit', disabled: page.busy }, page.status === 'published' ? 'Save changes' : 'Save draft'),
        page.status === 'published'
          ? h('button', { class: 'button button--quiet', type: 'button', disabled: page.busy, 'data-action': 'editor.unpublish' }, 'Unpublish')
          : h('button', { class: 'button', type: 'button', disabled: page.busy, 'data-action': 'editor.publish' }, 'Publish'),
        h('a', { class: 'button button--quiet', href: hrefFor('screen-desk') }, 'Back to the desk'),
      ),
    ),
  ];
}

const SWATCH_NAMES = { moss: 'Moss', clay: 'Clay', ink: 'Ink', ochre: 'Ochre', plum: 'Plum', linen: 'Linen', stone: 'Stone', chalk: 'Chalk' };

function swatches(name, legend, field, options) {
  return h(
    'fieldset',
    { class: 'fieldset swatches' },
    h('legend', null, legend),
    h(
      'div',
      { class: 'swatches__row' },
      options.map((option) =>
        h(
          'label',
          { key: option, class: 'swatch' },
          h('input', { type: 'radio', name, value: option, checked: field.value === option }),
          h('span', { class: `swatch__chip swatch__chip--${option}`, 'aria-hidden': 'true' }),
          h('span', null, SWATCH_NAMES[option] ?? option),
        ),
      ),
    ),
    field.error ? h('p', { class: 'field__error' }, field.error) : null,
  );
}

export function settings(page) {
  const f = page.fields;
  return [
    h('h1', { class: 'page-title' }, 'Publication settings'),
    h(
      'p',
      { class: 'standfirst' },
      'These are stored as a record of the publication. Saving changes every open page straight away, with no reload.',
    ),
    h(
      'form',
      { class: 'form form--narrow', 'data-form': 'settings', 'data-rev': page.rev, novalidate: true },
      page.formError ? h('p', { class: 'form__error', role: 'alert' }, page.formError) : null,
      control('settings-name', 'Name', f.name, h('input', { name: 'name', type: 'text', value: f.name.value, 'data-rev': page.rev, required: true })),
      control('settings-tagline', 'Tagline', f.tagline, h('input', { name: 'tagline', type: 'text', value: f.tagline.value, 'data-rev': page.rev }), 'Shown under the name on the front page.'),
      swatches('accent', 'Accent colour', f.accent, page.accents),
      swatches('paper', 'Paper', f.paper, page.papers),
      h(
        'div',
        { class: 'form__actions' },
        h('button', { class: 'button', type: 'submit', disabled: page.busy }, page.busy ? 'Saving…' : 'Save settings'),
        h('a', { class: 'button button--quiet', href: hrefFor('screen-desk') }, 'Back to the desk'),
      ),
    ),
  ];
}
