/-
The page the view shows, derived from the browser kernel's state (docs/formal-verification.md, "Browser
kernel: gap analysis", slice 13; web/src/kernel/view.js, `page`).

`pageOf cat s` says which page the view shows: the frame states (loading, missing, the error, the sign-in
page), a screen's data, or the sign-in page for a screen that needs a signed-in person. This slice states
the kind of page only. The page builders of each screen kind (publication-pages.js), the composition of
tenant screens (compose.js, display.js) and the page's title and navigation are not yet in the spec: the
data of a page is the held model, shown as a screen name, with its builder to come.

Theorems: a screen's data is shown only from a model held for the person signed in (`page_data_is_own`);
a visitor is shown no data of a screen that needs a person (`visitor_sees_no_guarded_data`); and a
change of person shows the loading page, never the former person's data (`person_change_shows_loading`).

No unfinished proof and no added axiom.
-/

import Strata.Browser.Session

namespace Strata.Browser

/-- The page the view shows. -/
inductive PageKind where
  /-- Nothing to show yet. -/
  | loading
  /-- The address is not a page. -/
  | missing
  /-- The page could not be loaded. -/
  | error
  /-- The page asks the visitor to sign in (a screen that needs a person, read by a visitor). -/
  | signinRequired
  /-- The data of a screen. -/
  | data (screen : Screen)
  deriving DecidableEq

/-- The page the view shows, from the held model and the view's status. -/
def pageOf (cat : Catalogue) (s : State) : PageKind :=
  match s.view.model with
  | some h =>
    if cat.needsSession h.screen = true ∧ s.session = none then .signinRequired else .data h.screen
  | none =>
    match s.view.status with
    | .missing => .missing
    | .failed => .error
    | _ => .loading

/-- **A screen's data is shown only from a model held for the person signed in now.** -/
theorem page_data_is_own (cat : Catalogue) {s : State} (hs : Reachable cat s) {scr : Screen}
    (h : pageOf cat s = .data scr) :
    ∃ hh, s.view.model = some hh ∧ hh.readFor = s.session ∧ hh.screen = scr := by
  unfold pageOf at h
  cases hm : s.view.model with
  | none =>
    cases hst : s.view.status <;> simp [hm, hst] at h
  | some hh =>
    rw [hm] at h
    refine ⟨hh, rfl, held_is_own cat hs hh hm, ?_⟩
    by_cases hc : cat.needsSession hh.screen = true ∧ s.session = none
    · simp [hc] at h
    · simp only [hc, ite_false] at h
      cases h
      rfl

/-- **A visitor is shown no data of a screen that needs a signed-in person.** -/
theorem visitor_sees_no_guarded_data (cat : Catalogue) {s : State} (hs : Reachable cat s) {scr : Screen}
    (hv : s.session = none) (h : pageOf cat s = .data scr) : cat.needsSession scr = false := by
  obtain ⟨hh, hm, _, hsc⟩ := page_data_is_own cat hs h
  subst hsc
  cases hn : cat.needsSession hh.screen with
  | false => rfl
  | true =>
    have hg := held_guarded_signed_in cat hs hh hm hn
    simp [hv] at hg

/-- **A change of the person shows the loading page.** Nothing of the former person is shown after it. -/
theorem person_change_shows_loading (cat : Catalogue) {s s' : State} {m : Msg} {es : List Effect}
    (ha : Allowed cat s m s' es) (hch : s.session ≠ s'.session) :
    pageOf cat s' = .loading := by
  have hm := session_change_drops_model cat ha hch
  have hst : s'.view.status = ViewStatus.loading := by
    cases ha <;> simp_all [switched, askView, dropViews]
  unfold pageOf
  rw [hm, hst]

end Strata.Browser
