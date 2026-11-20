// @ts-check
// The page's own reading of the rules the server carries to it as text (the rule of a
// constraint, the guard of a workflow transition), with its copy of the tenant rule language.
// The copy is generated from the language declaration (`tenant-rule-language.js`) and is held
// to the server's by the shared case table (specc's tests/language_js.rs), so what the page
// parses, prints, checks and evaluates is what the server does. The server prints a rule in the
// authoring syntax; a rule the page reads and prints back differently is a disagreement to
// show, not to hide.
import { ParseError, parse, print } from '../generated/tenant-rule-language.js';

/**
 * What differs between the server's text of a rule and the page's reading of it, or null when
 * the page parses the text and prints it back unchanged.
 * @param {unknown} text
 * @returns {string | null}
 */
export function ruleDisagreement(text) {
  if (typeof text !== 'string') return null;
  try {
    const printed = print(parse(text));
    return printed === text ? null : `the page prints it as ${printed}`;
  } catch (e) {
    if (e instanceof ParseError) return e.message;
    throw e;
  }
}
