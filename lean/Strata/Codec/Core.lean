/-
The canonical byte codec, part 1: the combinators.

Bytes are `List UInt8` here and a `ByteArray` at the boundary (`Strata.Codec.Boundary`). A decoder
reads a prefix of its input and returns the value with the bytes it did not read.

An encoder `e` and a decoder `d` are `Lawful` when:
- round trip: decoding an encoding, with any bytes after it, returns the value and exactly those
  bytes;
- canonical: any bytes that decode to `(x, rest)` are the encoding of `x` followed by `rest`.

Together these give one encoding per value and no other bytes that decode. Every combinator here
preserves `Lawful`, so a new boundary type is a composition of these plus one proof per tag.

Format of the primitives:
- a tag is one byte, its constructor index (`tag k`);
- a natural is minimal LEB128: seven bits per byte, low group first, the high bit set on every
  byte but the last; a multi-byte natural never ends in a zero group (so `0` is one byte);
- an integer is a sign tag (`0` for a non-negative value, `1` for a negative one) and the natural
  `i` (non-negative) or `-i - 1` (negative);
- a boolean is one byte, `0` or `1`;
- a string is the natural length of its UTF-8 bytes, then the bytes; decoding checks that the
  bytes are exactly the UTF-8 of the string they decode to, so invalid UTF-8 does not decode;
- a list is the natural count of its elements, then the elements;
- an option is the tag `0` for none, or `1` then the value;
- a pair is its components in order.
-/

namespace Strata.Codec

abbrev Bytes := List UInt8

/-- A decoder reads a prefix of its input and returns the value with the rest. -/
abbrev Dec (α : Type) := Bytes → Option (α × Bytes)

/-- An encoder and a decoder are lawful together: round trip, and every decoding is canonical. -/
def Lawful {α : Type} (e : α → Bytes) (d : Dec α) : Prop :=
  (∀ (x : α) (rest : Bytes), d (e x ++ rest) = some (x, rest)) ∧
  (∀ (bs : Bytes) (x : α) (rest : Bytes), d bs = some (x, rest) → bs = e x ++ rest)

/-! ## Bytes -/

/-- The tag byte of constructor index `k`. -/
def tag (k : Nat) : UInt8 := UInt8.ofNat k

theorem toNat_tag {k : Nat} (h : k < 256) : (tag k).toNat = k := by
  unfold tag
  rw [UInt8.toNat_ofNat']
  have h256 : (2 : Nat) ^ 8 = 256 := by decide
  rw [h256]
  exact Nat.mod_eq_of_lt h

theorem tag_toNat (b : UInt8) : tag b.toNat = b := by
  unfold tag
  exact UInt8.ofNat_toNat

theorem byte_eq_tag (b : UInt8) (k : Nat) (h : b.toNat = k) : b = tag k := by
  rw [← h, tag_toNat]

/-! ## Naturals: minimal LEB128 -/

def encNat (n : Nat) : Bytes :=
  if h : n < 128 then [tag n]
  else tag (n % 128 + 128) :: encNat (n / 128)
termination_by n
decreasing_by omega

def decNat : Bytes → Option (Nat × Bytes)
  | [] => none
  | b :: r =>
    if b.toNat < 128 then some (b.toNat, r)
    else match decNat r with
      | some (m, r') => if m = 0 then none else some (b.toNat % 128 + 128 * m, r')
      | none => none

theorem decNat_encNat (n : Nat) (rest : Bytes) :
    decNat (encNat n ++ rest) = some (n, rest) := by
  induction n using Nat.strongRecOn generalizing rest with
  | _ n ih =>
    rw [encNat]
    split
    · rename_i h
      simp only [List.cons_append]
      rw [decNat, toNat_tag (by omega), if_pos (by omega)]
      simp
    · rename_i h
      have hb : (tag (n % 128 + 128)).toNat = n % 128 + 128 :=
        toNat_tag (by omega)
      have hrec := ih (n / 128) (by omega) rest
      have hne : ¬ (n / 128 = 0) := by omega
      simp only [List.cons_append]
      rw [decNat, hb, if_neg (by omega), hrec]
      simp only [hne, if_false]
      congr 2
      omega

theorem decNat_canon : ∀ (bs : Bytes) (n : Nat) (rest : Bytes),
    decNat bs = some (n, rest) → bs = encNat n ++ rest
  | [], n, rest, h => by simp [decNat] at h
  | b :: r, n, rest, h => by
    unfold decNat at h
    split at h
    · rename_i hlt
      simp only [Option.some.injEq, Prod.mk.injEq] at h
      obtain ⟨rfl, rfl⟩ := h
      rw [encNat]
      have hlt' : b.toNat < 128 := hlt
      rw [dif_pos hlt']
      rw [List.singleton_append, tag_toNat]
    · rename_i hlt
      split at h
      · rename_i m r' hrec
        split at h
        · simp at h
        · rename_i hm
          simp only [Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := decNat_canon r m r' hrec
          have hge : ¬ (b.toNat % 128 + 128 * m < 128) := by omega
          rw [encNat, dif_neg hge]
          have hv1 : (b.toNat % 128 + 128 * m) % 128 + 128 = b.toNat := by
            have := b.toNat_lt_size
            simp only [UInt8.size] at this
            omega
          have hv2 : (b.toNat % 128 + 128 * m) / 128 = m := by omega
          rw [hv2, hv1, List.cons_append, ← hr, tag_toNat]
      · simp at h

/-- The natural codec is lawful. -/
theorem lawful_nat : Lawful encNat decNat :=
  ⟨decNat_encNat, decNat_canon⟩

/-! ## Integers: a sign tag, then the natural magnitude (`i`, or `-i - 1` when negative) -/

def encInt (i : Int) : Bytes :=
  if 0 ≤ i then tag 0 :: encNat i.toNat else tag 1 :: encNat (-i - 1).toNat

def decInt : Bytes → Option (Int × Bytes)
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => match decNat r with
      | some (m, r') => some ((m : Int), r')
      | none => none
    | 1 => match decNat r with
      | some (m, r') => some (-(m : Int) - 1, r')
      | none => none
    | _ => none

theorem decInt_encInt (i : Int) (rest : Bytes) : decInt (encInt i ++ rest) = some (i, rest) := by
  unfold encInt
  by_cases h : 0 ≤ i
  · rw [if_pos h]
    have hm := Int.toNat_of_nonneg h
    simp [decInt, tag, decNat_encNat, hm]
  · rw [if_neg h]
    have hm := Int.toNat_of_nonneg (show 0 ≤ -i - 1 by omega)
    simp [decInt, tag, decNat_encNat, hm]
    omega

theorem decInt_canon : ∀ (bs : Bytes) (i : Int) (rest : Bytes),
    decInt bs = some (i, rest) → bs = encInt i ++ rest
  | [], i, rest, h => by simp [decInt] at h
  | b :: r, i, rest, h => by
    rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ (b.toNat ≠ 0 ∧ b.toNat ≠ 1) by omega) with hk | hk | hk
    · simp only [decInt, hk] at h
      cases hd : decNat r with
      | none => simp [hd] at h
      | some p =>
        obtain ⟨m, r'⟩ := p
        simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        have hr := decNat_canon r m r' hd
        have hnn : 0 ≤ (m : Int) := Int.natCast_nonneg m
        unfold encInt
        rw [if_pos hnn, Int.toNat_natCast, byte_eq_tag b 0 hk, hr]
        simp
    · simp only [decInt, hk] at h
      cases hd : decNat r with
      | none => simp [hd] at h
      | some p =>
        obtain ⟨m, r'⟩ := p
        simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        have hr := decNat_canon r m r' hd
        have hneg : ¬ (0 ≤ -(m : Int) - 1) := by omega
        unfold encInt
        rw [if_neg hneg]
        have hm : (-(-(m : Int) - 1) - 1).toNat = m := by omega
        rw [hm, byte_eq_tag b 1 hk, hr]
        simp
    · simp [decInt, hk.1, hk.2] at h

theorem lawful_int : Lawful encInt decInt :=
  ⟨decInt_encInt, decInt_canon⟩

/-! ## Booleans -/

def encBool (b : Bool) : Bytes := if b then [tag 1] else [tag 0]

def decBool : Bytes → Option (Bool × Bytes)
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => some (false, r)
    | 1 => some (true, r)
    | _ => none

theorem decBool_encBool (x : Bool) (rest : Bytes) :
    decBool (encBool x ++ rest) = some (x, rest) := by
  cases x <;> simp [encBool, decBool, tag]

theorem decBool_canon : ∀ (bs : Bytes) (x : Bool) (rest : Bytes),
    decBool bs = some (x, rest) → bs = encBool x ++ rest
  | [], x, rest, h => by simp [decBool] at h
  | b :: r, x, rest, h => by
    rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ (b.toNat ≠ 0 ∧ b.toNat ≠ 1) by omega) with hk | hk | hk
    · simp only [decBool, hk, Option.some.injEq, Prod.mk.injEq] at h
      obtain ⟨rfl, rfl⟩ := h
      simp [encBool, byte_eq_tag b 0 hk]
    · simp only [decBool, hk, Option.some.injEq, Prod.mk.injEq] at h
      obtain ⟨rfl, rfl⟩ := h
      simp [encBool, byte_eq_tag b 1 hk]
    · simp [decBool, hk.1, hk.2] at h

theorem lawful_bool : Lawful encBool decBool :=
  ⟨decBool_encBool, decBool_canon⟩

/-! ## Strings: the UTF-8 length, then the bytes; decoding checks the bytes are the string's own
    UTF-8, so only valid UTF-8 decodes. -/

/-- The string the bytes decode to, lossily; `decStr` accepts it only when re-encoding gives back
    the bytes. -/
def strOf (body : Bytes) : String := String.fromUTF8Lossy ⟨body.toArray⟩

def encStr (s : String) : Bytes := encNat s.toUTF8.size ++ s.toUTF8.data.toList

def decStr (bs : Bytes) : Option (String × Bytes) :=
  match decNat bs with
  | some (n, r) =>
    if n ≤ r.length ∧ (strOf (r.take n)).toUTF8.data.toList = r.take n then
      some (strOf (r.take n), r.drop n)
    else none
  | none => none

theorem byteArray_eta (ba : ByteArray) : (⟨ba.data.toList.toArray⟩ : ByteArray) = ba := by
  cases ba
  simp

theorem strOf_toUTF8 (s : String) : strOf s.toUTF8.data.toList = s := by
  unfold strOf
  rw [byteArray_eta, String.toUTF8_eq_toByteArray, String.fromUTF8Lossy_toByteArray]

theorem length_toUTF8 (s : String) : s.toUTF8.data.toList.length = s.toUTF8.size := by
  exact Array.length_toList

theorem decStr_encStr (s : String) (rest : Bytes) : decStr (encStr s ++ rest) = some (s, rest) := by
  unfold encStr decStr
  rw [List.append_assoc, decNat_encNat]
  dsimp only
  have htake : (s.toUTF8.data.toList ++ rest).take s.toUTF8.size = s.toUTF8.data.toList := by
    rw [← length_toUTF8]
    exact List.take_left
  have hdrop : (s.toUTF8.data.toList ++ rest).drop s.toUTF8.size = rest := by
    rw [← length_toUTF8]
    exact List.drop_left
  have hlen : s.toUTF8.size ≤ (s.toUTF8.data.toList ++ rest).length := by
    rw [List.length_append, length_toUTF8]
    omega
  have hcond : s.toUTF8.size ≤ (s.toUTF8.data.toList ++ rest).length ∧
      (strOf ((s.toUTF8.data.toList ++ rest).take s.toUTF8.size)).toUTF8.data.toList =
        (s.toUTF8.data.toList ++ rest).take s.toUTF8.size := by
    rw [htake, strOf_toUTF8]
    exact ⟨hlen, rfl⟩
  rw [if_pos hcond, htake, hdrop, strOf_toUTF8]

theorem decStr_canon : ∀ (bs : Bytes) (s : String) (rest : Bytes),
    decStr bs = some (s, rest) → bs = encStr s ++ rest := by
  intro bs s rest h
  unfold decStr at h
  split at h
  · rename_i n r hdec
    split at h
    · rename_i hcheck
      simp only [Option.some.injEq, Prod.mk.injEq] at h
      obtain ⟨rfl, rfl⟩ := h
      have hbs := decNat_canon bs n r hdec
      have hlen : (strOf (r.take n)).toUTF8.data.toList.length = n := by
        rw [hcheck.2, List.length_take]
        omega
      have hsize : (strOf (r.take n)).toUTF8.size = n := by
        rw [← length_toUTF8, hlen]
      rw [hbs]
      unfold encStr
      rw [hsize, hcheck.2, List.append_assoc, List.take_append_drop]
    · simp at h
  · simp at h

theorem lawful_str : Lawful encStr decStr :=
  ⟨decStr_encStr, decStr_canon⟩

/-! ## Combinators -/

/-- A pair: the first component, then the second. -/
def encPair {α β : Type} (ea : α → Bytes) (eb : β → Bytes) (p : α × β) : Bytes :=
  ea p.1 ++ eb p.2

def decPair {α β : Type} (da : Dec α) (db : Dec β) : Dec (α × β) := fun bs =>
  match da bs with
  | some (a, r) => match db r with
    | some (b, r') => some ((a, b), r')
    | none => none
  | none => none

theorem lawful_pair {α β : Type} {ea : α → Bytes} {da : Dec α} {eb : β → Bytes} {db : Dec β}
    (ha : Lawful ea da) (hb : Lawful eb db) : Lawful (encPair ea eb) (decPair da db) := by
  refine ⟨?_, ?_⟩
  · intro p rest
    simp only [encPair, decPair, List.append_assoc, ha.1, hb.1]
  · intro bs p rest h
    unfold decPair at h
    cases hda : da bs with
    | none => simp [hda] at h
    | some ar =>
      obtain ⟨a', r⟩ := ar
      cases hdb : db r with
      | none => simp [hda, hdb] at h
      | some br =>
        obtain ⟨b', r'⟩ := br
        simp only [hda, hdb, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨⟨rfl, rfl⟩, rfl⟩ := h
        have h1 := ha.2 bs _ r hda
        have h2 := hb.2 r _ r' hdb
        simp only [encPair]
        rw [h1, h2, List.append_assoc]

/-- An isomorphism: a codec for `β` reached through `g` (from `α`) and `f` (back). -/
def encIso {α β : Type} (e : β → Bytes) (g : α → β) (x : α) : Bytes := e (g x)

def decIso {α β : Type} (d : Dec β) (f : β → α) : Dec α := fun bs =>
  match d bs with
  | some (y, r) => some (f y, r)
  | none => none

theorem lawful_iso {α β : Type} {e : β → Bytes} {d : Dec β} (h : Lawful e d)
    {f : β → α} {g : α → β} (hfg : ∀ x, f (g x) = x) (hgf : ∀ y, g (f y) = y) :
    Lawful (encIso e g) (decIso d f) := by
  refine ⟨?_, ?_⟩
  · intro x rest
    simp only [encIso, decIso, h.1, hfg]
  · intro bs x rest hd
    unfold decIso at hd
    cases hdy : d bs with
    | none => simp [hdy] at hd
    | some yr =>
      obtain ⟨y, r⟩ := yr
      simp only [hdy, Option.some.injEq, Prod.mk.injEq] at hd
      obtain ⟨rfl, rfl⟩ := hd
      simp only [encIso]
      rw [hgf]
      exact h.2 bs y r hdy

/-- An option: the tag `0` for none, or `1` and the value. -/
def encOpt {α : Type} (e : α → Bytes) : Option α → Bytes
  | none => [tag 0]
  | some x => tag 1 :: e x

def decOpt {α : Type} (d : Dec α) : Dec (Option α) := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => some (none, r)
    | 1 => match d r with
      | some (x, r') => some (some x, r')
      | none => none
    | _ => none

theorem lawful_opt {α : Type} {e : α → Bytes} {d : Dec α} (h : Lawful e d) :
    Lawful (encOpt e) (decOpt d) := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x with
    | none => simp [encOpt, decOpt, tag]
    | some x => simp [encOpt, decOpt, tag, h.1]
  · intro bs x rest hd
    cases bs with
    | nil => simp [decOpt] at hd
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ (b.toNat ≠ 0 ∧ b.toNat ≠ 1) by omega) with hk | hk | hk
      · simp only [decOpt, hk, Option.some.injEq, Prod.mk.injEq] at hd
        obtain ⟨rfl, rfl⟩ := hd
        simp [encOpt, byte_eq_tag b 0 hk]
      · simp only [decOpt, hk] at hd
        cases hdy : d r with
        | none => simp [hdy] at hd
        | some xr =>
          obtain ⟨y, r'⟩ := xr
          simp only [hdy, Option.some.injEq, Prod.mk.injEq] at hd
          obtain ⟨rfl, rfl⟩ := hd
          have h2 := h.2 r y r' hdy
          simp only [encOpt]
          rw [byte_eq_tag b 1 hk, h2]
          simp
      · simp [decOpt, hk.1, hk.2] at hd

/-! ## Lists: the count, then the elements -/

def encList {α : Type} (e : α → Bytes) (xs : List α) : Bytes :=
  encNat xs.length ++ xs.flatMap e

/-- `k` elements, in order. -/
def decN {α : Type} (d : Dec α) : Nat → Bytes → Option (List α × Bytes)
  | 0, bs => some ([], bs)
  | k + 1, bs =>
    match d bs with
    | some (x, r) => match decN d k r with
      | some (xs, r') => some (x :: xs, r')
      | none => none
    | none => none

def decList {α : Type} (d : Dec α) : Dec (List α) := fun bs =>
  match decNat bs with
  | some (n, r) => decN d n r
  | none => none

theorem decN_flatMap {α : Type} {e : α → Bytes} {d : Dec α} (h : Lawful e d) :
    ∀ (xs : List α) (rest : Bytes), decN d xs.length (xs.flatMap e ++ rest) = some (xs, rest)
  | [], rest => rfl
  | x :: xs, rest => by
    have ih := decN_flatMap h xs rest
    rw [List.length_cons, List.flatMap_cons, List.append_assoc]
    rw [decN, h.1 x (xs.flatMap e ++ rest)]
    dsimp only
    rw [ih]

theorem decN_canon {α : Type} {e : α → Bytes} {d : Dec α} (h : Lawful e d) :
    ∀ (k : Nat) (bs : Bytes) (xs : List α) (rest : Bytes),
      decN d k bs = some (xs, rest) → xs.length = k ∧ bs = xs.flatMap e ++ rest
  | 0, bs, xs, rest, hk => by
    simp only [decN, Option.some.injEq, Prod.mk.injEq] at hk
    obtain ⟨rfl, rfl⟩ := hk
    simp
  | k + 1, bs, xs, rest, hk => by
    cases hdx : d bs with
    | none => simp [decN, hdx] at hk
    | some xr =>
      obtain ⟨x, r⟩ := xr
      cases hdk : decN d k r with
      | none => simp [decN, hdx, hdk] at hk
      | some xsr =>
        obtain ⟨xs', r'⟩ := xsr
        simp only [decN, hdx, hdk, Option.some.injEq, Prod.mk.injEq] at hk
        obtain ⟨rfl, rfl⟩ := hk
        obtain ⟨hl, hr⟩ := decN_canon h k r xs' r' hdk
        refine ⟨by simp [hl], ?_⟩
        rw [h.2 bs x r hdx, hr]
        simp [List.flatMap_cons, List.append_assoc]

theorem lawful_list {α : Type} {e : α → Bytes} {d : Dec α} (h : Lawful e d) :
    Lawful (encList e) (decList d) := by
  refine ⟨?_, ?_⟩
  · intro xs rest
    unfold encList decList
    rw [List.append_assoc, decNat_encNat]
    dsimp only
    exact decN_flatMap h xs rest
  · intro bs xs rest hd
    unfold decList at hd
    cases hdn : decNat bs with
    | none => simp [hdn] at hd
    | some nr =>
      obtain ⟨n, r⟩ := nr
      simp only [hdn] at hd
      obtain ⟨hl, hr⟩ := decN_canon h n r xs rest hd
      have hbs := decNat_canon bs n r hdn
      unfold encList
      rw [hbs, hr, ← hl, List.append_assoc]

end Strata.Codec
