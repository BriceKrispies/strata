/-
Contrast of a theme (docs/design-system.md, "Tokens and themes").

A theme is token values only. The tenant gives a few seed values; every colour a text or a control
is drawn in on a surface is either given and checked, or derived. WCAG 2.2 level AA asks a contrast
ratio of at least 4.5 to 1 for normal text, where the ratio of two relative luminances is
`(lighter + 0.05) / (darker + 0.05)`.

The model works in fixed point: a relative luminance is a natural number from 0 (black) to 10000
(white), so 0.05 is 500, and the ratio test `(hi + 0.05) / (lo + 0.05) ≥ 4.5` is the integer test
`9 * (lo + 500) ≤ 2 * (hi + 500)`. The implementation computes luminance from sRGB with the WCAG
formula in floating point and rounds into this scale conservatively (the lighter colour down, the
darker up), so a pair the fixed-point test accepts also passes the real test; that rounding rule is
the implementation's obligation, tested there, not proved here.

Proved:

- **the derived on-colour always passes AA** (`onColour_meets_aa`): for a surface of any luminance,
  black on it when its luminance is at least 0.1791, white otherwise, meets 4.5 to 1. This is why a
  derivation that looks for an on-colour along a ramp of the theme's own tints can always finish:
  the ramp ends at black or white (`search_finds`);
- **the theme check is exact** (`checkTheme_iff`): a theme passes the check if and only if every
  pair the catalogue declares as text on a surface meets AA.

No `sorry`, no added axioms.
-/

namespace Strata.UI.Contrast

/-- White, in fixed point (relative luminance 1). -/
def white : Nat := 10000
/-- Black (relative luminance 0). -/
def black : Nat := 0

/-- `hi` on `lo` (or `lo` on `hi`) meets 4.5 to 1, with `hi` the lighter. -/
def meetsAA (hi lo : Nat) : Bool := decide (9 * (lo + 500) ≤ 2 * (hi + 500))

/-- The AA test of two luminances in either order. -/
def contrastAA (a b : Nat) : Bool := if b ≤ a then meetsAA a b else meetsAA b a

/-- The on-colour derived for a surface: black on a light surface, white on a dark one. -/
def onColour (l : Nat) : Nat := if 1791 ≤ l then black else white

/-- Verifies: DS-005 -/
theorem onColour_meets_aa (l : Nat) (h : l ≤ 10000) : contrastAA l (onColour l) = true := by
  unfold onColour contrastAA meetsAA white black
  by_cases hl : 1791 ≤ l
  · simp only [hl, Nat.zero_le, ite_true]
    exact decide_eq_true (by omega)
  · have h2 : ¬ (10000 ≤ l) := by omega
    simp only [hl, h2, ite_false]
    exact decide_eq_true (by omega)

/-- The first candidate on-colour, in the order given, that meets AA on the surface. -/
def search (l : Nat) (cands : List Nat) : Option Nat := cands.find? (contrastAA l)

/-- A search along any ramp that ends at the derived on-colour finds an on-colour that meets AA.
    Verifies: DS-005 -/
theorem search_finds (l : Nat) (h : l ≤ 10000) (cands : List Nat) :
    ∃ c, search l (cands ++ [onColour l]) = some c ∧ contrastAA l c = true := by
  unfold search
  induction cands with
  | nil =>
    refine ⟨onColour l, ?_, onColour_meets_aa l h⟩
    simp [onColour_meets_aa l h]
  | cons x xs ih =>
    by_cases hx : contrastAA l x = true
    · exact ⟨x, by simp [hx], hx⟩
    · obtain ⟨c, hc, hok⟩ := ih
      refine ⟨c, ?_, hok⟩
      simp only [List.cons_append, List.find?]
      simp only [Bool.not_eq_true] at hx
      simp [hx, hc]

/-- The theme check: every declared pair (text token, surface token) meets AA. -/
def checkTheme {τ : Type} (lum : τ → Nat) (pairs : List (τ × τ)) : Bool :=
  pairs.all (fun p => contrastAA (lum p.1) (lum p.2))

/-- Verifies: DS-006 -/
theorem checkTheme_iff {τ : Type} (lum : τ → Nat) (pairs : List (τ × τ)) :
    checkTheme lum pairs = true ↔ ∀ p ∈ pairs, contrastAA (lum p.1) (lum p.2) = true := by
  unfold checkTheme
  simp [List.all_eq_true]

end Strata.UI.Contrast
