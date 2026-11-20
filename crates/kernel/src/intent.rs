//! What a caller asks for, the context it asks in, and what the kernel answers with.
//!
//! The types are generated ([`crate::types`]); this module holds what the kernel does with them.

pub use crate::types::{
    Check, Context, DeltaOp, DomainError, DomainEvent, Effect, ErrorKind, EventBody, ExistingData,
    Intent, Output, ReadRequest, ReadResult, SchemaChange, StateDelta, Transition,
};

/// What the kernel asks of an [`Intent`].
pub trait IntentExt {
    /// The intent's name, as journals and metrics write it.
    fn name(&self) -> &'static str;
}

impl IntentExt for Intent {
    fn name(&self) -> &'static str {
        match self {
            Intent::CreateTenant { .. } => "create_tenant",
            Intent::ChangeSchema { .. } => "change_schema",
            Intent::AssignRole { .. } => "assign_role",
            Intent::RevokeRole { .. } => "revoke_role",
            Intent::DefineTeam { .. } => "define_team",
            Intent::AssignTeamRole { .. } => "assign_team_role",
            Intent::RevokeTeamRole { .. } => "revoke_team_role",
            Intent::AddTeamMember { .. } => "add_team_member",
            Intent::RemoveTeamMember { .. } => "remove_team_member",
            Intent::CreateInvitation { .. } => "create_invitation",
            Intent::RedeemInvitation { .. } => "redeem_invitation",
            Intent::RevokeInvitation { .. } => "revoke_invitation",
            Intent::CreateRecord { .. } => "create_record",
            Intent::UpdateRecord { .. } => "update_record",
            Intent::DeleteRecord { .. } => "delete_record",
        }
    }
}

/// One evaluated rule, passed or failed. The list of these explains an acceptance or a
/// rejection.
pub trait CheckExt {
    /// A rule that held.
    fn pass(rule: impl Into<String>, detail: impl Into<String>) -> Self;
    /// A rule that did not hold.
    fn fail(rule: impl Into<String>, detail: impl Into<String>) -> Self;
}

impl CheckExt for Check {
    fn pass(rule: impl Into<String>, detail: impl Into<String>) -> Self {
        Check {
            rule: rule.into(),
            passed: true,
            detail: detail.into(),
        }
    }

    fn fail(rule: impl Into<String>, detail: impl Into<String>) -> Self {
        Check {
            rule: rule.into(),
            passed: false,
            detail: detail.into(),
        }
    }
}

/// A rejection, with the checks that led to it.
pub trait DomainErrorExt {
    /// A rejection of `kind` without evidence.
    fn new(kind: ErrorKind, message: impl Into<String>) -> Self;
    /// The rejection with `evidence` as its checks.
    fn with(self, evidence: Vec<Check>) -> Self;
    /// One line for people: `Kind: message`.
    fn describe(&self) -> String;
}

impl DomainErrorExt for DomainError {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        DomainError {
            kind,
            message: message.into(),
            evidence: Vec::new(),
            missing: None,
        }
    }

    fn with(mut self, evidence: Vec<Check>) -> Self {
        self.evidence = evidence;
        self
    }

    fn describe(&self) -> String {
        format!("{:?}: {}", self.kind, self.message)
    }
}
