// The renderer's closed vocabulary (lean/Strata/Client/Dom.lean): a tag outside it is drawn as a span,
// and an attribute outside it is dropped, so no node a view model or a tenant script names can be
// script-capable or set a handler, a style, an inline document or a form-action override.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { allowedAttr, h } from '../src/components/dom.js';

test('a tag outside the vocabulary is drawn as a span', () => {
  for (const tag of ['script', 'iframe', 'object', 'embed', 'base', 'link', 'meta', 'template', 'use', 'animate']) {
    assert.equal(h(tag, null, 'text').tag, 'span', tag);
  }
  assert.equal(h('p', null).tag, 'p');
  assert.equal(h('svg', null).tag, 'svg');
});

test('an event handler, a style, an inline document, a form action override or an unlisted name is dropped', () => {
  const node = h('a', {
    href: '/x',
    onclick: 'alert(1)',
    ONMOUSEOVER: 'alert(1)',
    style: 'x',
    srcdoc: '<script>alert(1)</script>',
    formaction: 'javascript:alert(1)',
    xlink: 'x',
  });
  assert.deepEqual(Object.keys(node.attrs), ['href']);
});

test('listed names and the data- and aria- families are kept', () => {
  assert.equal(allowedAttr('class'), true);
  assert.equal(allowedAttr('data-action'), true);
  assert.equal(allowedAttr('aria-label'), true);
  assert.equal(allowedAttr('onload'), false);
  assert.equal(allowedAttr('style'), false);
  assert.equal(allowedAttr('formaction'), false);
});
