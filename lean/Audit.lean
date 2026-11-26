/-
Prints, for each theorem the project claims, the axioms its proof depends on.
Run with `lake env lean Audit.lean`. A theorem proved with `sorry` would list `sorryAx`.
-/
import Strata

#print axioms Strata.step_preserves_wf
#print axioms Strata.stepTenant_preserves_wf
#print axioms Strata.unauthorized_cannot_mutate
#print axioms Strata.unauthorized_cannot_update
#print axioms Strata.unauthorized_cannot_delete
#print axioms Strata.missing_record_hidden_on_update
#print axioms Strata.missing_record_hidden_on_delete
#print axioms Strata.step_ok_authorized
#print axioms Strata.step_isolated
#print axioms Strata.step_local
#print axioms Strata.step_deterministic
#print axioms Strata.lax_change_can_break_wf
#print axioms Strata.problem_none
#print axioms Strata.stepTenant_preserves_refs
#print axioms Strata.step_preserves_refs
#print axioms Strata.delete_refused_while_referenced
#print axioms Strata.delete_leaves_no_reference
#print axioms Strata.conforms_afterDelete
#print axioms Strata.problem_none_parts
#print axioms Strata.patch_keeps_unnamed
#print axioms Strata.patch_last_wins
#print axioms Strata.update_writes_patched
#print axioms Strata.update_keeps_unnamed
#print axioms Strata.update_raises_version
#print axioms Strata.delete_raises_referrer_version
#print axioms Strata.version_conflict_on_update
#print axioms Strata.version_conflict_on_delete
#print axioms Strata.create_fills_default
#print axioms Strata.required_list_never_blocks
#print axioms Strata.problem_none_within
#print axioms Strata.create_refused_over_record_bound
#print axioms Strata.create_within_limits
#print axioms Strata.update_within_limits

-- Slice 3: uniqueness
#print axioms Strata.valueProblem_conds
#print axioms Strata.valueProblem_uniq_dichotomy
#print axioms Strata.valueProblem_clash
#print axioms Strata.findSome_dichotomy
#print axioms Strata.findSome_hit
#print axioms Strata.fieldProblem_uniq_dichotomy
#print axioms Strata.fieldProblem_clash
#print axioms Strata.problem_unique_none
#print axioms Strata.problem_uniq_dichotomy
#print axioms Strata.problem_uniq_refusal
#print axioms Strata.agreeOn_comm
#print axioms Strata.dupFields_false
#print axioms Strata.dropRef_getD_keyed
#print axioms Strata.get_cons_self
#print axioms Strata.get_cons_ne
#print axioms Strata.get_clearFields_eq
#print axioms Strata.clearRef_keyed
#print axioms Strata.agreeOn_clearRef
#print axioms Strata.nodup_filterMap_fst
#print axioms Strata.mem_set_keys
#print axioms Strata.set_nodup
#print axioms Strata.patch_nodup
#print axioms Strata.normalise_keys_sub
#print axioms Strata.normalise_nodup
#print axioms Strata.newRecord_nodup
#print axioms Strata.clearRef_nodup
#print axioms Strata.keys_makeRequired
#print axioms Strata.keys_makeUnique
#print axioms Strata.mem_makeRequired
#print axioms Strata.mem_makeUnique
#print axioms Strata.mem_replace
#print axioms Strata.mem_afterDelete
#print axioms Strata.stepTenant_preserves_distinct
#print axioms Strata.stepTenant_preserves_unique
#print axioms Strata.create_duplicate_refused
#print axioms Strata.update_duplicate_refused
#print axioms Strata.define_unique_refused_over_duplicates
#print axioms Strata.set_unique_refused_over_duplicates
#print axioms Strata.lax_unique_change_can_break_wf
#print axioms Strata.strict_schema_change_conforms
#print axioms Strata.schemaViolated_false
#print axioms Strata.setRequired_spec
#print axioms Strata.defineUnique_spec
#print axioms Strata.lookup_map_ty
#print axioms Strata.lookup_makeUnique_ty
#print axioms Strata.fieldOk_makeUnique
#print axioms Strata.conforms_makeUnique
#print axioms Strata.setUnique_records
#print axioms Strata.defineUnique_records
#print axioms Strata.stepTenant_conform_ids
#print axioms Strata.step_preserves_unique
#print axioms Strata.setUnique_spec

#print axioms Strata.Evolution.closure
#print axioms Strata.Evolution.locality
#print axioms Strata.Evolution.compatibility
#print axioms Strata.Evolution.breaking_change_can_change_value
#print axioms Strata.Evolution.breaking_change_can_invalidate
#print axioms Strata.Evolution.rewrite_correct
#print axioms Strata.Evolution.rewrite_correct_nb
#print axioms Strata.Evolution.project_migrate
#print axioms Strata.Evolution.left_inverse_on_footprint
#print axioms Strata.Evolution.pinned_eval
#print axioms Strata.Evolution.pinned_eval_live
#print axioms Strata.Evolution.removeField_irreversible
#print axioms Strata.Evolution.retargetRef_irreversible
#print axioms Strata.Evolution.removeObject_irreversible
#print axioms Strata.Evolution.narrow_nat_irreversible
#print axioms Strata.Evolution.commutation
#print axioms Strata.Evolution.rewrite_compose
#print axioms Strata.Evolution.rewrite_chain
#print axioms Strata.Evolution.pinned_breaking_change_accepted

#print axioms Strata.WorkingSet.problem_eq_via
#print axioms Strata.WorkingSet.decideTenant_faithful
#print axioms Strata.WorkingSet.decideOn_fetch
#print axioms Strata.WorkingSet.decideOn_congr
#print axioms Strata.WorkingSet.touchedIn_sound
#print axioms Strata.WorkingSet.hit_stamp
#print axioms Strata.WorkingSet.serve_eq_fetch
#print axioms Strata.WorkingSet.sync_safe_self
#print axioms Strata.WorkingSet.readWS_value
#print axioms Strata.WorkingSet.checkedRead_linearizes
#print axioms Strata.WorkingSet.decideWS_refusal
#print axioms Strata.WorkingSet.decideWS_pendingOk
#print axioms Strata.WorkingSet.submitWS_world
#print axioms Strata.WorkingSet.submitWS_inv
#print axioms Strata.WorkingSet.stepWS_inv
#print axioms Strata.WorkingSet.stepWS_safe
#print axioms Strata.WorkingSet.stepWS_faithful
#print axioms Strata.WorkingSet.stepWS_within
#print axioms Strata.WorkingSet.stepWS_pins
#print axioms Strata.WorkingSet.runWS_faithful
#print axioms Strata.WorkingSet.runWS_reads
#print axioms Strata.WorkingSet.runWS_within
#print axioms Strata.WorkingSet.stepWS_reqOk
#print axioms Strata.WorkingSet.request_snapshot
#print axioms Strata.WorkingSet.runWS_reqOk
#print axioms Strata.WorkingSet.unfenced_breaks_reads
#print axioms Strata.WorkingSet.w4_violation_breaks_checked_read
#print axioms Strata.WorkingSet.admit_keeps_pinned
#print axioms Strata.WorkingSet.evict_keeps_pinned
#print axioms Strata.WorkingSet.resize_keeps_pinned
#print axioms Strata.WorkingSet.pin_resident
#print axioms Strata.WorkingSet.resize_budget_exact
#print axioms Strata.WorkingSet.resize_budget_le
#print axioms Strata.WorkingSet.machine_resize
#print axioms Strata.WorkingSet.below_sealing_erasure_safe
#print axioms Strata.WorkingSet.above_sealing_serves_erased
#print axioms Strata.WorkingSet.submit_one_generation
#print axioms Strata.WorkingSet.decideReq_refusal
#print axioms Strata.WorkingSet.decideReq_expected
#print axioms Strata.WorkingSet.reqGet_eq_fetch
#print axioms Strata.WorkingSet.decideReqWS_pendingOk
#print axioms Strata.WorkingSet.stepWS_invR
#print axioms Strata.WorkingSet.conflict_refreshes
#print axioms Strata.WorkingSet.quiet_run
#print axioms Strata.WorkingSet.resubmit_no_conflict
#print axioms Strata.WorkingSet.Leases.write_refused_unless_holder
#print axioms Strata.WorkingSet.Leases.acquire_refused_while_serving
#print axioms Strata.WorkingSet.Leases.cache_read_before_term
#print axioms Strata.WorkingSet.Leases.read_correct
#print axioms Strata.WorkingSet.Leases.stepL_inv
#print axioms Strata.WorkingSet.Leases.leased_reads_correct
#print axioms Strata.WorkingSet.Leases.linv_init
#print axioms Strata.WorkingSet.Leases.raw_breaks_leased_read
#print axioms Strata.WorkingSet.Arbiter.arbiter_alarm
#print axioms Strata.WorkingSet.Arbiter.arbiter_within
#print axioms Strata.WorkingSet.Arbiter.arbiter_floors
#print axioms Strata.WorkingSet.Arbiter.arbiter_length
#print axioms Strata.WorkingSet.Arbiter.arbiter_overplaced_le_floors
#print axioms Strata.WorkingSet.bytes_fitTo_le
#print axioms Strata.WorkingSet.resizeHard_within
#print axioms Strata.WorkingSet.resizeHard_eq_resize
#print axioms Strata.WorkingSet.machine_hard
#print axioms Strata.WorkingSet.machine_arbiter

#print axioms Strata.Client.Cache.run_base
#print axioms Strata.Client.Cache.run_coh
#print axioms Strata.Client.Cache.current_recent
#print axioms Strata.Client.Cache.current_value
#print axioms Strata.Client.Cache.current_answer
#print axioms Strata.Client.Cache.read_stale_older
#print axioms Strata.Client.Cache.rest_one_epoch
#print axioms Strata.Client.Cache.rest_above_floor
#print axioms Strata.Client.Cache.rest_same_tenant_epoch
#print axioms Strata.Client.Cache.contact_floor
#print axioms Strata.Client.Cache.tenant_epoch_purges
#print axioms Strata.Client.Cache.older_epoch_ignored
#print axioms Strata.Client.Cache.contact_reaches_current_epoch
#print axioms Strata.Client.Cache.out_of_order_epochs_recover
#print axioms Strata.Client.Cache.later_request_is_authoritative
#print axioms Strata.Client.Cache.lower_ordinal_recovers
#print axioms Strata.Client.Cache.restarted_counter_dropped_without_reset
#print axioms Strata.Client.Cache.reset_accepts
#print axioms Strata.Client.Cache.hear_purges
#print axioms Strata.Client.Cache.hear_access_purges
#print axioms Strata.Client.Cache.receipt_read
#print axioms Strata.Client.Cache.read_after_commit
#print axioms Strata.Client.Cache.incomplete_notice_serves_stale
#print axioms Strata.Client.Cache.fresh_contact_still_stale
#print axioms Strata.Client.Cache.stamp_after_open_serves_erased
#print axioms Strata.Client.Cache.unguarded_put_resurrects
#print axioms Strata.Client.Cache.guarded_put_drops
#print axioms Strata.Client.Cache.unfloored_put_resurrects
#print axioms Strata.Client.Cache.floored_put_drops
#print axioms Strata.Client.Cache.current_fetch
#print axioms Strata.Client.Cache.browser_generation_only_serves_erased
#print axioms Strata.Client.Privacy.reachable_inv
#print axioms Strata.Client.Privacy.no_cross_principal
#print axioms Strata.Client.Privacy.served_own
#print axioms Strata.Client.Privacy.signout_erases
#print axioms Strata.Client.Privacy.offline_signout_erases
#print axioms Strata.Client.Privacy.expire_locks
#print axioms Strata.Client.Privacy.unlock_needs_own_session
#print axioms Strata.Client.Privacy.expire_then_same_principal_unlocks
#print axioms Strata.Client.Privacy.revoke_device_erases
#print axioms Strata.Client.Privacy.revoke_all_erases
#print axioms Strata.Client.Privacy.offline_key_survives_reload
#print axioms Strata.Client.Privacy.rot_no_loss
#print axioms Strata.Client.Privacy.rot_no_lost_update
#print axioms Strata.Client.Privacy.rot_finish_retires
#print axioms Strata.Client.Privacy.rot_stale_append_breaks
#print axioms Strata.Client.Privacy.rot_stale_append_refused
#print axioms Strata.Client.Privacy.rot_blind_write_loses_update
#print axioms Strata.Client.Privacy.rot_cas_keeps_update
#print axioms Strata.Client.Queue.submit_ok_guard
#print axioms Strata.Client.Queue.submit_not_ok_unchanged
#print axioms Strata.Client.Queue.submit_classes
#print axioms Strata.Client.Queue.at_most_once
#print axioms Strata.Client.Queue.stamped_at_most_once
#print axioms Strata.Client.Queue.q2_sent
#print axioms Strata.Client.Queue.fence_holds
#print axioms Strata.Client.Queue.unfenced_lookup_applies_twice
#print axioms Strata.Client.Queue.fenced_lookup_applies_once
#print axioms Strata.Client.Queue.create_twice_after_forget
#print axioms Strata.Client.Queue.rekeyed_create_applies_twice
#print axioms Strata.Client.Queue.restore_without_epoch_applies_twice
#print axioms Strata.Client.Queue.restore_with_epoch_applies_once
#print axioms Strata.Client.Drafts.orphan_drops_server_values
#print axioms Strata.Client.Drafts.survives_orphaning_erasure
#print axioms Strata.Client.Drafts.gone_after_next_erasure
#print axioms Strata.Client.Drafts.gone_after_idle
#print axioms Strata.Client.Drafts.kept_until
#print axioms Strata.UI.Composition.check_iff
#print axioms Strata.UI.Composition.checked_bindings_resolve
#print axioms Strata.UI.Composition.publishable_no_placeholder
#print axioms Strata.UI.Composition.publishable_bound
#print axioms Strata.UI.Composition.bindAt_valid
#print axioms Strata.UI.Composition.bindAt_placeholders
#print axioms Strata.UI.Composition.refine_valid
#print axioms Strata.UI.Composition.refine_publishable
#print axioms Strata.UI.Composition.incremental_valid
#print axioms Strata.UI.Composition.unguarded_removal_breaks
#print axioms Strata.UI.Composition.orphan_valid
#print axioms Strata.UI.Composition.orphan_removes_only_failing
#print axioms Strata.UI.Composition.orphan_keeps_valid
#print axioms Strata.UI.Contrast.onColour_meets_aa
#print axioms Strata.UI.Contrast.search_finds
#print axioms Strata.UI.Contrast.checkTheme_iff
#print axioms Strata.UI.Visibility.render_sound
#print axioms Strata.UI.Visibility.served_has_element
#print axioms Strata.UI.Visibility.served_needs_content
#print axioms Strata.UI.Visibility.hidden_leaves_no_trace
#print axioms Strata.UI.Visibility.hidden_leaves_no_trace_in
#print axioms Strata.UI.Visibility.admin_needs_role
#print axioms Strata.UI.Visibility.admin_never_to_default_member
#print axioms Strata.UI.Visibility.admin_action_for_members_refused
#print axioms Strata.UI.Visibility.admin_default_role_refused
#print axioms Strata.UI.Visibility.select_sees
#print axioms Strata.UI.Visibility.select_first
#print axioms Strata.UI.Visibility.selectServed_sound
#print axioms Strata.UI.Visibility.selectServed_first
#print axioms Strata.UI.Visibility.variant_falls_through
#print axioms Strata.UI.Visibility.audOf_sees
#print axioms Strata.UI.Visibility.dataAud_sees
#print axioms Strata.UI.Visibility.public_grant_anon_sees
#print axioms Strata.UI.Visibility.contentAud_sub
#print axioms Strata.UI.Visibility.checked_screen_has_content
#print axioms Strata.UI.Visibility.empty_region_pruned
#print axioms Strata.UI.Visibility.static_does_not_keep_region
#print axioms Strata.UI.Visibility.narrow_class_eq_render
#print axioms Strata.UI.Visibility.renderClass_shared
#print axioms Strata.UI.Visibility.signed_in_decides
#print axioms Strata.UI.Loop.draft_steps_frame
#print axioms Strata.UI.Loop.snapshot_readable
#print axioms Strata.UI.Loop.snapshot_ignores_unreadable
#print axioms Strata.UI.Loop.preview_depends_on_readable_only
#print axioms Strata.UI.Loop.preview_reads_within_viewer
#print axioms Strata.UI.Loop.followed_within_viewer
#print axioms Strata.UI.Loop.no_stale_draft_write
#print axioms Strata.UI.Loop.revisions_increase
#print axioms Strata.UI.Loop.approve_applies_held
#print axioms Strata.UI.Loop.approve_class_matches
#print axioms Strata.UI.Loop.approve_refused_when_moved
#print axioms Strata.UI.Loop.approve_refused_on_hash_mismatch
#print axioms Strata.UI.Loop.staged_invisible_until_last
#print axioms Strata.UI.Visibility.selectNarrowed_eq_selectServed
#print axioms Strata.UI.Loop.hidden_reference_not_followed
#print axioms Strata.UI.Loop.early_change_to_reachable_visible

#print axioms Strata.Ledger.ledger_conserves_units
#print axioms Strata.Ledger.ledger_append_only
#print axioms Strata.Ledger.ledger_replay_idempotent
#print axioms Strata.Ledger.ledger_no_overdraft

-- Temporal logic library (Strata.Temporal)
#print axioms Strata.Temporal.suffix_zero
#print axioms Strata.Temporal.suffix_suffix
#print axioms Strata.Temporal.lift_suffix
#print axioms Strata.Temporal.act_suffix
#print axioms Strata.Temporal.eventually_of
#print axioms Strata.Temporal.always_elim
#print axioms Strata.Temporal.always_always
#print axioms Strata.Temporal.always_mono
#print axioms Strata.Temporal.always_imp
#print axioms Strata.Temporal.always_conj
#print axioms Strata.Temporal.eventually_disj
#print axioms Strata.Temporal.eventually_eventually
#print axioms Strata.Temporal.not_always_iff
#print axioms Strata.Temporal.not_eventually_iff
#print axioms Strata.Temporal.strong_implies_weak
#print axioms Strata.Temporal.leadsTo_refl
#print axioms Strata.Temporal.leadsTo_trans
#print axioms Strata.Temporal.leadsTo_disj
#print axioms Strata.Temporal.leadsTo_of_always_imp
#print axioms Strata.Temporal.leadsTo_apply
#print axioms Strata.Temporal.leadsTo_wf
#print axioms Strata.Temporal.invariant_of
#print axioms Strata.Temporal.invariant_of_spec
#print axioms Strata.Temporal.wf1
#print axioms Strata.Temporal.always_enabled_leadsTo
#print axioms Strata.Temporal.always_enabled_infinitely_often
#print axioms Strata.Temporal.Lock.lock_enabled
#print axioms Strata.Temporal.Lock.lock_deliver_leadsTo
#print axioms Strata.Temporal.Lock.lock_pass_leadsTo
#print axioms Strata.Temporal.Lock.lock_liveness
#print axioms Strata.Temporal.Lock.cyc_step
#print axioms Strata.Temporal.Lock.recur_of_period4
#print axioms Strata.Temporal.Lock.cycBeh_deliver_one
#print axioms Strata.Temporal.Lock.cycBeh_host_zero
#print axioms Strata.Temporal.Lock.cycBeh_host_one
#print axioms Strata.Temporal.Lock.lock_nonvacuous

#print axioms Strata.Browser.held_is_own
#print axioms Strata.Browser.held_guarded_signed_in
#print axioms Strata.Browser.page_shows_only_own
#print axioms Strata.Browser.visitor_shows_no_guarded_page
#print axioms Strata.Browser.session_change_drops_model
#print axioms Strata.Browser.requests_are_pending
#print axioms Strata.Browser.signed_in_view_reachable
#print axioms Strata.Browser.held_only_when_known
#print axioms Strata.Browser.cold_answer_changes_no_model
#print axioms Strata.Browser.cold_start_awaits_session
#print axioms Strata.Browser.login_sent_only_complete
#print axioms Strata.Browser.failure_keeps_session
#print axioms Strata.Browser.input_login_sends_nothing
#print axioms Strata.Browser.theme_cycle_returns
#print axioms Strata.Browser.theme_choice_stored
#print axioms Strata.Browser.session_change_keeps_theme
#print axioms Strata.Browser.dismissed_toast_gone
#print axioms Strata.Browser.toast_expiry_removes
#print axioms Strata.Browser.notice_advance_moves_both
#print axioms Strata.Browser.stopped_never_reconnects
#print axioms Strata.Browser.refusal_counts_and_stops
#print axioms Strata.Browser.invite_accepted_signs_nobody_in
#print axioms Strata.Browser.oidc_redirect_leaves_page
#print axioms Strata.Browser.link_sent_only_complete
#print axioms Strata.Browser.reconnect_waits_backoff
#print axioms Strata.Browser.second_factor_sent_only_when_waiting
#print axioms Strata.Browser.sign_in_ends_prompt
#print axioms Strata.Browser.refused_second_keeps_prompt
#print axioms Strata.Browser.passkey_answer_for_ceremony
#print axioms Strata.Browser.changed_list_is_read_again
#print axioms Strata.Browser.save_sent_only_checked
#print axioms Strata.Browser.publish_sent_only_checked
#print axioms Strata.Browser.editor_answer_reads_view
#print axioms Strata.Browser.manifest_version_never_drops
#print axioms Strata.Browser.tenant_action_sent_only_allowed
#print axioms Strata.Browser.tenant_answer_reads_view
#print axioms Strata.Browser.lab_run_sent_only_complete
#print axioms Strata.Browser.poll_only_while_running
#print axioms Strata.Browser.apply_only_when_ready
#print axioms Strata.Browser.page_data_is_own
#print axioms Strata.Browser.visitor_sees_no_guarded_data
#print axioms Strata.Browser.person_change_shows_loading


-- System spec and protocol (WG-5655): the trusted spec, the protocol, the refinement.
#print axioms Strata.System.serve_log_sub
#print axioms Strata.System.frames_ok_step
#print axioms Strata.System.spec_frames_readable
#print axioms Strata.System.serve_log_mem
#print axioms Strata.System.spec_answered_unique_step
#print axioms Strata.System.spec_log_unique
#print axioms Strata.System.updTxn_self
#print axioms Strata.System.updTxn_ne
#print axioms Strata.System.good_inert
#print axioms Strata.System.good_begin
#print axioms Strata.System.good_noSession
#print axioms Strata.System.upd_fields
#print axioms Strata.System.commit_transfer
#print axioms Strata.System.good_snap
#print axioms Strata.System.good_commit
#print axioms Strata.System.good_conflict
#print axioms Strata.System.good_done
#print axioms Strata.System.good_load
#print axioms Strata.System.good_storeDecided
#print axioms Strata.System.good_storeAccept
#print axioms Strata.System.good_storeRefuse
#print axioms Strata.System.good_step
#print axioms Strata.System.good_init
#print axioms Strata.System.steps_star
#print axioms Strata.System.star_one
#print axioms Strata.System.notFrame_ne
#print axioms Strata.System.frame_iff_send
#print axioms Strata.System.frame_eq_iff
#print axioms Strata.System.SysState.ext'
#print axioms Strata.System.abs_eq_of
#print axioms Strata.System.abs_init
#print axioms Strata.System.net_mono
#print axioms Strata.System.frames_eq_of
#print axioms Strata.System.star_eq_abs
#print axioms Strata.System.star_noSession
#print axioms Strata.System.star_accept
#print axioms Strata.System.star_refuse
#print axioms Strata.System.star_frame
#print axioms Strata.System.refine_star
#print axioms Strata.System.relayed_step
#print axioms Strata.System.protocol_refines
#print axioms Strata.System.observed_reply_is_decision
#print axioms Strata.System.observed_matches_spec
#print axioms Strata.System.frames_readable
#print axioms Strata.System.commit_once
#print axioms Strata.System.commit_is_verdict
#print axioms Strata.System.rev_step
#print axioms Strata.System.duplicate_commit_inert
#print axioms Strata.System.wInit_pinit
#print axioms Strata.System.refusal_reachable

-- Backend liveness (WG-5656): the fair spec, the reply, publish and decision theorems, the witness.
#print axioms Strata.System.fair_pers
#print axioms Strata.System.sfair_inf
#print axioms Strata.System.net_seq
#print axioms Strata.System.dcount_le
#print axioms Strata.System.dcount_mono
#print axioms Strata.System.dcount_upd
#print axioms Strata.System.bnd_upd
#print axioms Strata.System.txn_keep_step
#print axioms Strata.System.rev_mono_step
#print axioms Strata.System.updAtt_ne
#print axioms Strata.System.linv_bndP
#print axioms Strata.System.linv_bndT
#print axioms Strata.System.linv_rev
#print axioms Strata.System.linv_doneDec
#print axioms Strata.System.linv_nsRep
#print axioms Strata.System.linv_evsCom
#print axioms Strata.System.linv_txnFin
#print axioms Strata.System.linv_txnLoad
#print axioms Strata.System.linv_txnCom
#print axioms Strata.System.linv_loadRev
#print axioms Strata.System.linv_snapAtt
#print axioms Strata.System.linv_attLe
#print axioms Strata.System.linv_zeroNone
#print axioms Strata.System.linv_attBound
#print axioms Strata.System.linv_conflictLt
#print axioms Strata.System.linv_step
#print axioms Strata.System.linv_init
#print axioms Strata.System.seq_inv
#print axioms Strata.System.txn_chain
#print axioms Strata.System.txn_tenant_const
#print axioms Strata.System.store_enabled
#print axioms Strata.System.store_conflict_only
#print axioms Strata.System.ready_keep
#print axioms Strata.System.committing_keep
#print axioms Strata.System.storeStep_net
#print axioms Strata.System.store_step_undecided
#print axioms Strata.System.eventually_const_aux
#print axioms Strata.System.eventually_const
#print axioms Strata.System.txn_some_seq
#print axioms Strata.System.relay_of_reply
#print axioms Strata.System.relay_of_done
#print axioms Strata.System.nosession_false
#print axioms Strata.System.decided_none_of
#print axioms Strata.System.txn_exists
#print axioms Strata.System.attempt_mono_step
#print axioms Strata.System.conflict_inc
#print axioms Strata.System.protoSpec_parts
#print axioms Strata.System.reply_core
#print axioms Strata.System.reply_leadsTo
#print axioms Strata.System.publish_leadsTo
#print axioms Strata.System.decided_relayed_leadsTo
#print axioms Strata.System.sendOne_self
#print axioms Strata.System.updTxn_same
#print axioms Strata.System.tail_always
#print axioms Strata.System.tail_weak
#print axioms Strata.System.aT1_spec
#print axioms Strata.System.aT1_owner
#print axioms Strata.System.aT1_can
#print axioms Strata.System.a1_step
#print axioms Strata.System.a2_step
#print axioms Strata.System.a3_step
#print axioms Strata.System.a4_step
#print axioms Strata.System.a5_step
#print axioms Strata.System.a6_step
#print axioms Strata.System.a7_step
#print axioms Strata.System.a8_step
#print axioms Strata.System.a9_step
#print axioms Strata.System.a9_load
#print axioms Strata.System.a10_step
#print axioms Strata.System.a10_committed
#print axioms Strata.System.a11_step
#print axioms Strata.System.atr_step
#print axioms Strata.System.atr_ge
#print axioms Strata.System.a11_net
#print axioms Strata.System.a11_txn1
#print axioms Strata.System.a11_txne
#print axioms Strata.System.a11_decided
#print axioms Strata.System.a11_decided_rest
#print axioms Strata.System.a11_hist
#print axioms Strata.System.a11_rev
#print axioms Strata.System.a11_attempt
#print axioms Strata.System.a11_done
#print axioms Strata.System.a11_reply
#print axioms Strata.System.a11_clientReply
#print axioms Strata.System.a11_commit
#print axioms Strata.System.a11_fwd
#print axioms Strata.System.a11_load
#print axioms Strata.System.a11_snap
#print axioms Strata.System.a11_frame
#print axioms Strata.System.fwd_self
#print axioms Strata.System.begin_none
#print axioms Strata.System.relay_self
#print axioms Strata.System.done_self
#print axioms Strata.System.load_self
#print axioms Strata.System.commit_none
#print axioms Strata.System.conflict_none
#print axioms Strata.System.snap_none
#print axioms Strata.System.a11_committed
#print axioms Strata.System.publish_self
#print axioms Strata.System.store_self
#print axioms Strata.System.protoSpec_nonvacuous

-- Reduction (WG-5660): the concurrent transactions (a), the generic distributed reduction (b),
-- and their instantiation for the backend protocol.
#print axioms Strata.Reduction.upd_self
#print axioms Strata.Reduction.upd_ne
#print axioms Strata.Reduction.serialRun_append
#print axioms Strata.Reduction.committedReqs_length
#print axioms Strata.Reduction.filter_snoc_acc
#print axioms Strata.Reduction.filter_snoc_rej
#print axioms Strata.Reduction.tinv_init
#print axioms Strata.Reduction.accepted_of_ok
#print axioms Strata.Reduction.mem_accepted_base
#print axioms Strata.Reduction.tinv_step
#print axioms Strata.Reduction.tinv_of_reach
#print axioms Strata.Reduction.serial_equivalence
#print axioms Strata.Reduction.conflict_no_effect
#print axioms Strata.Reduction.refusal_no_effect
#print axioms Strata.Reduction.storeState_eq'
#print axioms Strata.Reduction.find_snoc
#print axioms Strata.Reduction.decideOk_refines_store
#print axioms Strata.Reduction.decideErr_refines_store
#print axioms Strata.Reduction.conflict_refines_store
#print axioms Strata.Reduction.step_refines_store
#print axioms Strata.Reduction.allRecvIn_netOk
#print axioms Strata.Reduction.netOk_mono
#print axioms Strata.Reduction.netOk_congr
#print axioms Strata.Reduction.set_cons
#print axioms Strata.Reduction.netOk_append
#print axioms Strata.Reduction.netOk_prefix
#print axioms Strata.Reduction.recv_of_nosend
#print axioms Strata.Reduction.split_first
#print axioms Strata.Reduction.seg_eq_hostEvs
#print axioms Strata.Reduction.tagIs_unique
#print axioms Strata.Reduction.netOk_remove
#print axioms Strata.Reduction.hostEvs_append
#print axioms Strata.Reduction.hostEvs_filter
#print axioms Strata.Reduction.hostEvs_of_all
#print axioms Strata.Reduction.hostEvs_of_none
#print axioms Strata.Reduction.sorted_split
#print axioms Strata.Reduction.hostEvs_split
#print axioms Strata.Reduction.seg_remove_ne
#print axioms Strata.Reduction.seg_remove_self
#print axioms Strata.Reduction.seg_netOk
#print axioms Strata.Reduction.ne_or_of_not_and
#print axioms Strata.Reduction.seg_remove_of_nonempty
#print axioms Strata.Reduction.reordered_nil
#print axioms Strata.Reduction.reorder_core
#print axioms Strata.Reduction.reduction
#print axioms Strata.Reduction.rstep_tk
#print axioms Strata.Reduction.rstep_empty
#print axioms Strata.Reduction.bridge_backend
#print axioms Strata.Reduction.bridge_store
#print axioms Strata.Reduction.bridge_other
#print axioms Strata.Reduction.sendIff
#print axioms Strata.Reduction.recvIff
#print axioms Strata.Reduction.mem_sendsOf
#print axioms Strata.Reduction.mem_recvsOf
#print axioms Strata.Reduction.recv_of_obl
#print axioms Strata.Reduction.eq_of_gap
#print axioms Strata.Reduction.star_trans
#print axioms Strata.Reduction.initState_pinit
#print axioms Strata.Reduction.step_role
#print axioms Strata.Reduction.bridge_run
#print axioms Strata.Reduction.backend_fine_refines
#print axioms Strata.Reduction.run_congr

-- Boundary codec (WG-5653)
#print axioms Strata.Codec.lawful_nat
#print axioms Strata.Codec.lawful_int
#print axioms Strata.Codec.lawful_bool
#print axioms Strata.Codec.lawful_str
#print axioms Strata.Codec.lawful_pair
#print axioms Strata.Codec.lawful_iso
#print axioms Strata.Codec.lawful_opt
#print axioms Strata.Codec.lawful_list
#print axioms Strata.Codec.lawful_onDelete
#print axioms Strata.Codec.lawful_err
#print axioms Strata.Codec.lawful_ty
#print axioms Strata.Codec.lawful_elem
#print axioms Strata.Codec.lawful_val
#print axioms Strata.Codec.lawful_fieldDef
#print axioms Strata.Codec.lawful_limits
#print axioms Strata.Codec.lawful_record
#print axioms Strata.Codec.lawful_tenant
#print axioms Strata.Codec.lawful_intent
#print axioms Strata.Codec.lawful_ctx
#print axioms Strata.Codec.lawful_result
#print axioms Strata.Codec.decodeBytes_encodeBytes
#print axioms Strata.Codec.encodeBytes_of_decodeBytes
#print axioms Strata.Codec.fix_val_str
#print axioms Strata.Codec.fix_tenant
#print axioms Strata.Codec.fix_intent_update
#print axioms Strata.Codec.fix_result_ok
#print axioms Strata.Codec.boundary_codecs


-- Slice 4: several object types
#print axioms Strata.unknown_type_refused
#print axioms Strata.unknown_type_owner
#print axioms Strata.unknown_type_other
#print axioms Strata.unknown_type_hidden
#print axioms Strata.create_refs_resolve
#print axioms Strata.update_refs_resolve
#print axioms Strata.write_refused_dangling_across_types
#print axioms Strata.update_refused_dangling_across_types
#print axioms Strata.unknown_type_schema_refused

-- Slice 4 correction: a strict schema change is checked over the object types it touches
#print axioms Strata.schemaViolated_of_strict
#print axioms Strata.schema_step_putType
#print axioms Strata.strict_schema_change_scoped
#print axioms Strata.strict_schema_change_conforms
#print axioms Strata.strict_schema_change_unique

-- Executable kernel (Strata/Impl/*.lean, docs/formal-verification.md): every theorem
#print axioms Strata.initialWorld_records
#print axioms Strata.Impl.ascend_append
#print axioms Strata.Impl.lookup_append_nat
#print axioms Strata.Impl.lookup_cell
#print axioms Strata.Impl.lookup_ascend
#print axioms Strata.Impl.lookup_absSlots
#print axioms Strata.Impl.has_absSlots
#print axioms Strata.Impl.mem_ascend
#print axioms Strata.Impl.mem_absSlots
#print axioms Strata.Impl.anyFrom_iff
#print axioms Strata.Impl.any_absSlots
#print axioms Strata.Impl.ids_ge
#print axioms Strata.Impl.ascend_replicate
#print axioms Strata.Impl.absSlots_replicate
#print axioms Strata.Impl.absSlots_push
#print axioms Strata.Impl.beq_false_of_ne'
#print axioms Strata.Impl.ascend_set
#print axioms Strata.Impl.absSlots_setIfInBounds
#print axioms Strata.Impl.ascend_map
#print axioms Strata.Impl.ascend_clear
#print axioms Strata.Impl.absSlots_clear
#print axioms Strata.Impl.absSlots_map
#print axioms Strata.Impl.absSlots_padTo
#print axioms Strata.Impl.size_padTo
#print axioms Strata.Impl.IObj.abs_schema
#print axioms Strata.Impl.IObj.abs_uniques
#print axioms Strata.Impl.lookup_map_abs
#print axioms Strata.Impl.typ?_abs
#print axioms Strata.Impl.putTypeI_abs
#print axioms Strata.Impl.putType_abs
#print axioms Strata.Impl.env_abs
#print axioms Strata.Impl.lookup_cons_ne
#print axioms Strata.Impl.lookup_cons_eq
#print axioms Strata.Impl.lookup_putTypeI
#print axioms Strata.Impl.inv_putType
#print axioms Strata.Impl.lookup_map_entry
#print axioms Strata.Impl.getRec_setSlot
#print axioms Strata.Impl.getRec_push_padTo
#print axioms Strata.Impl.size_setSlot
#print axioms Strata.Impl.getRec_some
#print axioms Strata.Impl.refHas_eq
#print axioms Strata.Impl.elem_resolves_eq
#print axioms Strata.Impl.val_resolves_eq
#print axioms Strata.Impl.clashAt_eq
#print axioms Strata.Impl.valueProblem_eq
#print axioms Strata.Impl.fieldProblem_eq
#print axioms Strata.Impl.compositeProblem_eq
#print axioms Strata.Impl.problem_eq
#print axioms Strata.Impl.canCreateI_eq
#print axioms Strata.Impl.canAttemptCreateI_eq
#print axioms Strata.Impl.canUpdateI_eq
#print axioms Strata.Impl.canDeleteI_eq
#print axioms Strata.Impl.missingRecordI_eq
#print axioms Strata.Impl.unknownTypeI_eq
#print axioms Strata.Impl.problemI_abs
#print axioms Strata.Impl.blocked_eq
#print axioms Strata.Impl.abs_members
#print axioms Strata.Impl.abs_limits
#print axioms Strata.Impl.abs_nextId
#print axioms Strata.Impl.abs_types
#print axioms Strata.Impl.afterDelete_abs
#print axioms Strata.Impl.except_map_ite
#print axioms Strata.Impl.stepTenantI_refines
#print axioms Strata.Impl.stepTenantI_inv
#print axioms Strata.Impl.stepI_refines
#print axioms Strata.Impl.stepI_inv
#print axioms Strata.Impl.ofEmpty_abs
#print axioms Strata.Impl.ofEmpty_inv
#print axioms Strata.Impl.implInitial_inv
#print axioms Strata.Impl.absW_implInitial
#print axioms Strata.Impl.clearRef_idem
#print axioms Strata.Impl.blockedX_eq
#print axioms Strata.Impl.typ?_putType_same
#print axioms Strata.Impl.typ?_putType_other
#print axioms Strata.Impl.ClearedFrom.refl
#print axioms Strata.Impl.clearOne_cleared
#print axioms Strata.Impl.ClearedFrom.congr
#print axioms Strata.Impl.clearAll_cleared
#print axioms Strata.Impl.getRec_map
#print axioms Strata.Impl.array_ext_getRec
#print axioms Strata.Impl.list_ext_lookup
#print axioms Strata.Impl.IObj.ext'
#print axioms Strata.Impl.ITenant.ext'
#print axioms Strata.Impl.getRec_delObjI
#print axioms Strata.Impl.deleteX_good
#print axioms Strata.Impl.delete_good
#print axioms Strata.Impl.MM.get_add
#print axioms Strata.Impl.MM.get_remove
#print axioms Strata.Impl.MM.mem_addAll
#print axioms Strata.Impl.MM.mem_removeAll
#print axioms Strata.Impl.MM.mem_addCells
#print axioms Strata.Impl.MM.inv_change
#print axioms Strata.Impl.MM.inv_addCells
#print axioms Strata.Impl.modifyTypeI_eq
#print axioms Strata.Impl.ITenant.modifyType_eq
#print axioms Strata.Impl.putTypeI_keys
#print axioms Strata.Impl.typ?_putType
#print axioms Strata.Impl.mem_of_lookup
#print axioms Strata.Impl.lookup_of_mem_nodup
#print axioms Strata.Impl.getRec_of_ge
#print axioms Strata.Impl.getRec_of_lt
#print axioms Strata.Impl.cc_putType
#print axioms Strata.Impl.CellChange.withNextId
#print axioms Strata.Impl.lt_of_getRec
#print axioms Strata.Impl.findSome_congr
#print axioms Strata.Impl.any_congr
#print axioms Strata.Impl.clashX_eq
#print axioms Strata.Impl.compClashX_eq
#print axioms Strata.Impl.valueProblemX_eq
#print axioms Strata.Impl.fieldProblemX_eq
#print axioms Strata.Impl.compositeProblemX_eq
#print axioms Strata.Impl.problemX_eq
#print axioms Strata.Impl.good_error
#print axioms Strata.Impl.create_good
#print axioms Strata.Impl.update_good
#print axioms Strata.Impl.keysBy_putSlots
#print axioms Strata.Impl.lookup_ty_map
#print axioms Strata.Impl.lookup_ty_makeRequired
#print axioms Strata.Impl.lookup_ty_makeUnique
#print axioms Strata.Impl.rkeysOf_congr
#print axioms Strata.Impl.ukeysOf_makeRequired
#print axioms Strata.Impl.ukeysOf_makeUnique_sub
#print axioms Strata.Impl.mem_cellsOf
#print axioms Strata.Impl.keysBy_ty
#print axioms Strata.Impl.MM.inv_putSlots
#print axioms Strata.Impl.setRequired_good
#print axioms Strata.Impl.setUnique_good
#print axioms Strata.Impl.defineUnique_good
#print axioms Strata.Impl.mem_refIds
#print axioms Strata.Impl.mem_ukeysOf
#print axioms Strata.Impl.mem_ckeysOf
#print axioms Strata.Impl.mem_rkeysOf
#print axioms Strata.Impl.CellChange.keysBy
#print axioms Strata.Impl.ITenant.keysBy_at
#print axioms Strata.Impl.ukf_congr
#print axioms Strata.Impl.ckf_congr
#print axioms Strata.Impl.rkf_congr
#print axioms Strata.Impl.XTenant.ok_reindex
#print axioms Strata.Impl.stepTenantXL_good
#print axioms Strata.Impl.stepTenantX_good
#print axioms Strata.Impl.stepTenantX_refines
#print axioms Strata.Impl.stepTenantX_ok
#print axioms Strata.Impl.stepX_refines
#print axioms Strata.Impl.stepX_ok
#print axioms Strata.Impl.ofCore_ok
#print axioms Strata.Impl.xInitial_ok
#print axioms Strata.Impl.absX_xInitial

-- Slice 5: expressions and constraints
#print axioms Strata.and_false_shortcircuit
#print axioms Strata.or_true_shortcircuit
#print axioms Strata.and_null_false
#print axioms Strata.and_null_true
#print axioms Strata.and_null_null
#print axioms Strata.and_true_null
#print axioms Strata.and_null_error
#print axioms Strata.and_int_error
#print axioms Strata.or_null_true
#print axioms Strata.or_null_false
#print axioms Strata.or_false_error
#print axioms Strata.not_null
#print axioms Strata.isNull_null
#print axioms Strata.cmp_null_operand
#print axioms Strata.cmp_kind_error
#print axioms Strata.cmp_bool_order_error
#print axioms Strata.and_false_left
#print axioms Strata.or_true_left
#print axioms Strata.and_null_left_nonfalse
#print axioms Strata.and_error_left
#print axioms Strata.constraint_violated_refused
#print axioms Strata.constraint_evaluation_refused
#print axioms Strata.null_constraint_passes
#print axioms Strata.constraint_first_failure
#print axioms Strata.create_constraint_refused
#print axioms Strata.update_constraint_refused
#print axioms Strata.create_default_error_when_permitted
#print axioms Strata.create_unauthorized_regardless_of_defaults
#print axioms Strata.create_checks_defaulted_record
#print axioms Strata.default_refused_by_constraint
#print axioms Strata.named_value_accepted
#print axioms Strata.unauthorized_regardless_of_defaults
#print axioms Strata.canCreate_attempt
#print axioms Strata.canCreate_false_of_attempt
#print axioms Strata.evalExpr_congr
#print axioms Strata.constraintProblem_congr
#print axioms Strata.satisfied_of_putType
#print axioms Strata.stepTenant_preserves_satisfied
#print axioms Strata.step_preserves_satisfied
#print axioms Strata.brkTenant_satisfied
#print axioms Strata.brk_delete_refused
#print axioms Strata.clearBreaks_constraints
#print axioms Strata.Impl.clearBreaks_eq
#print axioms Strata.create_fills_default
#print axioms Strata.create_within_limits
#print axioms Strata.update_within_limits
#print axioms Strata.create_refused_over_record_bound
#print axioms Strata.newRecord_nodup
#print axioms Strata.defaultsOf_nodup
#print axioms Strata.create_refs_resolve
#print axioms Strata.write_refused_dangling_across_types
#print axioms Strata.unknown_type_hidden
#print axioms Strata.Codec.lawful_expr
#print axioms Strata.Codec.lawful_err
#print axioms Strata.Codec.lawful_objType
#print axioms Strata.Codec.lawful_fieldDef
#print axioms Strata.Codec.fix_fieldDef
#print axioms Strata.Codec.fix_tenant

-- Slice 6: roles and policies as data (Strata/Model.lean, Strata/Roles.lean).
#print axioms Strata.Tenant.allows_iff
#print axioms Strata.default_deny
#print axioms Strata.default_deny_create
#print axioms Strata.default_deny_update
#print axioms Strata.default_deny_delete
#print axioms Strata.create_ok_permitted
#print axioms Strata.update_ok_permitted
#print axioms Strata.delete_ok_permitted
#print axioms Strata.setRequired_ok_permitted
#print axioms Strata.unknown_type_owner
#print axioms Strata.unknown_type_other
#print axioms Strata.unknown_type_hidden
#print axioms Strata.unknown_type_schema_refused
#print axioms Strata.rolesOf_system
#print axioms Strata.rolesOf_anonymous
#print axioms Strata.anonymous_not_owner
#print axioms Strata.system_never_permitted
#print axioms Strata.anonymous_permitted_only_public
#print axioms Strata.system_refused_schema
#print axioms Strata.system_refused_create
#print axioms Strata.system_refused_update
#print axioms Strata.system_refused_delete
#print axioms Strata.owner_holds_every_capability
#print axioms Strata.capability_held_by_grant
#print axioms Strata.fixture_owner_permits
#print axioms Strata.fixture_editor_permits
#print axioms Strata.fixture_stranger_refused_item
#print axioms Strata.fixture_anonymous_refused_item
#print axioms Strata.fixture_anonymous_creates_part
#print axioms Strata.fixture_stranger_refused_part
#print axioms Strata.fixture_editor_creates_item
#print axioms Strata.fixture_writer_updates_own
#print axioms Strata.fixture_writer_refused_other
#print axioms Strata.fixture_editor_updates_other
#print axioms Strata.fixture_writer_missing_record_hidden
#print axioms Strata.fixture_editor_missing_record
#print axioms Strata.fixture_system_refused_item

-- Slice 7: membership, teams, invitations, tenant creation (Strata/MembersProofs.lean)
#print axioms Strata.effective_roles
#print axioms Strata.system_holds_no_role
#print axioms Strata.anonymous_holds_public
#print axioms Strata.system_cannot_manage_members
#print axioms Strata.hasOwner_iff
#print axioms Strata.genesis_hasOwner
#print axioms Strata.changeRole_owner
#print axioms Strata.defineTeam_owner
#print axioms Strata.teamRole_owner
#print axioms Strata.teamMember_owner
#print axioms Strata.redeemInvitation_owner
#print axioms Strata.stepMember_owner_kept
#print axioms Strata.owner_kept_world
#print axioms Strata.stepMember_teamsNodup
#print axioms Strata.step_nodup
#print axioms Strata.assign_only_conferrable
#print axioms Strata.teamRole_only_conferrable
#print axioms Strata.teamMember_only_conferrable
#print axioms Strata.redeem_only_conferrable
#print axioms Strata.create_invitation_expiry
#print axioms Strata.redeem_sets_redeemed
#print axioms Strata.redeem_twice_refused
#print axioms Strata.revoked_not_redeemable
#print axioms Strata.redeem_not_expired
#print axioms Strata.redeem_not_closed
#print axioms Strata.redeem_issuer_authority
#print axioms Strata.reserved_refused
#print axioms Strata.assign_reserved_refused
#print axioms Strata.add_member_reserved_refused
#print axioms Strata.create_reserved_refused
#print axioms Strata.create_genesis
#print axioms Strata.create_existing_refused
#print axioms Strata.create_not_system_refused
#print axioms Strata.genesis_owner_and_nodup
#print axioms Strata.directOf_setDirect
#print axioms Strata.rolesOf_mono_setDirect

-- Slice 7 integration: the membership intents in `Intent` and `step`, the kernels, the working set and the codec
#print axioms Strata.stepTenant_member_eq
#print axioms Strata.step_cases
#print axioms Strata.stepMember_manage
#print axioms Strata.member_authorized
#print axioms Strata.genesisTenant_wf
#print axioms Strata.rolesOf_user
#print axioms Strata.rolesOf_user_no_teams
#print axioms Strata.reserved_caller_refused
#print axioms Strata.create_blank_refused
#print axioms Strata.create_world_form
#print axioms Strata.step_member_form
#print axioms Strata.stepTenant_data_mt
#print axioms Strata.step_data_mt
#print axioms Strata.step_other
#print axioms Strata.step_exists
#print axioms Strata.Impl.createdTenantI_abs
#print axioms Strata.Impl.genesis_inv
#print axioms Strata.Impl.createTenant_good
#print axioms Strata.Impl.member_good
#print axioms Strata.Impl.genesis_ok
#print axioms Strata.Impl.createdTenantX_abs
#print axioms Strata.Impl.createdTenantX_ok
#print axioms Strata.WorkingSet.sound_members
#print axioms Strata.WorkingSet.sound_genesis
#print axioms Strata.WorkingSet.applyWorld_other
#print axioms Strata.Codec.lawful_teamDef
#print axioms Strata.Codec.lawful_invitationDef
#print axioms Strata.Codec.lawful_memberIntent
#print axioms Strata.Codec.fix_tenant_members
#print axioms Strata.Codec.fix_policy_members
#print axioms Strata.Codec.fix_intent_createTenant
#print axioms Strata.Codec.fix_intent_assignRole
#print axioms Strata.Codec.fix_intent_defineTeam
#print axioms Strata.Codec.fix_intent_createInvitation
#print axioms Strata.Codec.fix_intent_revokeInvitation
#print axioms Strata.Codec.fix_intent_redeemInvitation
#print axioms Strata.Codec.fix_intent_redeemInvitation_unverified
#print axioms Strata.Codec.fix_err_invalidIntent
#print axioms Strata.Codec.fix_err_unknownRole
#print axioms Strata.Codec.fix_err_lastOwner
#print axioms Strata.Codec.fix_err_tenantExists

-- Slice 8: workflows (`Strata/Workflows.lean`)
#print axioms Strata.create_ok_initials
#print axioms Strata.create_workflow_initial
#print axioms Strata.create_non_initial_refused
#print axioms Strata.fireOne_declared
#print axioms Strata.fireOne_keeps
#print axioms Strata.fireAll_final
#print axioms Strata.fireAll_keeps
#print axioms Strata.fireAll_present
#print axioms Strata.clearRef_present
#print axioms Strata.get_clearFields_str
#print axioms Strata.update_changed_fired
#print axioms Strata.update_undeclared_refused
#print axioms Strata.update_writes_fired
#print axioms Strata.str_of_clearFields
#print axioms Strata.clearRef_statesOk
#print axioms Strata.stepTenant_preserves_states
#print axioms Strata.step_preserves_states
#print axioms Strata.update_raises_version
#print axioms Strata.update_writes_patched
#print axioms Strata.update_keeps_unnamed
#print axioms Strata.update_ok_parts
#print axioms Strata.fireAll_plain
#print axioms Strata.fireOne_version
#print axioms Strata.fireAll_nodup
#print axioms Strata.evalSets_keys
#print axioms Strata.applySets_get_mem
#print axioms Strata.wf_lookup_filter_none
#print axioms Strata.lookup_filter_of_ne

-- Slice 9: events, revisions and effects (`Strata/EventProofs.lean`).
#print axioms Strata.transition_ok
#print axioms Strata.transition_refused
#print axioms Strata.eventBodies_commit
#print axioms Strata.transition_existing
#print axioms Strata.numberEvents_map_seq
#print axioms Strata.numberEvents_length
#print axioms Strata.transition_seqs
#print axioms Strata.transition_publish
#print axioms Strata.transition_revision
#print axioms Strata.transition_created
#print axioms Strata.typ_of_stepTenant_create
#print axioms Strata.transition_create
#print axioms Strata.transition_update
#print axioms Strata.stored_of_stepTenant_delete
#print axioms Strata.transition_delete
#print axioms Strata.transition_setRequired
#print axioms Strata.Impl.absW_commitWorldI
#print axioms Strata.Impl.transitionI_refines
#print axioms Strata.Impl.absX_commitWorldX
#print axioms Strata.Impl.transitionX_refines
#print axioms Strata.Impl.commitI_abs

/-! ## Slice 10: reads (`Strata/Reads.lean`): records and named queries

The statements are about the model's `read`. The theorems are checked by `lake env lean Audit.lean`,
which is not a module of the `lake build` target. -/

namespace Strata

/-! ### Visibility and the system -/

theorem system_never_visible (t : Tenant) (ty : TypeId) (r : Record) :
    t.canRead .system ty r = false := by
  simp [Tenant.canRead, Tenant.allows, Tenant.rolesOf, Tenant.mt, MTenant.rolesOf]

theorem redact_system (rs : FieldRules) (ty : TypeId) (r : Record) :
    redact rs .system ty r = r := by
  have hkeep : r.fields.filter (fun fv => shownField rs .system ty r fv.1) = r.fields :=
    List.filter_eq_self.mpr (fun a _ => by simp [shownField])
  simp only [redact, hkeep]

/-! ### Record reads -/

theorem rowOf_visible {t : Tenant} {rs : FieldRules} {p : Principal} {ty : TypeId}
    {id : RecordId} {row : RecordId × Record} (h : rowOf t rs p ty id = some row) :
    ∃ r, t.recordAt ty id = some r ∧ t.canRead p ty r = true ∧ row = (id, redact rs p ty r) := by
  unfold rowOf at h
  cases hr : t.recordAt ty id with
  | none => simp [hr] at h
  | some r =>
    cases hv : t.canRead p ty r with
    | false => simp [hr, hv] at h
    | true =>
      simp only [hr, hv, ite_true] at h
      exact ⟨r, rfl, hv, (Option.some.inj h).symm⟩

/-- A missing record and an invisible one are left out the same way: neither yields a row. -/
theorem missing_and_invisible_same (t : Tenant) (rs : FieldRules) (p : Principal) (ty : TypeId)
    (id : RecordId) :
    (t.recordAt ty id = none → rowOf t rs p ty id = none) ∧
    (∀ r, t.recordAt ty id = some r → t.canRead p ty r = false → rowOf t rs p ty id = none) := by
  constructor
  · intro h
    simp [rowOf, h]
  · intro r hr hv
    simp [rowOf, hr, hv]

theorem readRecords_limit_first (t : Tenant) (spec : ReadSpec) (p : Principal) (ty : TypeId)
    (ids : List RecordId) (h : maxReadIds < ids.length) :
    readRecords t spec p ty ids = .error .limitExceeded := by
  simp [readRecords, h]

theorem readRecords_unknown_type (t : Tenant) (spec : ReadSpec) (p : Principal) (ty : TypeId)
    (ids : List RecordId) (h : ids.length ≤ maxReadIds) (hty : t.typ? ty = none) :
    readRecords t spec p ty ids = .error .unknownObject := by
  have h' : ¬ maxReadIds < ids.length := by omega
  simp [readRecords, h', hty]

/-- The rows of a record read are the visible records, redacted, in request order. -/
theorem readRecords_rows (t : Tenant) (spec : ReadSpec) (p : Principal) (ty : TypeId)
    (ids : List RecordId) (rows : List (RecordId × Record))
    (h : readRecords t spec p ty ids = .ok rows) :
    rows = ids.filterMap (rowOf t spec.rules p ty) ∧
    ∀ x ∈ rows, ∃ id r, t.recordAt ty id = some r ∧ t.canRead p ty r = true ∧
      x = (id, redact spec.rules p ty r) := by
  unfold readRecords at h
  split at h
  · simp at h
  · split at h
    · simp at h
    · have hrows : rows = ids.filterMap (rowOf t spec.rules p ty) := by
        simpa using (Except.ok.inj h).symm
      refine ⟨hrows, ?_⟩
      intro x hx
      rw [hrows] at hx
      obtain ⟨id, _, hid⟩ := List.mem_filterMap.mp hx
      obtain ⟨r, hr, hv, hx'⟩ := rowOf_visible hid
      exact ⟨id, r, hr, hv, hx'⟩

/-- A read rule that hides a field of a stored record keeps that field out of every row, for every
    reader but the system (which the rule does not restrict). -/
theorem hidden_field_absent (rs : FieldRules) (p : Principal) (ty : TypeId) (r : Record)
    (f : FieldId) (e : Expr) (hp : p ≠ .system) (hr : rs.rule ty f = some e)
    (hh : condHolds (readEnv p r) (some e) = false) :
    ∀ v, (f, v) ∉ (redact rs p ty r).fields := by
  intro v hv
  simp only [redact, List.mem_filter] at hv
  obtain ⟨_, hs⟩ := hv
  cases p with
  | system => exact hp rfl
  | user n => simp_all [shownField]
  | anonymous => simp_all [shownField]

/-- The system is refused every record, so it reads none: the answer is the error or the empty list. -/
theorem system_reads_no_record (t : Tenant) (spec : ReadSpec) (ty : TypeId)
    (ids : List RecordId) (h : ids.length ≤ maxReadIds) :
    readRecords t spec .system ty ids =
      if (t.typ? ty).isNone then .error .unknownObject else .ok [] := by
  have h' : ¬ maxReadIds < ids.length := by omega
  unfold readRecords
  simp only [h', ite_false]
  split
  · rfl
  · have : ids.filterMap (rowOf t spec.rules .system ty) = [] := by
      apply List.filterMap_eq_nil_iff.mpr
      intro id _
      unfold rowOf
      cases t.recordAt ty id with
      | none => rfl
      | some r => simp [system_never_visible]
    simp [this]

end Strata

namespace Strata

/-! ### Named queries -/

theorem scanRows_mem (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef)
    (x : RecordId × Record) :
    x ∈ scanRows t spec p q ↔ ∃ id r, (id, r) ∈ t.recordsOf q.ty ∧ t.canRead p q.ty r = true ∧
      condHolds (readEnv p (redact spec.rules p q.ty r)) q.filter = true ∧
      x = (id, redact spec.rules p q.ty r) := by
  unfold scanRows
  rw [List.mem_filterMap]
  constructor
  · rintro ⟨⟨id, r⟩, hmem, hx⟩
    dsimp only at hx
    split at hx
    · rename_i hv
      split at hx
      · rename_i hf
        simp only [Option.some.injEq] at hx
        exact ⟨id, r, hmem, hv, hf, hx.symm⟩
      · simp at hx
    · simp at hx
  · rintro ⟨id, r, hmem, hv, hf, rfl⟩
    refine ⟨(id, r), hmem, ?_⟩
    simp [hv, hf]

theorem queryAll_mem_iff (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef)
    (x : RecordId × Record) : x ∈ queryAll t spec p q ↔ x ∈ scanRows t spec p q := by
  unfold queryAll
  exact List.mem_mergeSort

/-- The rows of a named query are exactly the records the principal may read whose redacted copy
    makes the filter exactly `true`, each redacted. -/
theorem query_mem_iff (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef)
    (x : RecordId × Record) :
    x ∈ queryAll t spec p q ↔ ∃ id r, (id, r) ∈ t.recordsOf q.ty ∧ t.canRead p q.ty r = true ∧
      condHolds (readEnv p (redact spec.rules p q.ty r)) q.filter = true ∧
      x = (id, redact spec.rules p q.ty r) := by
  rw [queryAll_mem_iff, scanRows_mem]

/-- A row of a named query hides no field a read rule hides on the record it was read from. -/
theorem query_rows_hide (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef)
    (x : RecordId × Record) (hx : x ∈ queryAll t spec p q) :
    ∃ id r, (id, r) ∈ t.recordsOf q.ty ∧ x = (id, redact spec.rules p q.ty r) ∧
      ∀ f e, spec.rules.rule q.ty f = some e →
        condHolds (readEnv p r) (some e) = false →
        ∀ v, (f, v) ∉ x.2.fields := by
  obtain ⟨id, r, hmem, hv, _, hxr⟩ := (query_mem_iff t spec p q x).mp hx
  refine ⟨id, r, hmem, hxr, ?_⟩
  intro f e hr hh v hvm
  have hp : p ≠ .system := by
    rintro rfl
    simp [system_never_visible] at hv
  rw [hxr] at hvm
  exact hidden_field_absent spec.rules p q.ty r f e hp hr hh v hvm

theorem orderKey_length (q : QueryDef) (row : RecordId × Record) :
    (orderKey q row).length = (orderDirs q).length := by
  simp [orderKey, orderDirs]

/-- The rows of a named query are in order: each row's keys, then its id, are at most the next row's. -/
theorem queryAll_sorted (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef) :
    (queryAll t spec p q).Pairwise
      (fun a b => keysLe (orderDirs q) (orderKey q a) (orderKey q b) = true) := by
  unfold queryAll
  apply List.pairwise_mergeSort
  · intro a b c h1 h2
    exact keysLe_trans _ _ _ _ (orderKey_length q a) (orderKey_length q b)
      (orderKey_length q c) h1 h2
  · intro a b
    exact keysLe_total _ _ _ (orderKey_length q a) (orderKey_length q b)

theorem walk_eq_drop {α : Type} (L : List α) (size : Nat) (hs : 0 < size) :
    ∀ n off, L.length - off ≤ n → walk L size off = L.drop off := by
  intro n
  induction n with
  | zero =>
    intro off h
    have hlen : L.length ≤ off := by omega
    unfold walk
    split
    · omega
    · simp [List.drop_eq_nil_of_le hlen]
  | succ n ih =>
    intro off h
    unfold walk
    split
    · rename_i hc
      rw [ih (off + size) (by omega), ← List.drop_drop, List.take_append_drop]
    · rename_i hc
      rw [List.take_of_length_le (by simp; omega)]

/-- The pages of a named query, concatenated, are the whole of its result, in order. Since the result
    lists each record once, no record appears in two pages. -/
theorem query_pages_concat (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef)
    (size : Nat) (hs : 0 < size) :
    walk (queryAll t spec p q) size 0 = queryAll t spec p q := by
  rw [walk_eq_drop _ size hs (queryAll t spec p q).length 0 (by simp), List.drop_zero]

/-! ### The limits and refusals of a named query -/

theorem query_unknown (t : Tenant) (spec : ReadSpec) (p : Principal) (qid : QueryId)
    (params : List (String × Val)) (page : Option Nat) (cursor : Option Cursor)
    (h : spec.queries.lookup qid = none) :
    queryPage t spec p qid params page cursor = .error .unknownQuery := by
  simp [queryPage, h]

theorem query_undeclared_param (t : Tenant) (spec : ReadSpec) (p : Principal) (qid : QueryId)
    (params : List (String × Val)) (page : Option Nat) (cursor : Option Cursor) (q : QueryDef)
    (hq : spec.queries.lookup qid = some q)
    (hp : params.any (fun kv => !(q.params.contains kv.1)) = true) :
    queryPage t spec p qid params page cursor = .error .invalidIntent := by
  unfold queryPage
  rw [hq]
  dsimp only
  rw [if_pos hp]

theorem query_page_zero (t : Tenant) (spec : ReadSpec) (p : Principal) (qid : QueryId)
    (params : List (String × Val)) (page : Option Nat) (cursor : Option Cursor) (q : QueryDef)
    (hq : spec.queries.lookup qid = some q)
    (hp : params.any (fun kv => !(q.params.contains kv.1)) = false)
    (hs : (page.or q.limit).getD defaultPage = 0) :
    queryPage t spec p qid params page cursor = .error .invalidIntent := by
  unfold queryPage
  rw [hq]
  dsimp only
  rw [if_neg (by rw [hp]; decide), if_pos hs]

theorem query_page_over (t : Tenant) (spec : ReadSpec) (p : Principal) (qid : QueryId)
    (params : List (String × Val)) (page : Option Nat) (cursor : Option Cursor) (q : QueryDef)
    (hq : spec.queries.lookup qid = some q)
    (hp : params.any (fun kv => !(q.params.contains kv.1)) = false)
    (hs : maxPage < (page.or q.limit).getD defaultPage) :
    queryPage t spec p qid params page cursor = .error .limitExceeded := by
  have hz : (page.or q.limit).getD defaultPage ≠ 0 := by omega
  unfold queryPage
  rw [hq]
  dsimp only
  rw [if_neg (by rw [hp]; decide), if_neg hz, if_pos hs]

theorem query_wrong_cursor (t : Tenant) (spec : ReadSpec) (p : Principal) (qid : QueryId)
    (params : List (String × Val)) (page : Option Nat) (q : QueryDef) (id : RecordId)
    (hq : spec.queries.lookup qid = some q)
    (hp : params.any (fun kv => !(q.params.contains kv.1)) = false)
    (hs : 0 < (page.or q.limit).getD defaultPage)
    (hb : (page.or q.limit).getD defaultPage ≤ maxPage) :
    queryPage t spec p qid params page (some (.after id)) = .error .invalidIntent := by
  have hz : (page.or q.limit).getD defaultPage ≠ 0 := by omega
  have hb' : ¬ maxPage < (page.or q.limit).getD defaultPage := by omega
  unfold queryPage
  rw [hq]
  dsimp only
  rw [if_neg (by rw [hp]; decide), if_neg hz, if_neg hb']

theorem query_scan_limit (t : Tenant) (spec : ReadSpec) (p : Principal) (qid : QueryId)
    (params : List (String × Val)) (page : Option Nat) (off : Nat) (q : QueryDef)
    (hq : spec.queries.lookup qid = some q)
    (hp : params.any (fun kv => !(q.params.contains kv.1)) = false)
    (hs : 0 < (page.or q.limit).getD defaultPage)
    (hb : (page.or q.limit).getD defaultPage ≤ maxPage)
    (hn : maxScan < ((t.typ? q.ty).map (fun o => o.records.length)).getD 0) :
    queryPage t spec p qid params page (some (.offset off)) = .error .limitExceeded := by
  have hz : (page.or q.limit).getD defaultPage ≠ 0 := by omega
  have hb' : ¬ maxPage < (page.or q.limit).getD defaultPage := by omega
  unfold queryPage
  rw [hq]
  dsimp only
  rw [if_neg (by rw [hp]; decide), if_neg hz, if_neg hb', if_pos hn]

/-! ### The system -/

/-- The system is refused every record of every type, so a query's result for it is empty. -/
theorem queryAll_system (t : Tenant) (spec : ReadSpec) (q : QueryDef) :
    queryAll t spec .system q = [] := by
  unfold queryAll scanRows
  have h0 : (t.recordsOf q.ty).filterMap (fun e =>
      if t.canRead .system q.ty e.2 then
        (if condHolds (readEnv .system (redact spec.rules .system q.ty e.2)) q.filter = true then
          some (e.1, redact spec.rules .system q.ty e.2) else none)
      else none) = [] := by
    apply List.filterMap_eq_nil_iff.mpr
    intro e _
    simp [system_never_visible]
  rw [h0]
  simp

end Strata

namespace Strata

/-! ### Distinct rows, hidden fields and the system, for both reads -/

theorem filterMap_fst_sublist {γ α β : Type} (l : List (γ × α)) (f : γ × α → Option (γ × β))
    (hf : ∀ e b, f e = some b → b.1 = e.1) :
    ((l.filterMap f).map Prod.fst).Sublist (l.map Prod.fst) := by
  induction l with
  | nil => simp
  | cons e l ih =>
    rw [List.filterMap_cons]
    cases h : f e with
    | none =>
      simp only [h, List.map_cons]
      exact ih.trans (List.sublist_cons_self _ _)
    | some b =>
      have hb := hf e b h
      simp only [h, List.map_cons, hb]
      exact List.Sublist.cons₂ _ ih

theorem scanRows_ids_nodup (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef)
    (h : ((t.recordsOf q.ty).map Prod.fst).Nodup) :
    ((scanRows t spec p q).map Prod.fst).Nodup := by
  unfold scanRows
  refine List.Sublist.nodup (filterMap_fst_sublist _ _ ?_) h
  intro e b hb
  dsimp only at hb
  split at hb
  · split at hb
    · simp only [Option.some.injEq] at hb
      rw [← hb]
    · simp at hb
  · simp at hb

/-- The rows of a named query name each record once, when the records of its type are distinct. -/
theorem queryAll_ids_nodup (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef)
    (h : ((t.recordsOf q.ty).map Prod.fst).Nodup) :
    ((queryAll t spec p q).map Prod.fst).Nodup := by
  unfold queryAll
  exact ((List.mergeSort_perm _ _).map Prod.fst).nodup_iff.mpr (scanRows_ids_nodup t spec p q h)

/-- The pages of a named query name no record twice: concatenated, they are the result. -/
theorem query_pages_nodup (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef)
    (size : Nat) (hs : 0 < size) (h : ((t.recordsOf q.ty).map Prod.fst).Nodup) :
    ((walk (queryAll t spec p q) size 0).map Prod.fst).Nodup := by
  rw [query_pages_concat t spec p q size hs]
  exact queryAll_ids_nodup t spec p q h

/-- A row of a record read hides no field a read rule hides on the record it was read from; the
    system reads no row at all, since no record is visible to it. -/
theorem readRecords_hide (t : Tenant) (spec : ReadSpec) (p : Principal) (ty : TypeId)
    (ids : List RecordId) (rows : List (RecordId × Record))
    (h : readRecords t spec p ty ids = .ok rows) :
    ∀ x ∈ rows, ∃ id r, t.recordAt ty id = some r ∧ x = (id, redact spec.rules p ty r) ∧
      ∀ f e, spec.rules.rule ty f = some e →
        condHolds (readEnv p r) (some e) = false →
        ∀ v, (f, v) ∉ x.2.fields := by
  intro x hx
  obtain ⟨_, hrows⟩ := readRecords_rows t spec p ty ids rows h
  obtain ⟨id, r, hr, hv, hxr⟩ := hrows x hx
  refine ⟨id, r, hr, hxr, ?_⟩
  intro f e hf hh v hvm
  have hp : p ≠ .system := by
    rintro rfl
    simp [system_never_visible] at hv
  rw [hxr] at hvm
  exact hidden_field_absent spec.rules p ty r f e hp hf hh v hvm

/-- The system's page of a named query is empty, whatever the query and the cursor. -/
theorem queryPage_system_rows (t : Tenant) (spec : ReadSpec) (qid : QueryId)
    (params : List (String × Val)) (page : Option Nat) (cursor : Option Cursor)
    (r : ReadResult) (h : queryPage t spec .system qid params page cursor = .ok r) :
    r.rows = [] := by
  unfold queryPage at h
  rcases hq : spec.queries.lookup qid with _ | q
  · rw [hq] at h
    simp at h
  · rw [hq] at h
    dsimp only at h
    repeat' split at h
    all_goals first
      | (cases h <;> simp [queryAll_system])
      | cases h

end Strata

/-! ## Slice 10 axioms -/
#print axioms Strata.Val.leb_total
#print axioms Strata.Val.leb_trans
#print axioms Strata.dirLe_total
#print axioms Strata.dirLe_trans
#print axioms Strata.keysLe_total
#print axioms Strata.keysLe_trans
#print axioms Strata.system_never_visible
#print axioms Strata.redact_system
#print axioms Strata.rowOf_visible
#print axioms Strata.missing_and_invisible_same
#print axioms Strata.readRecords_limit_first
#print axioms Strata.readRecords_unknown_type
#print axioms Strata.readRecords_rows
#print axioms Strata.hidden_field_absent
#print axioms Strata.system_reads_no_record
#print axioms Strata.scanRows_mem
#print axioms Strata.queryAll_mem_iff
#print axioms Strata.query_mem_iff
#print axioms Strata.query_rows_hide
#print axioms Strata.orderKey_length
#print axioms Strata.queryAll_sorted
#print axioms Strata.walk_eq_drop
#print axioms Strata.query_pages_concat
#print axioms Strata.query_unknown
#print axioms Strata.query_undeclared_param
#print axioms Strata.query_page_zero
#print axioms Strata.query_page_over
#print axioms Strata.query_wrong_cursor
#print axioms Strata.query_scan_limit
#print axioms Strata.queryAll_system
#print axioms Strata.filterMap_fst_sublist
#print axioms Strata.scanRows_ids_nodup
#print axioms Strata.queryAll_ids_nodup
#print axioms Strata.query_pages_nodup
#print axioms Strata.readRecords_hide
#print axioms Strata.queryPage_system_rows

/-! ## Slice 10: the order on values as the kernel's, and the executable read -/
#print axioms Strata.Elem.leb_total
#print axioms Strata.Elem.leb_trans
#print axioms Strata.lexLe_total
#print axioms Strata.lexLe_trans
#print axioms Strata.Impl.slotRows_eq
#print axioms Strata.Impl.ITenant.recordsOf_abs
#print axioms Strata.Impl.ITenant.canRead_abs
#print axioms Strata.Impl.ITenant.rowOfI_abs
#print axioms Strata.Impl.readRecordsI_abs
#print axioms Strata.Impl.ITenant.scanRowsI_abs
#print axioms Strata.Impl.ITenant.queryAllI_abs
#print axioms Strata.Impl.ITenant.countI_abs
#print axioms Strata.Impl.queryPageI_abs
#print axioms Strata.Impl.readI_refines
#print axioms Strata.Impl.readX_refines

/-! ## Slice 10, second half: search and aggregates (`Strata/Search.lean`, `Strata/Aggregates.lean`) -/

namespace Strata

/-- A hit is sound for a query and a reader: it is a stored record of its type that the reader may read, given
    as the reader receives it (redacted), and its matched fields are shown ones that match every query word. -/
def SoundHit (src : Src) (rs : FieldRules) (p : Principal) (q : List String) (h : SearchHit) : Prop :=
  ∃ rec, (h.id, rec) ∈ src.recs h.ty ∧ src.canRead p h.ty rec = true ∧
    h.record = redact rs p h.ty rec ∧ h.matched ≠ [] ∧
    ∀ f ∈ h.matched, (∃ v, (f, v) ∈ h.record.fields) ∧
      allMatch q (textTokens (Fields.get h.record.fields f)) = true

theorem matchedIn_mem (q : List String) (fs : List FieldId) (r : Record) (f : FieldId)
    (h : f ∈ matchedIn q fs r) : f ∈ fs := by
  unfold matchedIn at h
  exact (List.mem_filter.mp h).1

theorem matchedIn_match (q : List String) (fs : List FieldId) (r : Record) (f : FieldId)
    (h : f ∈ matchedIn q fs r) : allMatch q (textTokens (Fields.get r.fields f)) = true := by
  unfold matchedIn at h
  have := (List.mem_filter.mp h).2
  simpa using this

theorem candidatesOf_mem (src : Src) (ty : TypeId) (q : List String) (probes : List FieldId) (c : Cand)
    (hc : c ∈ candidatesOf src ty q probes) : (c.id, c.record) ∈ src.recs ty := by
  unfold candidatesOf at hc
  obtain ⟨e, he, hf⟩ := List.mem_filterMap.mp hc
  have he' : e ∈ src.recs ty := List.mem_mergeSort.mp he
  by_cases hm : (matchedIn q probes e.2).isEmpty = true
  · simp [hm] at hf
  · simp only [hm, Bool.false_eq_true, if_false, Option.some.injEq] at hf
    subst hf
    exact he'

theorem decideCand_sound (src : Src) (rs : FieldRules) (p : Principal) (ty : TypeId) (q : List String)
    (probes : List FieldId) (c : Cand) (h : SearchHit) (hc : (c.id, c.record) ∈ src.recs ty)
    (hd : decideCand src rs p ty q probes c = some h) : SoundHit src rs p q h := by
  unfold decideCand at hd
  dsimp only at hd
  split at hd
  · cases hd
  · rename_i hcan
    have hv : src.canRead p ty c.record = true := by simpa using hcan
    split at hd
    · cases hd
    · rename_i hne
      have hh := Option.some.inj hd
      subst hh
      refine ⟨c.record, ?_, hv, rfl, ?_, ?_⟩
      · exact hc
      · intro h0
        apply hne
        rw [List.isEmpty_iff]
        exact h0
      · intro f hf
        refine ⟨?_, matchedIn_match _ _ _ _ hf⟩
        have hmem := matchedIn_mem _ _ _ _ hf
        have hfilt := (List.mem_filter.mp hmem).2
        simp only [Bool.and_eq_true] at hfilt
        obtain ⟨fv, hfv, hfe⟩ := List.any_eq_true.mp hfilt.2
        have hfe' : fv.1 = f := beq_iff_eq.mp hfe
        subst hfe'
        exact ⟨fv.2, by simpa using hfv⟩

theorem decideRun_sound (S : SearchHit → Prop) (ev : Cand → Option SearchHit) :
    ∀ (cs : List Cand) (room : Nat), (∀ c ∈ cs, ∀ h, ev c = some h → S h) →
      ∀ h ∈ (decideRun ev room cs).1, S h
  | [], _, _, h, hh => by simp [decideRun] at hh
  | c :: cs, room, hev, h, hh => by
    unfold decideRun at hh
    rcases hc : ev c with _ | h0
    · simp only [hc] at hh
      exact decideRun_sound S ev cs room (fun c' hc' h' he' => hev c' (List.mem_cons_of_mem _ hc') h' he') h hh
    · simp only [hc] at hh
      have hS := hev c List.mem_cons_self h0 hc
      by_cases hr : room ≤ 1
      · simp only [hr, ite_true] at hh
        simp at hh
        subst hh
        exact hS
      · simp only [hr, ite_false] at hh
        rcases List.mem_cons.mp hh with h1 | h2
        · subst h1; exact hS
        · exact decideRun_sound S ev cs (room - 1)
            (fun c' hc' h' he' => hev c' (List.mem_cons_of_mem _ hc') h' he') h h2

theorem hits_append_sound (S : SearchHit → Prop) (acc hs : List SearchHit)
    (h1 : ∀ h ∈ acc, S h) (h2 : ∀ h ∈ hs, S h) : ∀ h ∈ acc ++ hs, S h := by
  intro h hh
  rcases List.mem_append.mp hh with h' | h'
  · exact h1 h h'
  · exact h2 h h'

theorem probeType_sound (S : SearchHit → Prop) (size : Nat) (ty : TypeId) (ev : Cand → Option SearchHit) :
    ∀ (fuel : Nat) (acc : SearchAcc) (rest : List Cand),
      (∀ h ∈ acc.hits, S h) → (∀ c ∈ rest, ∀ h, ev c = some h → S h) →
      ∀ h ∈ (probeType size ty ev fuel acc rest).hits, S h
  | 0, acc, rest, hacc, _, h, hh => by
    simp only [probeType] at hh
    exact hacc h hh
  | fuel + 1, acc, rest, hacc, hrest, h, hh => by
    unfold probeType at hh
    dsimp only at hh
    by_cases hs0 : acc.stop = true
    · simp only [hs0, ite_true] at hh
      exact hacc h hh
    · simp only [hs0, Bool.false_eq_true, ite_false] at hh
      by_cases hb : maxCandidates ≤ acc.examined
      · simp only [hb, ite_true] at hh
        exact hacc h hh
      · simp only [hb, ite_false] at hh
        by_cases hf : (rest.take (min candidatePage (maxCandidates - acc.examined))).isEmpty = true
        · simp only [hf, ite_true] at hh
          exact hacc h hh
        · simp only [hf, Bool.false_eq_true, ite_false] at hh
          have hfound : ∀ c ∈ rest.take (min candidatePage (maxCandidates - acc.examined)),
              ∀ h', ev c = some h' → S h' :=
            fun c hc h' he => hrest c (List.mem_of_mem_take hc) h' he
          rcases hdr : decideRun ev (size - acc.hits.length)
              (rest.take (min candidatePage (maxCandidates - acc.examined))) with ⟨hs, consumed⟩
          have hsound : ∀ h' ∈ hs, S h' := by
            intro h' hh'
            have hm : h' ∈ (decideRun ev (size - acc.hits.length)
                (rest.take (min candidatePage (maxCandidates - acc.examined)))).1 := by
              rw [hdr]; exact hh'
            exact decideRun_sound S ev _ _ hfound h' hm
          rw [hdr] at hh
          dsimp only at hh
          by_cases hfull : size ≤ (acc.hits ++ hs).length
          · simp only [hfull, ite_true] at hh
            exact hits_append_sound S acc.hits hs hacc hsound h hh
          · simp only [hfull, ite_false] at hh
            by_cases hl : (rest.take (min candidatePage (maxCandidates - acc.examined))).length <
                min candidatePage (maxCandidates - acc.examined)
            · simp only [hl, ite_true] at hh
              exact hits_append_sound S acc.hits hs hacc hsound h hh
            · simp only [hl, ite_false] at hh
              exact probeType_sound S size ty ev fuel _ _
                (hits_append_sound S acc.hits hs hacc hsound)
                (fun c hc => hrest c (List.mem_of_mem_drop hc)) h hh

theorem searchTypes_sound (src : Src) (rs : FieldRules) (p : Principal) (q : List String) (size : Nat) :
    ∀ (plan : List (TypeId × List FieldId)) (acc : SearchAcc),
      (∀ h ∈ acc.hits, SoundHit src rs p q h) →
      ∀ h ∈ (searchTypes src rs p q size plan acc).hits, SoundHit src rs p q h
  | [], acc, hacc, h, hh => by
    simp only [searchTypes] at hh
    exact hacc h hh
  | (ty, probes) :: more, acc, hacc, h, hh => by
    unfold searchTypes at hh
    dsimp only at hh
    by_cases hs0 : acc.stop = true
    · simp only [hs0, ite_true] at hh
      exact hacc h hh
    · simp only [hs0, Bool.false_eq_true, ite_false] at hh
      have hacc' := probeType_sound (SoundHit src rs p q) size ty
        (decideCand src rs p ty q probes) ((candidatesOf src ty q probes).length + 1) acc
        (candidatesOf src ty q probes) hacc
        (fun c hc h' hd => decideCand_sound src rs p ty q probes c h'
          (candidatesOf_mem src ty q probes c hc) hd)
      exact searchTypes_sound src rs p q size more _ hacc' h hh

theorem searchOn_sound (src : Src) (rs : FieldRules) (p : Principal) (q : List String) (size : Nat)
    (plan : List (TypeId × List FieldId)) :
    ∀ h ∈ (searchOn src rs p q size plan).hits, SoundHit src rs p q h := by
  intro h hh
  unfold searchOn at hh
  exact searchTypes_sound src rs p q size plan {} (fun h' hh' => by simp at hh') h hh

theorem searchPlan_query (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq)
    (q : List String) (size : Nat) (plan : List (TypeId × List FieldId))
    (h : searchPlan t spec p req = .ok (q, size, plan)) : q = tokensOf req.query := by
  unfold searchPlan at h
  dsimp only at h
  repeat' (split at h)
  all_goals (try simp_all)

/-- Every hit of a search is sound: a stored record the reader may read, given redacted, whose matched fields
    are shown and match the query. -/
theorem search_sound (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq) (r : SearchResult)
    (hr : search t spec p req = .ok r) :
    ∀ h ∈ r.hits, SoundHit (Src.ofTenant t) spec.rules p (tokensOf req.query) h := by
  unfold search at hr
  cases hpl : searchPlan t spec p req with
  | error e => rw [hpl] at hr; cases hr
  | ok v =>
    rw [hpl] at hr
    obtain ⟨q, size, plan⟩ := v
    have hq := searchPlan_query t spec p req q size plan hpl
    have hr' := Except.ok.inj hr
    subst hr'
    subst hq
    exact searchOn_sound _ _ _ _ _ _

/-! Refusals of a search, in the kernel's order -/

theorem search_query_too_long (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq)
    (h : maxQueryBytes < req.query.utf8ByteSize) :
    search t spec p req = .error .invalidIntent := by
  unfold search searchPlan
  simp [h]

theorem search_query_no_word (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq)
    (h1 : ¬ maxQueryBytes < req.query.utf8ByteSize) (h2 : tokensOf req.query = []) :
    search t spec p req = .error .invalidIntent := by
  unfold search searchPlan
  simp [h1, h2]

theorem search_query_too_many_words (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq)
    (h1 : ¬ maxQueryBytes < req.query.utf8ByteSize) (h2 : ¬ tokensOf req.query = [])
    (h3 : maxQueryTokens < (tokensOf req.query).length) :
    search t spec p req = .error .invalidIntent := by
  unfold search searchPlan
  simp [h1, h2, h3]

theorem search_page_out_of_range (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq)
    (h1 : ¬ maxQueryBytes < req.query.utf8ByteSize) (h2 : ¬ tokensOf req.query = [])
    (h3 : ¬ maxQueryTokens < (tokensOf req.query).length)
    (h4 : req.page.getD defaultPage = 0 ∨ maxPage < req.page.getD defaultPage) :
    search t spec p req = .error .invalidIntent := by
  unfold search searchPlan
  simp [h1, h2, h3, h4]

theorem search_unknown_object (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq)
    (h1 : ¬ maxQueryBytes < req.query.utf8ByteSize) (h2 : ¬ tokensOf req.query = [])
    (h3 : ¬ maxQueryTokens < (tokensOf req.query).length)
    (h4 : ¬ (req.page.getD defaultPage = 0 ∨ maxPage < req.page.getD defaultPage))
    (h5 : req.objects.any (fun ty => (t.typ? ty).isNone) = true) :
    search t spec p req = .error .unknownObject := by
  unfold search searchPlan
  simp [h1, h2, h3, h4, h5]

/-! The candidates a search examines -/

theorem decideRun_consumed_le (ev : Cand → Option SearchHit) :
    ∀ (cs : List Cand) (room : Nat), (decideRun ev room cs).2 ≤ cs.length
  | [], _ => by simp [decideRun]
  | c :: cs, room => by
    have ih := decideRun_consumed_le ev cs
    unfold decideRun
    rcases hc : ev c with _ | h0
    · simp only [hc]
      have := ih room
      simp only [List.length_cons]
      omega
    · simp only [hc]
      by_cases hr : room ≤ 1
      · simp [hr]
      · simp only [hr, ite_false]
        have := ih (room - 1)
        simp only [List.length_cons]
        omega

theorem probeType_examined_le (size : Nat) (ty : TypeId) (ev : Cand → Option SearchHit) :
    ∀ (fuel : Nat) (acc : SearchAcc) (rest : List Cand), acc.examined ≤ maxCandidates →
      (probeType size ty ev fuel acc rest).examined ≤ maxCandidates
  | 0, acc, rest, hacc => by
    simp only [probeType]
    exact hacc
  | fuel + 1, acc, rest, hacc => by
    unfold probeType
    dsimp only
    by_cases hs0 : acc.stop = true
    · simp only [hs0, ite_true]
      exact hacc
    · simp only [hs0, Bool.false_eq_true, ite_false]
      by_cases hb : maxCandidates ≤ acc.examined
      · simp only [hb, ite_true]
        exact hacc
      · simp only [hb, ite_false]
        by_cases hf : (rest.take (min candidatePage (maxCandidates - acc.examined))).isEmpty = true
        · simp only [hf, ite_true]
          exact hacc
        · simp only [hf, Bool.false_eq_true, ite_false]
          have hlen := List.length_take_le (min candidatePage (maxCandidates - acc.examined)) rest
          have hcons := decideRun_consumed_le ev (rest.take (min candidatePage (maxCandidates - acc.examined)))
            (size - acc.hits.length)
          have hmin := Nat.min_le_right candidatePage (maxCandidates - acc.examined)
          rcases hdr : decideRun ev (size - acc.hits.length)
              (rest.take (min candidatePage (maxCandidates - acc.examined))) with ⟨hs, consumed⟩
          rw [hdr] at hcons
          dsimp only at hcons ⊢
          have hcon : consumed ≤ maxCandidates - acc.examined := by
            have := hcons
            omega
          by_cases hfull : size ≤ (acc.hits ++ hs).length
          · simp only [hfull, ite_true]
            omega
          · simp only [hfull, ite_false]
            by_cases hl : (rest.take (min candidatePage (maxCandidates - acc.examined))).length <
                min candidatePage (maxCandidates - acc.examined)
            · simp only [hl, ite_true]
              omega
            · simp only [hl, ite_false]
              exact probeType_examined_le size ty ev fuel _ _ (by
                show acc.examined + consumed ≤ maxCandidates
                omega)

theorem searchTypes_examined_le (src : Src) (rs : FieldRules) (p : Principal) (q : List String) (size : Nat) :
    ∀ (plan : List (TypeId × List FieldId)) (acc : SearchAcc), acc.examined ≤ maxCandidates →
      (searchTypes src rs p q size plan acc).examined ≤ maxCandidates
  | [], acc, hacc => by
    simp only [searchTypes]
    exact hacc
  | (ty, probes) :: more, acc, hacc => by
    unfold searchTypes
    dsimp only
    by_cases hs0 : acc.stop = true
    · simp only [hs0, ite_true]
      exact hacc
    · simp only [hs0, Bool.false_eq_true, ite_false]
      exact searchTypes_examined_le src rs p q size more _
        (probeType_examined_le size ty (decideCand src rs p ty q probes)
          ((candidatesOf src ty q probes).length + 1) acc (candidatesOf src ty q probes) hacc)

/-- A search examines at most `maxCandidates` (1,000) candidates, whatever the reader may read of them. -/
theorem search_examined_le (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq) (r : SearchResult)
    (hr : search t spec p req = .ok r) : r.examined ≤ maxCandidates := by
  unfold search at hr
  cases hpl : searchPlan t spec p req with
  | error e => rw [hpl] at hr; cases hr
  | ok v =>
    rw [hpl] at hr
    obtain ⟨q, size, plan⟩ := v
    have hr' := Except.ok.inj hr
    subst hr'
    unfold searchOn
    exact searchTypes_examined_le _ _ _ _ _ _ _ (by simp)

/-- Hits in type order, then record id order. -/
def hitLt (a b : SearchHit) : Prop := a.ty < b.ty ∨ (a.ty = b.ty ∧ a.id < b.id)

theorem decideRun_none_eq (ev : Cand → Option SearchHit) (c : Cand) (cs : List Cand) (room : Nat)
    (h : ev c = none) :
    decideRun ev room (c :: cs) = ((decideRun ev room cs).1, (decideRun ev room cs).2 + 1) := by
  rw [decideRun, h]

theorem decideRun_some_last_eq (ev : Cand → Option SearchHit) (c : Cand) (cs : List Cand) (room : Nat)
    (h0 : SearchHit) (h : ev c = some h0) (hr : room ≤ 1) :
    decideRun ev room (c :: cs) = ([h0], 1) := by
  rw [decideRun, h]
  dsimp only
  rw [if_pos hr]

theorem decideRun_some_more_eq (ev : Cand → Option SearchHit) (c : Cand) (cs : List Cand) (room : Nat)
    (h0 : SearchHit) (h : ev c = some h0) (hr : ¬ room ≤ 1) :
    decideRun ev room (c :: cs) =
      (h0 :: (decideRun ev (room - 1) cs).1, (decideRun ev (room - 1) cs).2 + 1) := by
  rw [decideRun, h]
  dsimp only
  rw [if_neg hr]

theorem decideRun_prefix (ev : Cand → Option SearchHit) :
    ∀ (cs : List Cand) (room : Nat), ∀ h ∈ (decideRun ev room cs).1,
      ∃ d ∈ cs.take (decideRun ev room cs).2, ev d = some h
  | [], _, h, hh => by simp [decideRun] at hh
  | c :: cs, room, h, hh => by
    rcases hc : ev c with _ | h0
    · rw [decideRun_none_eq ev c cs room hc] at hh ⊢
      obtain ⟨d, hd, hed⟩ := decideRun_prefix ev cs room h hh
      exact ⟨d, by simpa [List.take_succ_cons] using List.mem_cons_of_mem _ hd, hed⟩
    · by_cases hr : room ≤ 1
      · rw [decideRun_some_last_eq ev c cs room h0 hc hr] at hh ⊢
        simp at hh
        subst hh
        exact ⟨c, by simp, hc⟩
      · rw [decideRun_some_more_eq ev c cs room h0 hc hr] at hh ⊢
        rcases List.mem_cons.mp hh with h1 | h2
        · subst h1
          exact ⟨c, by simp, hc⟩
        · obtain ⟨d, hd, hed⟩ := decideRun_prefix ev cs (room - 1) h h2
          exact ⟨d, by simpa [List.take_succ_cons] using List.mem_cons_of_mem _ hd, hed⟩

theorem decideRun_order (ev : Cand → Option SearchHit) (ty : TypeId)
    (hev : ∀ c h, ev c = some h → h.ty = ty ∧ h.id = c.id) :
    ∀ (cs : List Cand) (room : Nat), cs.Pairwise (fun c d => c.id < d.id) →
      (decideRun ev room cs).1.Pairwise hitLt
  | [], _, _ => by simp [decideRun]
  | c :: cs, room, hp => by
    have hp' := List.pairwise_cons.mp hp
    have ih := decideRun_order ev ty hev cs
    rcases hc : ev c with _ | h0
    · rw [decideRun_none_eq ev c cs room hc]
      exact ih room hp'.2
    · have hh0 := hev c h0 hc
      by_cases hr : room ≤ 1
      · rw [decideRun_some_last_eq ev c cs room h0 hc hr]
        simp
      · rw [decideRun_some_more_eq ev c cs room h0 hc hr]
        apply List.Pairwise.cons
        · intro h' hh'
          obtain ⟨d, hd, hed⟩ := decideRun_prefix ev cs (room - 1) h' hh'
          have hd2 := hev d h' hed
          have hdc : c.id < d.id := hp'.1 d (List.mem_of_mem_take hd)
          right
          exact ⟨by rw [hh0.1, hd2.1], by rw [hh0.2, hd2.2]; exact hdc⟩
        · exact ih (room - 1) hp'.2

theorem decideCand_ty (src : Src) (rs : FieldRules) (p : Principal) (ty : TypeId) (q : List String)
    (probes : List FieldId) (c : Cand) (h : SearchHit) (hd : decideCand src rs p ty q probes c = some h) :
    h.ty = ty := by
  unfold decideCand at hd
  dsimp only at hd
  split at hd
  · cases hd
  · split at hd
    · cases hd
    · have hh := Option.some.inj hd
      subst hh
      rfl

theorem sorted_recs_strict (l : List (RecordId × Record)) (hnd : (l.map Prod.fst).Nodup)
    (hle : l.Pairwise (fun a b => a.1 ≤ b.1)) : l.Pairwise (fun a b => a.1 < b.1) := by
  have hne : l.Pairwise (fun a b => a.1 ≠ b.1) := List.pairwise_map.mp hnd
  exact (List.Pairwise.and hle hne).imp (fun ⟨h1, h2⟩ => Nat.lt_of_le_of_ne h1 h2)

/-- The records of a type, sorted by id, as a list with distinct ids: the order a search reads them in. -/
theorem mergeSort_recs_sorted (l : Records) :
    (l.mergeSort (fun a b => decide (a.1 ≤ b.1))).Pairwise (fun a b => a.1 ≤ b.1) := by
  have key := List.pairwise_mergeSort (le := fun (a b : RecordId × Record) => decide (a.1 ≤ b.1))
    (fun a b c h1 h2 => by
      simp only [decide_eq_true_eq] at h1 h2 ⊢
      exact Nat.le_trans h1 h2)
    (fun a b => by
      cases Nat.le_total a.1 b.1 <;> simp_all)
    l
  exact key.imp (fun h => by simpa using h)

theorem candidatesOf_sorted (src : Src) (ty : TypeId) (q : List String) (probes : List FieldId)
    (hnd : ((src.recs ty).map Prod.fst).Nodup) :
    (candidatesOf src ty q probes).Pairwise (fun c d => c.id < d.id) := by
  unfold candidatesOf
  have hs : ((src.recs ty).mergeSort (fun a b => decide (a.1 ≤ b.1))).Pairwise (fun a b => a.1 < b.1) :=
    sorted_recs_strict _ (((List.mergeSort_perm _ _).map Prod.fst).nodup_iff.mpr hnd)
      (mergeSort_recs_sorted _)
  refine List.Pairwise.filterMap _ (fun a b hab b' hb' b'' hb'' => ?_) hs
  cases hfa : matchedIn q probes a.2 with
  | nil => simp [hfa] at hb'
  | cons _ _ =>
    cases hfb : matchedIn q probes b.2 with
    | nil => simp [hfb] at hb''
    | cons _ _ =>
      simp [hfa] at hb'
      simp [hfb] at hb''
      subst hb' hb''
      exact hab

theorem take_take_le {α : Type} (l : List α) (n m : Nat) (h : m ≤ n) :
    (l.take n).take m = l.take m := by
  rw [List.take_take, Nat.min_eq_left h]

theorem take_lt_drop (rest : List Cand) (hs : rest.Pairwise (fun c d => c.id < d.id)) (n : Nat) :
    ∀ d ∈ rest.take n, ∀ c ∈ rest.drop n, d.id < c.id := by
  have hsplit : (rest.take n ++ rest.drop n).Pairwise (fun c d => c.id < d.id) := by
    rw [List.take_append_drop]
    exact hs
  have hp := List.pairwise_append.mp hsplit
  exact fun d hd c hc => hp.2.2 d hd c hc

theorem probeType_order (size : Nat) (ty : TypeId) (ev : Cand → Option SearchHit)
    (hev : ∀ c h, ev c = some h → h.ty = ty ∧ h.id = c.id) :
    ∀ (fuel : Nat) (acc : SearchAcc) (rest : List Cand),
      acc.hits.Pairwise hitLt →
      (∀ h ∈ acc.hits, h.ty < ty ∨ (h.ty = ty ∧ ∀ c ∈ rest, h.id < c.id)) →
      rest.Pairwise (fun c d => c.id < d.id) →
      (probeType size ty ev fuel acc rest).hits.Pairwise hitLt := by
  intro fuel
  induction fuel with
  | zero =>
    intro acc rest hacc _ _
    simp only [probeType]
    exact hacc
  | succ fuel ih =>
    intro acc rest hacc hbound hsorted
    unfold probeType
    dsimp only
    by_cases hs0 : acc.stop = true
    · simp only [hs0, ite_true]
      exact hacc
    · simp only [hs0, Bool.false_eq_true, ite_false]
      by_cases hb : maxCandidates ≤ acc.examined
      · simp only [hb, ite_true]
        exact hacc
      · simp only [hb, ite_false]
        by_cases hf : (rest.take (min candidatePage (maxCandidates - acc.examined))).isEmpty = true
        · simp only [hf, ite_true]
          exact hacc
        · simp only [hf, Bool.false_eq_true, ite_false]
          have hfsorted : (rest.take (min candidatePage (maxCandidates - acc.examined))).Pairwise
              (fun c d => c.id < d.id) := hsorted.sublist (List.take_sublist _ _)
          rcases hdr : decideRun ev (size - acc.hits.length)
              (rest.take (min candidatePage (maxCandidates - acc.examined))) with ⟨hs, consumed⟩
          have hsp : hs.Pairwise hitLt := by
            have := decideRun_order ev ty hev (rest.take (min candidatePage (maxCandidates - acc.examined)))
              (size - acc.hits.length) hfsorted
            rw [hdr] at this
            exact this
          have hcl : consumed ≤ min candidatePage (maxCandidates - acc.examined) := by
            have := decideRun_consumed_le ev (rest.take (min candidatePage (maxCandidates - acc.examined)))
              (size - acc.hits.length)
            rw [hdr] at this
            have hlen := List.length_take_le (min candidatePage (maxCandidates - acc.examined)) rest
            simp only at this
            omega
          have hcross : ∀ a ∈ acc.hits, ∀ b ∈ hs, hitLt a b := by
            intro a ha b hbm
            have hbmem : b ∈ (decideRun ev (size - acc.hits.length)
                (rest.take (min candidatePage (maxCandidates - acc.examined)))).1 := by
              rw [hdr]
              exact hbm
            obtain ⟨d, hd, hed⟩ := decideRun_prefix ev _ _ b hbmem
            have hdrest : d ∈ rest := List.mem_of_mem_take (List.mem_of_mem_take hd)
            have hbt := hev d b hed
            rcases hbound a ha with h | h
            · left
              rw [hbt.1]
              exact h
            · right
              exact ⟨by rw [hbt.1, h.1], by rw [hbt.2]; exact h.2 d hdrest⟩
          have hacc' : (acc.hits ++ hs).Pairwise hitLt := List.pairwise_append.mpr ⟨hacc, hsp, hcross⟩
          by_cases hfull : size ≤ (acc.hits ++ hs).length
          · dsimp only
            simp only [hfull, ite_true]
            exact hacc'
          · dsimp only
            simp only [hfull, ite_false]
            by_cases hl : (rest.take (min candidatePage (maxCandidates - acc.examined))).length <
                min candidatePage (maxCandidates - acc.examined)
            · simp only [hl, ite_true]
              exact hacc'
            · simp only [hl, ite_false]
              have hbound'' : ∀ h ∈ acc.hits ++ hs, h.ty < ty ∨
                  (h.ty = ty ∧ ∀ c ∈ rest.drop consumed, h.id < c.id) := by
                intro h hh
                rcases List.mem_append.mp hh with ha | hbm
                · rcases hbound h ha with h1 | h1
                  · exact Or.inl h1
                  · exact Or.inr ⟨h1.1, fun c hc => h1.2 c (List.mem_of_mem_drop hc)⟩
                · have hbmem : h ∈ (decideRun ev (size - acc.hits.length)
                      (rest.take (min candidatePage (maxCandidates - acc.examined)))).1 := by
                    rw [hdr]
                    exact hbm
                  obtain ⟨d, hd, hed⟩ := decideRun_prefix ev _ _ h hbmem
                  have hd2 : d ∈ (rest.take (min candidatePage (maxCandidates - acc.examined))).take consumed := by
                    have hcopy := hd
                    rw [hdr] at hcopy
                    exact hcopy
                  rw [take_take_le rest _ consumed hcl] at hd2
                  have hbt := hev d h hed
                  exact Or.inr ⟨hbt.1, fun c hc => by
                    rw [hbt.2]
                    exact take_lt_drop rest hsorted consumed d hd2 c hc⟩
              exact ih _ (rest.drop consumed) hacc' hbound'' (hsorted.sublist (List.drop_sublist _ _))

theorem decideCand_id (src : Src) (rs : FieldRules) (p : Principal) (ty : TypeId) (q : List String)
    (probes : List FieldId) (c : Cand) (h : SearchHit) (hd : decideCand src rs p ty q probes c = some h) :
    h.id = c.id := by
  unfold decideCand at hd
  dsimp only at hd
  split at hd
  · cases hd
  · split at hd
    · cases hd
    · have hh := Option.some.inj hd
      subst hh
      rfl

theorem searchTypes_order (src : Src) (rs : FieldRules) (p : Principal) (q : List String) (size : Nat)
    (hnd : ∀ ty, ((src.recs ty).map Prod.fst).Nodup) :
    ∀ (plan : List (TypeId × List FieldId)) (acc : SearchAcc),
      plan.Pairwise (fun a b => a.1 < b.1) → acc.hits.Pairwise hitLt →
      (∀ h ∈ acc.hits, ∀ e ∈ plan, h.ty < e.1) →
      (searchTypes src rs p q size plan acc).hits.Pairwise hitLt
  | [], acc, _, hacc, _ => by
    simp only [searchTypes]
    exact hacc
  | (ty, probes) :: more, acc, hplan, hacc, hbound => by
    have hpl := List.pairwise_cons.mp hplan
    unfold searchTypes
    dsimp only
    by_cases hs0 : acc.stop = true
    · simp only [hs0, ite_true]
      exact hacc
    · simp only [hs0, Bool.false_eq_true, ite_false]
      have hpt := probeType_order size ty (decideCand src rs p ty q probes)
        (fun c h hd => ⟨decideCand_ty src rs p ty q probes c h hd, decideCand_id src rs p ty q probes c h hd⟩)
        ((candidatesOf src ty q probes).length + 1) acc (candidatesOf src ty q probes) hacc
        (fun h hh => Or.inl (hbound h hh (ty, probes) List.mem_cons_self))
        (candidatesOf_sorted src ty q probes (hnd ty))
      have hsome : ∀ h ∈ (probeType size ty (decideCand src rs p ty q probes)
          ((candidatesOf src ty q probes).length + 1) acc (candidatesOf src ty q probes)).hits,
          h ∈ acc.hits ∨ h.ty = ty := by
        intro h hh
        exact probeType_sound (fun h => h ∈ acc.hits ∨ h.ty = ty) size ty _
          ((candidatesOf src ty q probes).length + 1) acc (candidatesOf src ty q probes)
          (fun h hh => Or.inl hh)
          (fun c _ h hd => Or.inr (decideCand_ty src rs p ty q probes c h hd)) h hh
      exact searchTypes_order src rs p q size hnd more _ hpl.2 hpt (by
        intro h hh e he
        rcases hsome h hh with h1 | h1
        · exact Nat.lt_trans (hbound h h1 (ty, probes) List.mem_cons_self) (hpl.1 e he)
        · rw [h1]
          exact hpl.1 e he)

/-- The object types of a plan are in ascending order, when their ids are distinct. -/
theorem types_sorted_strict (types : List (TypeId × ObjType)) (hnd : (types.map Prod.fst).Nodup) :
    (types.mergeSort (fun a b => decide (a.1 ≤ b.1))).Pairwise (fun a b => a.1 < b.1) := by
  have hle : (types.mergeSort (fun a b => decide (a.1 ≤ b.1))).Pairwise (fun a b => a.1 ≤ b.1) := by
    have key := List.pairwise_mergeSort (le := fun (a b : TypeId × ObjType) => decide (a.1 ≤ b.1))
      (fun a b c h1 h2 => by
        simp only [decide_eq_true_eq] at h1 h2 ⊢
        exact Nat.le_trans h1 h2)
      (fun a b => by
        cases Nat.le_total a.1 b.1 <;> simp_all)
      types
    exact key.imp (fun h => by simpa using h)
  have hne : (types.mergeSort (fun a b => decide (a.1 ≤ b.1))).Pairwise (fun a b => a.1 ≠ b.1) :=
    List.pairwise_map.mp (((List.mergeSort_perm _ _).map Prod.fst).nodup_iff.mpr hnd)
  exact (List.Pairwise.and hle hne).imp (fun ⟨h1, h2⟩ => Nat.lt_of_le_of_ne h1 h2)

theorem searchPlan_sorted (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq)
    (q : List String) (size : Nat) (plan : List (TypeId × List FieldId))
    (hkeys : (t.types.map Prod.fst).Nodup)
    (h : searchPlan t spec p req = .ok (q, size, plan)) :
    plan.Pairwise (fun a b => a.1 < b.1) := by
  unfold searchPlan at h
  dsimp only at h
  by_cases c1 : maxQueryBytes < req.query.utf8ByteSize
  · simp only [c1, ite_true] at h
    cases h
  simp only [c1, ite_false] at h
  by_cases c2 : tokensOf req.query = []
  · simp only [c2, ite_true] at h
    cases h
  simp only [c2, ite_false] at h
  by_cases c3 : maxQueryTokens < (tokensOf req.query).length
  · simp only [c3, ite_true] at h
    cases h
  simp only [c3, ite_false] at h
  by_cases c4 : req.page.getD defaultPage = 0 ∨ maxPage < req.page.getD defaultPage
  · simp only [c4, ite_true] at h
    cases h
  simp only [c4, ite_false] at h
  by_cases c5 : req.objects.any (fun ty => (t.typ? ty).isNone) = true
  · simp only [c5, ite_true] at h
    cases h
  simp only [c5, ite_false] at h
  have hh := Except.ok.inj h
  simp only [Prod.mk.injEq] at hh
  rw [← hh.2.2]
  refine List.Pairwise.filterMap _ (fun a b hab b' hb' b'' hb'' => ?_) (types_sorted_strict t.types hkeys)
  repeat' (split at hb')
  repeat' (split at hb'')
  all_goals (try simp only [Option.mem_def, Option.some.injEq] at hb' hb'')
  all_goals (try subst hb')
  all_goals (try subst hb'')
  all_goals (try exact hab)
  all_goals (try simp_all)

theorem searchOn_order (src : Src) (rs : FieldRules) (p : Principal) (q : List String) (size : Nat)
    (plan : List (TypeId × List FieldId)) (hplan : plan.Pairwise (fun a b => a.1 < b.1))
    (hnd : ∀ ty, ((src.recs ty).map Prod.fst).Nodup) :
    (searchOn src rs p q size plan).hits.Pairwise hitLt := by
  unfold searchOn
  exact searchTypes_order src rs p q size hnd plan {} hplan List.Pairwise.nil
    (fun h hh => absurd hh (by simp))

/-- The hits of a search are in ascending type, then record id order. -/
theorem search_order (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq) (r : SearchResult)
    (hkeys : (t.types.map Prod.fst).Nodup) (hids : ∀ ty, ((t.recordsOf ty).map Prod.fst).Nodup)
    (hr : search t spec p req = .ok r) : r.hits.Pairwise hitLt := by
  unfold search at hr
  cases hpl : searchPlan t spec p req with
  | error e =>
    rw [hpl] at hr
    cases hr
  | ok v =>
    rw [hpl] at hr
    obtain ⟨q, size, plan⟩ := v
    have hr' := Except.ok.inj hr
    subst hr'
    exact searchOn_order _ _ _ _ _ _ (searchPlan_sorted t spec p req q size plan hkeys hpl) hids

/-- No record is a hit twice: the hits, as type and id pairs, have no repeats. -/
theorem search_no_repeat (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq) (r : SearchResult)
    (hkeys : (t.types.map Prod.fst).Nodup) (hids : ∀ ty, ((t.recordsOf ty).map Prod.fst).Nodup)
    (hr : search t spec p req = .ok r) : (r.hits.map (fun h => (h.ty, h.id))).Nodup := by
  have hp := search_order t spec p req r hkeys hids hr
  unfold List.Nodup
  rw [List.pairwise_map]
  refine hp.imp (fun {a b} hab heq => ?_)
  have h1 := Prod.mk.inj heq
  rcases hab with h2 | h2
  · rw [h1.1] at h2
    exact Nat.lt_irrefl _ h2
  · rw [h1.2] at h2
    exact Nat.lt_irrefl _ h2.2

end Strata

namespace Strata
/-! ## Aggregate refusals -/

theorem aggregate_unknown_query (t : Tenant) (spec : ReadSpec) (p : Principal) (req : AggReq)
    (h : spec.queries.lookup req.query = none) :
    aggregate t spec p req = .error .unknownQuery := by
  unfold aggregate
  simp only [h]

theorem aggregate_unknown_aggregate (t : Tenant) (spec : ReadSpec) (p : Principal) (req : AggReq)
    (q : QueryDef) (hq : spec.queries.lookup req.query = some q)
    (ha : q.aggregates.lookup req.aggregate = none) :
    aggregate t spec p req = .error .unknownQuery := by
  unfold aggregate
  simp only [hq, ha]

theorem aggregate_undeclared_param (t : Tenant) (spec : ReadSpec) (p : Principal) (req : AggReq)
    (q : QueryDef) (a : AggDef) (hq : spec.queries.lookup req.query = some q)
    (ha : q.aggregates.lookup req.aggregate = some a)
    (hp : req.params.any (fun kv => !(q.params.contains kv.1)) = true) :
    aggregate t spec p req = .error .invalidIntent := by
  unfold aggregate
  simp only [hq, ha, hp, ite_true]

theorem aggregate_unknown_object (t : Tenant) (spec : ReadSpec) (p : Principal) (req : AggReq)
    (q : QueryDef) (a : AggDef) (hq : spec.queries.lookup req.query = some q)
    (ha : q.aggregates.lookup req.aggregate = some a)
    (hp : ¬ req.params.any (fun kv => !(q.params.contains kv.1)) = true)
    (ht : t.typ? q.ty = none) :
    aggregate t spec p req = .error .unknownObject := by
  unfold aggregate
  simp only [hq, ha, hp, ite_false, ht, Bool.false_eq_true, ↓reduceIte]

theorem aggregate_undeclared_filter (t : Tenant) (spec : ReadSpec) (p : Principal) (req : AggReq)
    (q : QueryDef) (a : AggDef) (o : ObjType) (hq : spec.queries.lookup req.query = some q)
    (ha : q.aggregates.lookup req.aggregate = some a)
    (hp : ¬ req.params.any (fun kv => !(q.params.contains kv.1)) = true)
    (ht : t.typ? q.ty = some o)
    (hf : req.filter.any (fun kv => !(q.declared.contains kv.1)) = true) :
    aggregate t spec p req = .error .invalidIntent := by
  unfold aggregate aggregateOn
  simp only [hq, ha, hp, ite_false, ht, hf, ite_true, Bool.false_eq_true, ↓reduceIte]

theorem aggregate_unknown_field (t : Tenant) (spec : ReadSpec) (p : Principal) (req : AggReq)
    (q : QueryDef) (a : AggDef) (o : ObjType) (hq : spec.queries.lookup req.query = some q)
    (ha : q.aggregates.lookup req.aggregate = some a)
    (hp : ¬ req.params.any (fun kv => !(q.params.contains kv.1)) = true)
    (ht : t.typ? q.ty = some o)
    (hf : ¬ req.filter.any (fun kv => !(q.declared.contains kv.1)) = true)
    (hn : ((req.filter.map Prod.fst) ++ a.field.toList ++ a.groupBy.toList).any
      (fun f => (o.schema.lookup f).isNone) = true) :
    aggregate t spec p req = .error .unknownField := by
  unfold aggregate aggregateOn
  simp only [hq, ha, hp, ite_false, ht, hf, hn, ite_true, Bool.false_eq_true, ↓reduceIte]

theorem aggregate_scan_window (t : Tenant) (spec : ReadSpec) (p : Principal) (req : AggReq)
    (q : QueryDef) (a : AggDef) (o : ObjType) (hq : spec.queries.lookup req.query = some q)
    (ha : q.aggregates.lookup req.aggregate = some a)
    (hp : ¬ req.params.any (fun kv => !(q.params.contains kv.1)) = true)
    (ht : t.typ? q.ty = some o)
    (hf : ¬ req.filter.any (fun kv => !(q.declared.contains kv.1)) = true)
    (hn : ¬ ((req.filter.map Prod.fst) ++ a.field.toList ++ a.groupBy.toList).any
      (fun f => (o.schema.lookup f).isNone) = true)
    (hw : maxScan < (t.recordsOf q.ty).length) :
    aggregate t spec p req = .error (.limitExceeded .scanBound) := by
  unfold aggregate aggregateOn
  simp only [hq, ha, hp, ite_false, ht, hf, hn, Src.ofTenant, Bool.false_eq_true, ↓reduceIte]
  simp only [hw, ite_true, Bool.false_eq_true, ↓reduceIte]

/-! ## Redaction and the filters of queries -/

/-- The fields an expression reads from a record. -/
def fieldsRead : Expr → List FieldId
  | .lit _ => []
  | .field _ f => [f]
  | .createdBy _ => []
  | .principal => []
  | .cmp _ a b => fieldsRead a ++ fieldsRead b
  | .and a b => fieldsRead a ++ fieldsRead b
  | .or a b => fieldsRead a ++ fieldsRead b
  | .not a => fieldsRead a
  | .isNull a => fieldsRead a

theorem lookup_filter (f : FieldId) (P : FieldId × Val → Bool) :
    ∀ (fs : Fields), (∀ fv ∈ fs, fv.1 = f → P fv = true) → (fs.filter P).lookup f = fs.lookup f
  | [], _ => rfl
  | (a, b) :: rest, h => by
    have ih := lookup_filter f P rest (fun fv hfv hf => h fv (List.mem_cons_of_mem _ hfv) hf)
    by_cases hP : P (a, b) = true
    · rw [List.filter_cons_of_pos hP]
      by_cases ha : a = f
      · subst ha
        simp [List.lookup]
      · have hne : (a == f) = false := by simpa using ha
        have hne2 : (f == a) = false := by simpa [beq_iff_eq] using Ne.symm ha
        simp [List.lookup, hne, hne2, ih]
    · have hfa : a ≠ f := by
        intro ha
        subst ha
        exact absurd (h (a, b) List.mem_cons_self rfl) (by simpa using hP)
      have hne : (f == a) = false := by simpa [beq_iff_eq] using Ne.symm hfa
      rw [List.filter_cons_of_neg (by simpa using hP)]
      simp [List.lookup, hne, ih]

theorem fields_get_redact (rs : FieldRules) (p : Principal) (ty : TypeId) (r : Record) (f : FieldId)
    (hf : shownField rs p ty r f = true) :
    Fields.get (redact rs p ty r).fields f = Fields.get r.fields f := by
  unfold Fields.get redact
  simp only
  rw [lookup_filter f (fun fv => shownField rs p ty r fv.1)]
  intro fv _ hfv
  rw [hfv]
  exact hf

theorem evalExpr_redact (rs : FieldRules) (p : Principal) (ty : TypeId) (r : Record) :
    ∀ (e : Expr), (∀ f ∈ fieldsRead e, shownField rs p ty r f = true) →
      evalExpr (readEnv p (redact rs p ty r)) e = evalExpr (readEnv p r) e
  | .lit _, _ => rfl
  | .field s f, h => by
    have hf : shownField rs p ty r f = true := h f (by simp [fieldsRead])
    cases s <;> simp only [evalExpr, readEnv, ExprEnv.scoped, fields_get_redact rs p ty r f hf]
  | .createdBy s, _ => by cases s <;> simp [evalExpr, readEnv, ExprEnv.scoped, redact]
  | .principal, _ => rfl
  | .cmp op a b, h => by
    have ha := evalExpr_redact rs p ty r a (fun f hf => h f (by simp [fieldsRead, hf]))
    have hb := evalExpr_redact rs p ty r b (fun f hf => h f (by simp [fieldsRead, hf]))
    simp only [evalExpr, ha, hb]
  | .and a b, h => by
    have ha := evalExpr_redact rs p ty r a (fun f hf => h f (by simp [fieldsRead, hf]))
    have hb := evalExpr_redact rs p ty r b (fun f hf => h f (by simp [fieldsRead, hf]))
    simp only [evalExpr, ha, hb]
  | .or a b, h => by
    have ha := evalExpr_redact rs p ty r a (fun f hf => h f (by simp [fieldsRead, hf]))
    have hb := evalExpr_redact rs p ty r b (fun f hf => h f (by simp [fieldsRead, hf]))
    simp only [evalExpr, ha, hb]
  | .not a, h => by
    have ha := evalExpr_redact rs p ty r a (fun f hf => h f (by simp [fieldsRead, hf]))
    simp only [evalExpr, ha]
  | .isNull a, h => by
    have ha := evalExpr_redact rs p ty r a (fun f hf => h f (by simp [fieldsRead, hf]))
    simp only [evalExpr, ha]

/-- A filter whose fields carry no read rule holds on a redacted record exactly when it holds on the stored
    one: the filter of a named query reads the same values in both. -/
theorem condHolds_redact (rs : FieldRules) (p : Principal) (ty : TypeId) (r : Record) (e : Expr)
    (hf : ∀ f ∈ fieldsRead e, rs.rule ty f = none) :
    condHolds (readEnv p (redact rs p ty r)) (some e) = condHolds (readEnv p r) (some e) := by
  have hshown : ∀ f ∈ fieldsRead e, shownField rs p ty r f = true := by
    intro f hfe
    cases p with
    | system => rfl
    | user n => simp [shownField, hf f hfe]
    | anonymous => simp [shownField, hf f hfe]
  simp only [condHolds, evalExpr_redact rs p ty r e hshown]

theorem length_filterMap_eq_filter {α β : Type} (f : α → Option β) (g : α → Bool) :
    ∀ (L : List α), (∀ a ∈ L, (f a).isSome = g a) → (L.filterMap f).length = (L.filter g).length
  | [], _ => rfl
  | a :: L, h => by
    have ih := length_filterMap_eq_filter f g L (fun x hx => h x (List.mem_cons_of_mem _ hx))
    have ha := h a List.mem_cons_self
    simp only [List.filterMap_cons, List.filter_cons]
    cases hfa : f a <;> cases hga : g a <;> simp_all

theorem selectedBy_plain (src : Src) (spec : ReadSpec) (p : Principal) (q : QueryDef) (req : AggReq)
    (hf : req.filter = []) (e : RecordId × Record) :
    selectedBy src spec p q req [] e = (src.canRead p q.ty e.2 && condHolds (readEnv p e.2) q.filter) := by
  unfold selectedBy
  cases p <;> simp [hf]

/-- The records an aggregate with no named field and no request filter selects, in table order. -/
def plainSelected (src : Src) (spec : ReadSpec) (p : Principal) (q : QueryDef) (req : AggReq) :
    List (RecordId × Record) :=
  (src.recs q.ty).filter (selectedBy src spec p q req [])

theorem aggregateOn_count_form (src : Src) (spec : ReadSpec) (p : Principal) (req : AggReq)
    (q : QueryDef) (a : AggDef) (o : ObjType)
    (hk : a.kind = .count) (hfe : req.filter = []) (hfield : a.field = none) (hg : a.groupBy = none) :
    aggregateOn src spec p req q a o =
      (if maxScan < (src.recs q.ty).length then .error (.limitExceeded .scanBound)
       else if min a.scanBound maxScan < (plainSelected src spec p q req).length
         then .error (.limitExceeded .scanBound)
       else .ok { groups := [{ key := none, value := some (.int ((plainSelected src spec p q req).length : Int)),
                               rows := [] }],
                  scanned := (plainSelected src spec p q req).length }) := by
  unfold aggregateOn plainSelected
  simp only [hfe, hfield, hg, List.map_nil, List.nil_append, Option.toList,
    List.any_nil, Bool.false_eq_true, ite_false, hk, finish, Except.map, List.map_map,
    List.length_map, List.length_singleton, maxAggGroups, Nat.reduceLT, ↓reduceIte]


theorem queryAll_length_eq (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef)
    (hfilt : ∀ e, q.filter = some e → ∀ f ∈ fieldsRead e, spec.rules.rule q.ty f = none) :
    (queryAll t spec p q).length =
      ((t.recordsOf q.ty).filter (fun e => t.canRead p q.ty e.2 && condHolds (readEnv p e.2) q.filter)).length := by
  unfold queryAll
  rw [(List.mergeSort_perm _ _).length_eq]
  unfold scanRows
  apply length_filterMap_eq_filter
  intro e _
  by_cases hc : t.canRead p q.ty e.2 = true
  · have hcond : condHolds (readEnv p (redact spec.rules p q.ty e.2)) q.filter =
        condHolds (readEnv p e.2) q.filter := by
      cases hq : q.filter with
      | none => rfl
      | some fe => exact condHolds_redact spec.rules p q.ty e.2 fe (hfilt fe hq)
    cases hcc : condHolds (readEnv p e.2) q.filter <;> simp [hc, hcond, hcc]
  · simp [hc]

theorem aggregate_count_queryAll (t : Tenant) (spec : ReadSpec) (p : Principal) (req : AggReq)
    (q : QueryDef) (a : AggDef) (o : ObjType) (r : AggResult)
    (hq : spec.queries.lookup req.query = some q) (ha : q.aggregates.lookup req.aggregate = some a)
    (hp : ¬ req.params.any (fun kv => !(q.params.contains kv.1)) = true) (ht : t.typ? q.ty = some o)
    (hk : a.kind = .count) (hfe : req.filter = []) (hfield : a.field = none) (hg : a.groupBy = none)
    (hfilt : ∀ e, q.filter = some e → ∀ f ∈ fieldsRead e, spec.rules.rule q.ty f = none)
    (hr : aggregate t spec p req = .ok r) :
    r.groups = [{ key := none, value := some (.int ((queryAll t spec p q).length : Int)), rows := [] }] ∧
      r.scanned = (queryAll t spec p q).length := by
  unfold aggregate at hr
  simp only [hq, ha, hp, ite_false, ht] at hr
  rw [aggregateOn_count_form (Src.ofTenant t) spec p req q a o hk hfe hfield hg] at hr
  by_cases hW : maxScan < ((Src.ofTenant t).recs q.ty).length
  · simp [hW] at hr
  · by_cases hB : min a.scanBound maxScan < (plainSelected (Src.ofTenant t) spec p q req).length
    · simp [hW, hB] at hr
    · simp only [hW, hB, ite_false] at hr
      have hr' := Except.ok.inj hr
      subst hr'
      have hsel : plainSelected (Src.ofTenant t) spec p q req =
          (t.recordsOf q.ty).filter (fun e => t.canRead p q.ty e.2 && condHolds (readEnv p e.2) q.filter) := by
        unfold plainSelected
        apply List.filter_congr
        intro e _
        rw [selectedBy_plain _ _ _ _ _ hfe]
        rfl
      rw [queryAll_length_eq t spec p q hfilt, ← hsel]
      exact ⟨rfl, rfl⟩

end Strata

-- Axioms of the slice 10 search and aggregate theorems
#print axioms Strata.SoundHit
#print axioms Strata.aggregateOn_count_form
#print axioms Strata.aggregate_count_queryAll
#print axioms Strata.aggregate_scan_window
#print axioms Strata.aggregate_undeclared_filter
#print axioms Strata.aggregate_undeclared_param
#print axioms Strata.aggregate_unknown_aggregate
#print axioms Strata.aggregate_unknown_field
#print axioms Strata.aggregate_unknown_object
#print axioms Strata.aggregate_unknown_query
#print axioms Strata.candidatesOf_mem
#print axioms Strata.candidatesOf_sorted
#print axioms Strata.condHolds_redact
#print axioms Strata.decideCand_id
#print axioms Strata.decideCand_sound
#print axioms Strata.decideCand_ty
#print axioms Strata.decideRun_consumed_le
#print axioms Strata.decideRun_none_eq
#print axioms Strata.decideRun_order
#print axioms Strata.decideRun_prefix
#print axioms Strata.decideRun_some_last_eq
#print axioms Strata.decideRun_some_more_eq
#print axioms Strata.decideRun_sound
#print axioms Strata.evalExpr_redact
#print axioms Strata.fieldsRead
#print axioms Strata.fields_get_redact
#print axioms Strata.hitLt
#print axioms Strata.hits_append_sound
#print axioms Strata.length_filterMap_eq_filter
#print axioms Strata.lookup_filter
#print axioms Strata.matchedIn_match
#print axioms Strata.matchedIn_mem
#print axioms Strata.mergeSort_recs_sorted
#print axioms Strata.plainSelected
#print axioms Strata.probeType_examined_le
#print axioms Strata.probeType_order
#print axioms Strata.probeType_sound
#print axioms Strata.queryAll_length_eq
#print axioms Strata.searchOn_order
#print axioms Strata.searchOn_sound
#print axioms Strata.searchPlan_query
#print axioms Strata.searchPlan_sorted
#print axioms Strata.searchTypes_examined_le
#print axioms Strata.searchTypes_order
#print axioms Strata.searchTypes_sound
#print axioms Strata.search_examined_le
#print axioms Strata.search_no_repeat
#print axioms Strata.search_order
#print axioms Strata.search_page_out_of_range
#print axioms Strata.search_query_no_word
#print axioms Strata.search_query_too_long
#print axioms Strata.search_query_too_many_words
#print axioms Strata.search_sound
#print axioms Strata.search_unknown_object
#print axioms Strata.selectedBy_plain
#print axioms Strata.sorted_recs_strict
#print axioms Strata.take_lt_drop
#print axioms Strata.take_take_le
#print axioms Strata.types_sorted_strict
