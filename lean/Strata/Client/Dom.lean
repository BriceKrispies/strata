/-
No cross-site scripting in the browser client (WG-5677, decision D-FE-14, requirement fe-024).

The page draws by applying DOM operations. This file states the closed vocabulary of those
operations, the abstract UI tree that a view model or a tenant script returns (text, and
design-system elements with attributes and children, where tags, attribute names, attribute values
and text are arbitrary strings), and the render function from a UI tree to operations. The render
function is written the way the main thread renders (web/src/components/dom.js, with the elements of
web/src/components/ds.js and web/src/components/parts.js): an element is created only of an allowed
kind (any other kind becomes a `span`), an attribute is set only under an allowed name, a URL
attribute is set only with a URL the policy accepts (web/src/components/dom.js `safeUrl`, the
same-site check of web/src/generated/site-path.js, and the scheme rule of web/src/trusted-urls.js
and the Content Security Policy of crates/bff/src/routes.rs), and text is set only as a text node.

Proved, for every UI tree (so for every tenant-script output and every record value):

- `script_output_emits_no_script_element`: every element the render creates is of an allowed kind,
  and no allowed kind is a script-capable one (`forbiddenTags`);
- `script_output_sets_no_handler_style_or_url`: every attribute the render sets is under an allowed
  name, which is never an event handler (`on*`), `style`, `srcdoc` or `formaction`; a URL attribute
  is set only with a value the URL policy accepts, which is never a `javascript:`, `vbscript:` or
  `data:` address (`urlOk_not_script`);
- `text_is_only_text`: text is emitted as one text node, and nothing else is emitted for it.

Trusted, and not proved here: the browser's DOM (that `createElement`, `createTextNode` and
`setAttribute` do what their names say and that a text node is never parsed), the Content Security
Policy and Trusted Types enforcement the page is served with, the URL parser's resolution of the
accepted addresses, and the small adapter that applies these operations (web/src/components/dom.js).

No unfinished proof and no added axiom.
-/

namespace Strata.Client.Dom

/-- The operations the main thread applies to the document. There is no operation that carries
markup: text goes in only as the payload of `createText`. -/
inductive Op where
  /-- Create an element of a kind (`createElement`, or `createElementNS` inside an SVG). -/
  | createElement (tag : String)
  /-- Create a text node. Its payload is text, never markup. -/
  | createText (text : String)
  /-- Set an attribute of the element being built. -/
  | setAttribute (name value : String)
  /-- Append the node made last to the element it is being built into. -/
  | append
  /-- Remove a node from its parent. Used when a redraw drops a node; it carries no content. -/
  | remove

/-- What a view model or a tenant script returns: a text node, or an element with attributes and
children. Every string in it is arbitrary. -/
inductive UINode where
  | text (s : String)
  | elem (tag : String) (attrs : List (String × String)) (children : List UINode)

/-! ### The vocabulary -/

/-- The element kinds the page creates (web/src/components/*.js and web/src/views/**): the HTML
kinds of the design system, the page's sections and forms, and the SVG kinds of the icons and the
QR code. -/
def allowedTags : List String :=
  [ "a", "aside", "article", "blockquote", "button", "caption", "code", "dd", "details", "div",
    "dl", "dt", "em", "fieldset", "figcaption", "figure", "footer", "form", "h1", "h2", "h3", "h4",
    "header", "hr", "img", "input", "label", "legend", "li", "main", "nav", "ol", "option", "p",
    "path", "pre", "progress", "rect", "section", "select", "span", "strong", "summary", "svg",
    "table", "tbody", "td", "textarea", "th", "thead", "time", "tr", "ul" ]

/-- Script-capable kinds: none of them is ever created. The list states what the closed vocabulary
leaves out; `allowed_not_forbidden` checks that the two lists are disjoint. -/
def forbiddenTags : List String :=
  [ "script", "iframe", "frame", "frameset", "object", "embed", "applet", "param", "base", "link",
    "meta", "style", "template", "noscript", "portal", "foreignObject", "animate",
    "animateMotion", "animateTransform", "set", "use", "handler", "listener" ]

/-- The kind an element is created as: its own tag when it is allowed, else `span` (the text it
holds is still shown, as text). -/
def sanitizeTag (t : String) : String :=
  if t ∈ allowedTags then t else "span"

/-- The attribute names the page sets (web/src/components/*.js, web/src/views/**), and the
`data-*` and `aria-*` families. -/
def allowedAttrs : List String :=
  [ "action", "alt", "autocapitalize", "autocomplete", "checked", "class", "d", "datetime",
    "disabled", "fill", "focusable", "for", "height", "hidden", "href", "id", "inputmode", "lang",
    "loading", "max", "maxlength", "min", "name", "novalidate", "open", "pattern", "placeholder",
    "required", "rel", "role", "rows", "scope", "selected", "shape-rendering", "spellcheck", "src",
    "step", "stroke", "stroke-linejoin", "stroke-width", "tabindex", "title", "type", "value",
    "viewBox", "width" ]

/-- The attributes whose value is an address the browser follows. -/
def urlAttrs : List String := [ "href", "src", "action" ]

/-- Whether a list of characters begins with a given prefix. -/
def startsWithL : List Char → List Char → Bool
  | [], _ => true
  | _ :: _, [] => false
  | p :: ps, c :: cs => (p == c) && startsWithL ps cs

/-- Event handlers, styles, inline documents and form-action overrides: never set. -/
def forbiddenAttr (n : String) : Bool :=
  startsWithL ['o', 'n'] n.toList || n == "style" || n == "srcdoc" || n == "formaction"

/-- An attribute name the render may set: not forbidden, and either listed or in the `data-` or
`aria-` family. -/
def attrNameAllowed (n : String) : Bool :=
  !forbiddenAttr n &&
    (allowedAttrs.contains n || startsWithL ['d', 'a', 't', 'a', '-'] n.toList ||
      startsWithL ['a', 'r', 'i', 'a', '-'] n.toList)

def urlAttr (n : String) : Bool := urlAttrs.contains n

/-- A path on this site: a single leading slash, not a protocol-relative `//` or `/\`, and no
backslash, tab or line break anywhere (the URL parser reads those as separators). The browser's
same-site check (`sitePath`) accepts this set and possibly more; the model is the narrower one. -/
def sameSitePath (cs : List Char) : Bool :=
  cs.head? == some '/' && cs.tail.head? != some '/' &&
    cs.all (fun c => c != '\\' && c != '\t' && c != '\n' && c != '\r')

/-- A scheme the page links to: a fragment, `https:`, `http:` or `mailto:`. -/
def schemeOk (cs : List Char) : Bool :=
  cs.head? == some '#' || startsWithL ['h', 't', 't', 'p', 's', ':'] cs ||
    startsWithL ['h', 't', 't', 'p', ':'] cs || startsWithL ['m', 'a', 'i', 'l', 't', 'o', ':'] cs

/-- The URL policy. An `src` (an image or a script address) is a path on this site and nothing
else; an `href` or `action` is a path on this site or a scheme `schemeOk` accepts. -/
def urlOk (name value : String) : Bool :=
  if name == "src" || startsWithL ['/'] value.toList then sameSitePath value.toList
  else schemeOk value.toList

/-- Addresses that run code, or carry a document a browser renders as code. -/
def isScriptUrl (value : String) : Bool :=
  startsWithL ['j', 'a', 'v', 'a', 's', 'c', 'r', 'i', 'p', 't', ':'] value.toList ||
    startsWithL ['v', 'b', 's', 'c', 'r', 'i', 'p', 't', ':'] value.toList ||
    startsWithL ['d', 'a', 't', 'a', ':'] value.toList

/-! ### The render -/

/-- One attribute, kept when its name is allowed and, for a URL attribute, its URL is accepted. -/
def attrOp (av : String × String) : Option Op :=
  if attrNameAllowed av.1 && (!urlAttr av.1 || urlOk av.1 av.2) then some (.setAttribute av.1 av.2)
  else none

def attrOps (attrs : List (String × String)) : List Op :=
  attrs.filterMap attrOp

mutual

/-- The operations that build one UI node into the element it is appended to. -/
def renderNode : UINode → List Op
  | .text s => [.createText s, .append]
  | .elem tag attrs children =>
      [.createElement (sanitizeTag tag)] ++ attrOps attrs ++ renderList children ++ [.append]

/-- The operations that build a list of UI nodes, in order. -/
def renderList : List UINode → List Op
  | [] => []
  | n :: ns => renderNode n ++ renderList ns

end

/-! ### Properties of the vocabulary and the policy -/

/-- The kind the render creates is always an allowed one. -/
theorem sanitizeTag_mem (t : String) : sanitizeTag t ∈ allowedTags := by
  unfold sanitizeTag
  split
  · assumption
  · decide

/-- No allowed kind is script-capable. -/
theorem allowed_not_forbidden : ∀ t ∈ allowedTags, t ∉ forbiddenTags := by
  decide

/-- An attribute name the render may set is never a handler, a style, an inline document or a
form-action override. -/
theorem allowed_attr_not_forbidden (n : String) (h : attrNameAllowed n = true) :
    forbiddenAttr n = false := by
  unfold attrNameAllowed at h
  cases hf : forbiddenAttr n
  · rfl
  · simp [hf] at h

/-- A list that begins with `p` makes the first character of the other `p`. -/
theorem startsWithL_head (p : Char) (ps : List Char) (cs : List Char)
    (h : startsWithL (p :: ps) cs = true) : cs.head? = some p := by
  cases cs with
  | nil => simp [startsWithL] at h
  | cons c rest =>
      simp only [startsWithL, Bool.and_eq_true, beq_iff_eq] at h
      simp [h.1]

/-- An accepted scheme or path begins with `/`, `#`, `h` or `m`. -/
theorem schemeOk_head (cs : List Char) (h : schemeOk cs = true) :
    cs.head? = some '#' ∨ cs.head? = some 'h' ∨ cs.head? = some 'm' := by
  unfold schemeOk at h
  simp only [Bool.or_eq_true, beq_iff_eq] at h
  rcases h with ((h | h) | h) | h
  · exact Or.inl h
  · exact Or.inr (Or.inl (startsWithL_head 'h' ['t', 't', 'p', 's', ':'] cs h))
  · exact Or.inr (Or.inl (startsWithL_head 'h' ['t', 't', 'p', ':'] cs h))
  · exact Or.inr (Or.inr (startsWithL_head 'm' ['a', 'i', 'l', 't', 'o', ':'] cs h))

/-- An accepted path begins with `/`. -/
theorem sameSitePath_head (cs : List Char) (h : sameSitePath cs = true) :
    cs.head? = some '/' := by
  unfold sameSitePath at h
  simp only [Bool.and_eq_true, beq_iff_eq] at h
  exact h.1.1

/-- An accepted URL begins with `/`, `#`, `h` or `m`. -/
theorem urlOk_head (name value : String) (h : urlOk name value = true) :
    value.toList.head? = some '/' ∨ value.toList.head? = some '#' ∨
      value.toList.head? = some 'h' ∨ value.toList.head? = some 'm' := by
  unfold urlOk at h
  split at h
  · exact Or.inl (sameSitePath_head _ h)
  · rcases schemeOk_head _ h with h' | h' | h'
    · exact Or.inr (Or.inl h')
    · exact Or.inr (Or.inr (Or.inl h'))
    · exact Or.inr (Or.inr (Or.inr h'))

/-- An accepted URL is never a script, data or VBScript address. -/
theorem urlOk_not_script (name value : String) (h : urlOk name value = true) :
    isScriptUrl value = false := by
  have hhead := urlOk_head name value h
  unfold isScriptUrl
  have hj : startsWithL ['j', 'a', 'v', 'a', 's', 'c', 'r', 'i', 'p', 't', ':'] value.toList = false := by
    cases hb : startsWithL ['j', 'a', 'v', 'a', 's', 'c', 'r', 'i', 'p', 't', ':'] value.toList
    · rfl
    · exfalso
      have := startsWithL_head 'j' ['a', 'v', 'a', 's', 'c', 'r', 'i', 'p', 't', ':'] _ hb
      rw [this] at hhead
      simp at hhead
  have hv : startsWithL ['v', 'b', 's', 'c', 'r', 'i', 'p', 't', ':'] value.toList = false := by
    cases hb : startsWithL ['v', 'b', 's', 'c', 'r', 'i', 'p', 't', ':'] value.toList
    · rfl
    · exfalso
      have := startsWithL_head 'v' ['b', 's', 'c', 'r', 'i', 'p', 't', ':'] _ hb
      rw [this] at hhead
      simp at hhead
  have hd : startsWithL ['d', 'a', 't', 'a', ':'] value.toList = false := by
    cases hb : startsWithL ['d', 'a', 't', 'a', ':'] value.toList
    · rfl
    · exfalso
      have := startsWithL_head 'd' ['a', 't', 'a', ':'] _ hb
      rw [this] at hhead
      simp at hhead
  simp [hj, hv, hd]

/-! ### What the operations of a render satisfy -/

/-- What an operation must satisfy to be one the render emits. -/
def Good : Op → Prop
  | .createElement t => t ∈ allowedTags
  | .setAttribute n v => attrNameAllowed n = true ∧ (urlAttr n = true → urlOk n v = true)
  | .createText _ => True
  | .append => True
  | .remove => True

theorem attrOp_good (n v : String) (op : Op) (h : attrOp (n, v) = some op) : Good op := by
  unfold attrOp at h
  split at h
  · rename_i hc
    cases h
    simp only [Bool.and_eq_true] at hc
    exact ⟨hc.1, fun hu => by simpa [hu] using hc.2⟩
  · cases h

theorem attrOps_good (attrs : List (String × String)) : ∀ op ∈ attrOps attrs, Good op := by
  intro op hop
  unfold attrOps at hop
  obtain ⟨⟨n, v⟩, _, hf⟩ := List.mem_filterMap.mp hop
  exact attrOp_good n v op hf

mutual

theorem renderNode_good : ∀ (t : UINode), ∀ op ∈ renderNode t, Good op
  | .text s => by
      intro op hop
      simp only [renderNode, List.mem_cons, List.mem_nil_iff, or_false] at hop
      rcases hop with rfl | rfl <;> simp [Good]
  | .elem tag attrs children => by
      intro op hop
      simp only [renderNode, List.mem_append, List.mem_singleton] at hop
      rcases hop with (((rfl | h) | h) | rfl)
      · exact sanitizeTag_mem tag
      · exact attrOps_good attrs op h
      · exact renderList_good children op h
      · simp [Good]

theorem renderList_good : ∀ (ts : List UINode), ∀ op ∈ renderList ts, Good op
  | [] => by
      intro op hop
      simp [renderList] at hop
  | n :: ns => by
      intro op hop
      simp only [renderList, List.mem_append] at hop
      rcases hop with h | h
      · exact renderNode_good n op h
      · exact renderList_good ns op h

end

/-- Every element the render creates is of an allowed kind, and no allowed kind is script-capable:
no `script`, `iframe`, `object`, `embed`, `base`, `link`, `meta`, `template`, SVG animation or `use`
element is ever created, whatever the UI tree.

Verifies: FE-024 -/
theorem script_output_emits_no_script_element (t : UINode) :
    ∀ tag, Op.createElement tag ∈ renderNode t → tag ∈ allowedTags ∧ tag ∉ forbiddenTags := by
  intro tag h
  have hg : tag ∈ allowedTags := renderNode_good t _ h
  exact ⟨hg, allowed_not_forbidden tag hg⟩

/-- Every attribute the render sets is under an allowed name, never an event handler (`on*`),
`style`, `srcdoc` or `formaction`; and a URL attribute is set only with a URL the policy accepts,
which is never a `javascript:`, `vbscript:` or `data:` address.

Verifies: FE-025, FE-026 -/
theorem script_output_sets_no_handler_style_or_url (t : UINode) :
    ∀ n v, Op.setAttribute n v ∈ renderNode t →
      forbiddenAttr n = false ∧ (urlAttr n = true → isScriptUrl v = false ∧ urlOk n v = true) := by
  intro n v h
  obtain ⟨hallow, hurl⟩ := (renderNode_good t _ h : Good (Op.setAttribute n v))
  refine ⟨allowed_attr_not_forbidden n hallow, fun hu => ?_⟩
  have hok := hurl hu
  exact ⟨urlOk_not_script n v hok, hok⟩

/-- Text is emitted as one text node and nothing else: no element and no attribute is made of it. -/
theorem text_is_only_text (s : String) : renderNode (.text s) = [.createText s, .append] := rfl

end Strata.Client.Dom
