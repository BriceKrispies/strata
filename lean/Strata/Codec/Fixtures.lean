/-
The canonical byte codec, part 3: byte fixtures and the boundary summary.

Each fixture states the encoding of one value as the list of bytes derived by hand from the format
(`Strata.Codec.Core`, `Strata.Codec.Boundary`), and proves that the Lean encoder produces it. The
Rust codec (`crates/lean-kernel-ffi/src/codec.rs`) has the same values and the same bytes as its
test fixtures, so the two encoders are checked against one list of bytes.

`encNat` is defined by well-founded recursion, so it does not reduce by `decide`; the two rewrite
lemmas below unfold it at a literal, and the fixtures are then closed by evaluation.
-/
import Strata.Codec.Boundary

namespace Strata.Codec

open Strata

/-- A natural below 128 is one byte. -/
theorem encNat_small {n : Nat} (h : n < 128) : encNat n = [tag n] := by
  rw [encNat, dite_eq_left_iff.mpr fun h' => absurd h h']

/-- A natural from 128 is its low group with the high bit set, then the rest. -/
theorem encNat_big {n : Nat} (h : 128 ≤ n) :
    encNat n = tag (n % 128 + 128) :: encNat (n / 128) := by
  rw [encNat]
  simp [show ¬ n < 128 by omega]

/-- The fixture tenant: one string field, owner 7, next id 1, the kernel's limits. -/
def fxTenant : Tenant :=
  { types := [(1, { schema := [(1, { ty := .str, required := true, unique := false, default := .lit .null })] })],
    members := [(7, [ownerRole])], grants := [], policies := [], nextId := 1,
    limits := ⟨65536, 1000, 200, 262144⟩ }

/-! ## Values and types -/

theorem fix_val_null : encVal .null = [0] := by
  simp +decide only [encVal, tag]

theorem fix_val_int_neg : encVal (.int (-1)) = [1, 1, 0] := by
  simp +decide only [encVal, encInt, encNat_small, encNat_big, tag, Int.reduceNeg, Int.reduceSub,
    Int.reduceToNat, Int.reduceLE, ite_true, ite_false, List.cons_append, List.nil_append,
    Nat.reduceMod, Nat.reduceAdd, Nat.reduceDiv]

theorem fix_val_str : encVal (.str "ab") = [2, 2, 97, 98] := by
  simp +decide only [encVal, encStr, encNat_small, encNat_big, tag, List.cons_append,
    List.nil_append, String.toUTF8_eq_toByteArray, String.size_toByteArray]

theorem fix_val_ref : encVal (.ref 300) = [4, 172, 2] := by
  simp +decide only [encVal, encNat_small, encNat_big, tag, List.cons_append, List.nil_append]

theorem fix_val_list : encVal (.list [.int 5, .null]) = [5, 2, 1, 0, 5, 0] := by
  simp +decide only [encVal, encList, encElem, encInt, encNat_small, encNat_big, tag,
    List.cons_append, List.nil_append, List.length_cons, List.length_nil, List.flatMap_cons,
    List.flatMap_nil]

theorem fix_ty_list_ref : encTy (.list (.ref 0 .setNull)) = [5, 4, 0] := by
  simp +decide only [encTy, encNat_small, tag]

theorem fix_ty_ref : encTy (.ref 300 .restrict) = [3, 172, 2] := by
  simp +decide only [encTy, encNat_small, encNat_big, tag, List.cons_append, List.nil_append]

/-! ## Schema, limits, records, tenant -/

theorem fix_fieldDef : encFieldDef ⟨.int, false, true, .lit (.int (-2))⟩ = [0, 0, 1, 0, 1, 1, 1] := by
  simp +decide only [encFieldDef, encIso, encPair, encTy, encBool, encExpr, encVal, encInt, encNat_small,
    encNat_big, tag, Int.reduceNeg, Int.reduceSub, Int.reduceToNat, Int.reduceLE, ite_true,
    ite_false, List.cons_append, List.nil_append, Nat.reduceMod, Nat.reduceAdd, Nat.reduceDiv]

theorem fix_limits : encLimits ⟨65536, 1000, 200, 262144⟩ =
    [128, 128, 4, 232, 7, 200, 1, 128, 128, 16] := by
  simp +decide only [encLimits, encIso, encPair, encNat_small, encNat_big, tag,
    List.cons_append, List.nil_append, Nat.reduceMod, Nat.reduceAdd, Nat.reduceDiv,
    Nat.reduceLT]

theorem fix_record : encRecord { owner := some 4, version := 1, fields := [(2, .bool true)] } =
    [1, 4, 1, 1, 2, 3, 1] := by
  simp +decide only [encRecord, encIso, encPair, encOpt, encList, encFieldVal, encVal, encBool, encNat_small,
    encNat_big, tag, List.length_cons, List.length_nil, List.flatMap_cons, List.flatMap_nil,
    List.cons_append, List.nil_append, Nat.reduceAdd]

/-- The fixture tenant has one object type, `1`: its schema is one string field, no constraints and
    no records. The tenant has owner 7 as its one member, holding the owner role, next id 1 and the kernel's
    limits. Its bytes are the type list (one entry: id `1`, then schema (one field: id `1`, string,
    required, not unique, null default), no uniques, no records, no constraints), then the members
    (one: user 7 with role 0), the grants (none), the policies (none), next id, limits, then (slice 7)
    the roles (two: `owner` and `public`), the teams (none) and the invitations (none). -/
theorem fix_tenant : encTenant fxTenant =
    [1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 1, 7, 1, 0, 0, 0, 1, 128, 128, 4, 232, 7, 200, 1, 128, 128, 16,
      2, 0, 1, 0, 0] := by
  simp +decide only [fxTenant, encTenant, encTenantBody, encIso, encPair, encList, encTypeEntry,
    encMember, encGrantEntry, encPolicy, encOpt, encTeamEntry, encInvitationEntry, ownerRole, publicRole,
    encObjType, encSchemaEntry, encFieldDef, encTy, encBool, encExpr, encVal, encLimits, encNat_small,
    encNat_big, tag, List.length_cons, List.length_nil, List.flatMap_cons, List.flatMap_nil,
    List.cons_append, List.nil_append, Nat.reduceMod, Nat.reduceAdd, Nat.reduceDiv, Nat.reduceLT]

/-- A policy naming the tenant's membership: role 1, the `members` resource (tag 3), one action
    (manage, tag 4), no condition. -/
theorem fix_policy_members :
    encPolicy ⟨1, .members, [.manage], none⟩ = [1, 3, 1, 4, 0] := by
  simp +decide only [encPolicy, encIso, encPair, encOpt, encList, encResource, encAction, encNat_small,
    tag, List.length_cons, List.length_nil, List.flatMap_cons, List.flatMap_nil, List.cons_append,
    List.nil_append]

/-- The fixture tenant with a role `2`, a team and an invitation (slice 7): the roles `[0, 1, 2]`, team
    `5` labelled `ab` with member 7 holding role 2, and invitation `9` to `a@b` for role 2, issued by 7,
    expiring at 300 and revoked by 7. -/
def fxTenantM : Tenant :=
  { fxTenant with
    roles := [ownerRole, publicRole, 2]
    teams := [(5, { label := "ab", members := [7], roles := [2] })]
    invitations := [(9, { address := "a@b", roles := [2], createdBy := 7, expiresAt := 300,
                          redeemedBy := none, revokedBy := some 7 })] }

/-- Its bytes are those of `fxTenant` up to the limits, then the three roles, one team (id 5, label
    `ab`, members `[7]`, roles `[2]`) and one invitation (id 9, address `a@b`, roles `[2]`, issuer 7,
    expiry 300 as two bytes, no redeemer, revoker 7). -/
theorem fix_tenant_members : encTenant fxTenantM =
    [1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 1, 7, 1, 0, 0, 0, 1, 128, 128, 4, 232, 7, 200, 1, 128, 128, 16,
      3, 0, 1, 2, 1, 5, 2, 97, 98, 1, 7, 1, 2, 1, 9, 3, 97, 64, 98, 1, 2, 7, 172, 2, 0, 1, 7] := by
  simp +decide only [fxTenantM, fxTenant, encTenant, encTenantBody, encIso, encPair, encList, encTypeEntry,
    encMember, encGrantEntry, encPolicy, encOpt, encTeamEntry, encInvitationEntry, encTeamDef,
    encInvitationDef, encStr, ownerRole, publicRole,
    encObjType, encSchemaEntry, encFieldDef, encTy, encBool, encExpr, encVal, encLimits, encNat_small,
    encNat_big, tag, List.length_cons, List.length_nil, List.flatMap_cons, List.flatMap_nil,
    List.cons_append, List.nil_append, Nat.reduceMod, Nat.reduceAdd, Nat.reduceDiv, Nat.reduceLT,
    String.toUTF8_eq_toByteArray, String.size_toByteArray]

/-! ## Intents, the context, errors and the result -/

theorem fix_intent_createTenant : encIntent (.createTenant 7 "ab") = [6, 7, 2, 97, 98] := by
  simp +decide only [encIntent, encStr, encNat_small, tag, List.cons_append, List.nil_append,
    String.toUTF8_eq_toByteArray, String.size_toByteArray]

theorem fix_intent_assignRole : encIntent (.member (.assignRole 9 2)) = [7, 0, 9, 2] := by
  simp +decide only [encIntent, encMemberIntent, encPair, encNat_small, tag, List.cons_append,
    List.nil_append]

theorem fix_intent_defineTeam : encIntent (.member (.defineTeam 5 "ab")) = [7, 2, 5, 2, 97, 98] := by
  simp +decide only [encIntent, encMemberIntent, encPair, encStr, encNat_small, tag, List.cons_append,
    List.nil_append, String.toUTF8_eq_toByteArray, String.size_toByteArray]

theorem fix_intent_createInvitation :
    encIntent (.member (.createInvitation 3 "a@b" [2] 300)) =
      [7, 7, 3, 3, 97, 64, 98, 1, 2, 172, 2] := by
  simp +decide only [encIntent, encMemberIntent, encPair, encStr, encList, encNat_small, encNat_big,
    tag, List.length_cons, List.length_nil, List.flatMap_cons, List.flatMap_nil, List.cons_append,
    List.nil_append, Nat.reduceMod, Nat.reduceAdd, Nat.reduceDiv, Nat.reduceLT,
    String.toUTF8_eq_toByteArray, String.size_toByteArray]

theorem fix_intent_revokeInvitation : encIntent (.member (.revokeInvitation 3)) = [7, 8, 3] := by
  simp +decide only [encIntent, encMemberIntent, encNat_small, tag, List.cons_append, List.nil_append]

theorem fix_intent_redeemInvitation :
    encIntent (.member (.redeemInvitation 3 300 (some "a@b"))) =
      [7, 9, 3, 172, 2, 1, 3, 97, 64, 98] := by
  simp +decide only [encIntent, encMemberIntent, encPair, encStr, encOpt, encNat_small, encNat_big,
    tag, List.cons_append, List.nil_append, Nat.reduceMod, Nat.reduceAdd, Nat.reduceDiv,
    Nat.reduceLT, String.toUTF8_eq_toByteArray, String.size_toByteArray]

theorem fix_intent_redeemInvitation_unverified :
    encIntent (.member (.redeemInvitation 3 300 none)) = [7, 9, 3, 172, 2, 0] := by
  simp +decide only [encIntent, encMemberIntent, encPair, encOpt, encNat_small, encNat_big, tag,
    List.cons_append, List.nil_append, Nat.reduceMod, Nat.reduceAdd, Nat.reduceDiv, Nat.reduceLT]

theorem fix_err_invalidIntent : encErr .invalidIntent = [14] := by
  simp +decide only [encErr, tag]

theorem fix_err_unknownRole : encErr .unknownRole = [15] := by
  simp +decide only [encErr, tag]

theorem fix_err_lastOwner : encErr .lastOwner = [16] := by
  simp +decide only [encErr, tag]

theorem fix_err_tenantExists : encErr .tenantExists = [17] := by
  simp +decide only [encErr, tag]

theorem fix_intent_setRequired : encIntent (.setRequired 1 2 false) = [3, 1, 2, 0] := by
  simp +decide only [encIntent, encNat_small, encBool, tag, List.cons_append, List.nil_append]

theorem fix_intent_update :
    encIntent (.update 1 5 [(1, .str "x")] none) = [1, 1, 5, 1, 1, 2, 1, 120, 0] := by
  simp +decide only [encIntent, encList, encFieldVal, encPair, encVal, encStr, encOpt, encNat_small,
    encNat_big, tag, List.length_cons, List.length_nil, List.flatMap_cons, List.flatMap_nil,
    List.cons_append, List.nil_append, String.toUTF8_eq_toByteArray, String.size_toByteArray]

theorem fix_intent_create : encIntent (.create 1 []) = [0, 1, 0] := by
  simp +decide only [encIntent, encList, encNat_small, tag, List.length_nil, List.flatMap_nil,
    List.nil_append, List.cons_append]

theorem fix_ctx : encCtx ⟨3, .user 9⟩ = [3, 0, 9] := by
  simp +decide only [encCtx, encIso, encPair, encPrincipal, encNat_small, tag, List.cons_append,
    List.nil_append]

theorem fix_err : encErr .limitExceeded = [8] := by
  simp +decide only [encErr, tag]

theorem fix_err_unknownObject : encErr .unknownObject = [11] := by
  simp +decide only [encErr, tag]

theorem fix_err_constraintViolated : encErr .constraintViolated = [12] := by
  simp +decide only [encErr, tag]

theorem fix_err_evaluation : encErr .evaluation = [13] := by
  simp +decide only [encErr, tag]

theorem fix_result_error : encResult (.error .unauthorized) = [0, 1] := by
  simp +decide only [encResult, encErr, tag, List.cons_append, List.nil_append]

theorem fix_result_ok : encResult (.ok fxTenant) =
    [1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 1, 7, 1, 0, 0, 0, 1, 128, 128, 4, 232, 7, 200, 1, 128, 128, 16,
      2, 0, 1, 0, 0] := by
  simp +decide only [encResult, fix_tenant, tag, List.cons_append]

/-! ## Decoding the fixtures

The decoders reduce by `decide`, since none of them is well-founded. -/

theorem fix_decode_ref : decodeBytes decVal ⟨#[4, 172, 2]⟩ = some (.ref 300) := by
  decide

/-! ## Summary: every boundary type round-trips and decodes only canonical bytes -/

/-- For each type that crosses the boundary: decoding an encoding returns the value, and any bytes
    that decode are the encoding of the decoded value. -/
theorem boundary_codecs :
    (∀ x, decodeBytes decTenant (encodeBytes encTenant x) = some x) ∧
    (∀ b x, decodeBytes decTenant b = some x → encodeBytes encTenant x = b) ∧
    (∀ x, decodeBytes decVal (encodeBytes encVal x) = some x) ∧
    (∀ b x, decodeBytes decVal b = some x → encodeBytes encVal x = b) ∧
    (∀ x, decodeBytes decElem (encodeBytes encElem x) = some x) ∧
    (∀ b x, decodeBytes decElem b = some x → encodeBytes encElem x = b) ∧
    (∀ x, decodeBytes decTy (encodeBytes encTy x) = some x) ∧
    (∀ b x, decodeBytes decTy b = some x → encodeBytes encTy x = b) ∧
    (∀ x, decodeBytes decTeamDef (encodeBytes encTeamDef x) = some x) ∧
    (∀ b x, decodeBytes decTeamDef b = some x → encodeBytes encTeamDef x = b) ∧
    (∀ x, decodeBytes decInvitationDef (encodeBytes encInvitationDef x) = some x) ∧
    (∀ b x, decodeBytes decInvitationDef b = some x → encodeBytes encInvitationDef x = b) ∧
    (∀ x, decodeBytes decMemberIntent (encodeBytes encMemberIntent x) = some x) ∧
    (∀ b x, decodeBytes decMemberIntent b = some x → encodeBytes encMemberIntent x = b) ∧
    (∀ x, decodeBytes decFieldDef (encodeBytes encFieldDef x) = some x) ∧
    (∀ b x, decodeBytes decFieldDef b = some x → encodeBytes encFieldDef x = b) ∧
    (∀ x, decodeBytes decLimits (encodeBytes encLimits x) = some x) ∧
    (∀ b x, decodeBytes decLimits b = some x → encodeBytes encLimits x = b) ∧
    (∀ x, decodeBytes decRecord (encodeBytes encRecord x) = some x) ∧
    (∀ b x, decodeBytes decRecord b = some x → encodeBytes encRecord x = b) ∧
    (∀ x, decodeBytes decIntent (encodeBytes encIntent x) = some x) ∧
    (∀ b x, decodeBytes decIntent b = some x → encodeBytes encIntent x = b) ∧
    (∀ x, decodeBytes decCtx (encodeBytes encCtx x) = some x) ∧
    (∀ b x, decodeBytes decCtx b = some x → encodeBytes encCtx x = b) ∧
    (∀ x, decodeBytes decErr (encodeBytes encErr x) = some x) ∧
    (∀ b x, decodeBytes decErr b = some x → encodeBytes encErr x = b) ∧
    (∀ x, decodeBytes decResult (encodeBytes encResult x) = some x) ∧
    (∀ b x, decodeBytes decResult b = some x → encodeBytes encResult x = b) ∧
    (∀ x, decodeBytes decOnDelete (encodeBytes encOnDelete x) = some x) ∧
    (∀ b x, decodeBytes decOnDelete b = some x → encodeBytes encOnDelete x = b) :=
  ⟨fun x => decodeBytes_encodeBytes lawful_tenant x,
    fun b x h => encodeBytes_of_decodeBytes lawful_tenant b x h,
    fun x => decodeBytes_encodeBytes lawful_val x,
    fun b x h => encodeBytes_of_decodeBytes lawful_val b x h,
    fun x => decodeBytes_encodeBytes lawful_elem x,
    fun b x h => encodeBytes_of_decodeBytes lawful_elem b x h,
    fun x => decodeBytes_encodeBytes lawful_ty x,
    fun b x h => encodeBytes_of_decodeBytes lawful_ty b x h,
    fun x => decodeBytes_encodeBytes lawful_teamDef x,
    fun b x h => encodeBytes_of_decodeBytes lawful_teamDef b x h,
    fun x => decodeBytes_encodeBytes lawful_invitationDef x,
    fun b x h => encodeBytes_of_decodeBytes lawful_invitationDef b x h,
    fun x => decodeBytes_encodeBytes lawful_memberIntent x,
    fun b x h => encodeBytes_of_decodeBytes lawful_memberIntent b x h,
    fun x => decodeBytes_encodeBytes lawful_fieldDef x,
    fun b x h => encodeBytes_of_decodeBytes lawful_fieldDef b x h,
    fun x => decodeBytes_encodeBytes lawful_limits x,
    fun b x h => encodeBytes_of_decodeBytes lawful_limits b x h,
    fun x => decodeBytes_encodeBytes lawful_record x,
    fun b x h => encodeBytes_of_decodeBytes lawful_record b x h,
    fun x => decodeBytes_encodeBytes lawful_intent x,
    fun b x h => encodeBytes_of_decodeBytes lawful_intent b x h,
    fun x => decodeBytes_encodeBytes lawful_ctx x,
    fun b x h => encodeBytes_of_decodeBytes lawful_ctx b x h,
    fun x => decodeBytes_encodeBytes lawful_err x,
    fun b x h => encodeBytes_of_decodeBytes lawful_err b x h,
    fun x => decodeBytes_encodeBytes lawful_result x,
    fun b x h => encodeBytes_of_decodeBytes lawful_result b x h,
    fun x => decodeBytes_encodeBytes lawful_onDelete x,
    fun b x h => encodeBytes_of_decodeBytes lawful_onDelete b x h⟩

end Strata.Codec
