// What the gateway contracts promise that the BFF leaves to the page.
// @ts-check
//
// The web application calls the BFF's generic records endpoint (`POST /api/records`) for the
// article and settings contracts (specifications/spec.jsonc: create-article, update-article,
// save-site-settings). Their postconditions state, in prose, how the inputs become record fields:
// a topic or author is a record identifier sent as text, tags are separated by commas, an empty
// optional input leaves the field unset, and a new article is created as a draft whatever status
// is asked, publishing being a second step. Nothing in the specification's structure says so, so
// the conversion stays here, between the generated forms (web/src/generated/forms.js) and the
// generated requests (web/src/generated/contracts.js).

/** @param {unknown} value */
const text = (value) => (value == null ? '' : String(value));
/** @param {unknown} value */
const trimmed = (value) => text(value).trim();
/** @param {unknown} value */
const trimmedOrNull = (value) => trimmed(value) || null;
/** @param {unknown} value */
const idOrNull = (value) => (value === '' || value == null ? null : Number(value));

/**
 * The inputs a records contract sends, converted to the record fields they fill. Only the inputs
 * given are converted; the others stay absent, so an update leaves those fields as they are.
 * For a new article it also says whether publishing follows as a second step.
 * @param {string} contract
 * @param {Record<string, any>} inputs
 * @returns {{ inputs: Record<string, any>, publishAfterCreate: boolean }}
 */
export function gatewayInputs(contract, inputs) {
  /** @type {Record<string, (value: any) => any>} */
  let convert = {};
  switch (contract) {
    case 'create-article':
    case 'update-article':
      convert = {
        title: trimmed,
        slug: trimmed,
        excerpt: trimmedOrNull,
        body: text,
        category: idOrNull,
        author: idOrNull,
        tags: (value) => {
          const tags = text(value).split(',').map((t) => t.trim()).filter(Boolean);
          return tags.length ? tags : null;
        },
        cover: (value) => value || null,
        cover_alt: trimmedOrNull,
        featured: (value) => (value ? true : null),
      };
      break;
    case 'save-site-settings':
      convert = { name: trimmed, tagline: trimmedOrNull };
      break;
    default:
      return { inputs, publishAfterCreate: false };
  }
  /** @type {Record<string, any>} */
  const out = {};
  for (const [name, value] of Object.entries(inputs)) {
    if (value === undefined) continue;
    out[name] = convert[name] ? convert[name](value) : value;
  }
  if (contract !== 'create-article') return { inputs: out, publishAfterCreate: false };
  // A new article is created as a draft, with only the fields that hold a value.
  const publishAfterCreate = out.status === 'published';
  delete out.status;
  for (const [name, value] of Object.entries(out)) if (value === null) delete out[name];
  return { inputs: out, publishAfterCreate };
}
