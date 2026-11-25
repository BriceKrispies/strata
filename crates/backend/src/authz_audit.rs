//! The audit entries of authorization events (D-8421, OBS-3126, OBS-3127): token issue and
//! refusal, a delegation granted or revoked, a session started or ended, and a role change made
//! by an identity provider.
//!
//! Each is a line of the tenant's audit trail, in the chain every other line is in, and none is
//! written to telemetry alone. A line names:
//!
//! - the actor: `actor` is the person who acted, or `actor_kind` says what acted (`system`,
//!   `anonymous`, `provider`, `client`);
//! - the credential used: `credential` (the sign-in method of a session, `client` for a token
//!   made by a client's credentials, `delegation` for one made from a grant) with the `client`
//!   and the `grant` it names;
//! - the person acted for, as `acted_for`, when the act is on a person's behalf;
//! - the provider, as `provider`, and the provisioning token it presented, as
//!   `provisioning_token`, for a change an identity provider makes (never an owner found in the
//!   member table, OBS-3127).

use crate::app::App;
use serde_json::{json, Map, Value as Json};
use strata_kernel::{Principal, TenantId};
use strata_storage_api::SessionRecord;

/// A change that an identity provider or one of its realm rules makes, as the audit names it.
#[derive(Clone, Debug)]
pub struct Attribution {
    /// The provider: `scim` for a SCIM client, `realm-rule` for a mapping the realm declared.
    pub provider: String,
    /// The id of the provisioning token the provider presented.
    pub provisioning_token: Option<String>,
    /// The rule, for a change a realm rule makes (the group it maps).
    pub rule: Option<String>,
    /// The person who set the rule off, when one did.
    pub actor: Option<String>,
}

impl Attribution {
    /// A SCIM client that presented the provisioning token `token`.
    pub fn scim(token: &str) -> Self {
        Self {
            provider: "scim".into(),
            provisioning_token: Some(token.to_string()),
            rule: None,
            actor: None,
        }
    }

    /// The members an entry carries for this attribution: the provider is the actor.
    pub fn fields(&self) -> Map<String, Json> {
        let mut fields = Map::new();
        match &self.actor {
            Some(person) => {
                fields.insert("actor".into(), json!(person));
            }
            None => {
                fields.insert("actor_kind".into(), json!("provider"));
            }
        }
        fields.insert("provider".into(), json!(self.provider));
        if let Some(token) = &self.provisioning_token {
            fields.insert("provisioning_token".into(), json!(token));
        }
        if let Some(rule) = &self.rule {
            fields.insert("rule".into(), json!(rule));
        }
        fields
    }
}

/// The members that name the actor `principal` of a context.
pub fn actor_fields(principal: &Principal) -> Map<String, Json> {
    let mut fields = Map::new();
    match principal {
        Principal::User(id) => fields.insert("actor".into(), json!(id.as_str())),
        Principal::System => fields.insert("actor_kind".into(), json!("system")),
        Principal::Anonymous => fields.insert("actor_kind".into(), json!("anonymous")),
    };
    fields
}

fn write(
    app: &App,
    event: &str,
    correlation: &str,
    tenant: &TenantId,
    mut fields: Map<String, Json>,
) {
    fields.insert("tenant".into(), json!(tenant.as_str()));
    app.audit(event, correlation, Json::Object(fields));
}

/// `session.started`: a session was opened by `method`.
pub fn session_started(app: &App, session: &SessionRecord, method: &str, correlation: &str) {
    let mut fields = Map::new();
    fields.insert("actor".into(), json!(session.principal.as_str()));
    fields.insert("principal".into(), json!(session.principal.as_str()));
    fields.insert("session".into(), json!(session.id.as_str()));
    fields.insert("credential".into(), json!(method));
    write(app, "session.started", correlation, &session.tenant, fields);
}

/// `session.ended`: a session ended, and why (`sign_out`, `replaced`, `revoked`, `revoked_others`,
/// `roles_changed`, `recovery`, `suspended`), by `by` (the members that name who ended it).
pub fn session_ended(
    app: &App,
    session: &SessionRecord,
    correlation: &str,
    reason: &str,
    by: Map<String, Json>,
) {
    let mut fields = by;
    fields.insert("principal".into(), json!(session.principal.as_str()));
    fields.insert("session".into(), json!(session.id.as_str()));
    fields.insert("reason".into(), json!(reason));
    write(app, "session.ended", correlation, &session.tenant, fields);
}

/// `token.issued`: a client was given a token, by its own credentials or on a delegation.
#[allow(clippy::too_many_arguments)]
pub fn token_issued(
    app: &App,
    tenant: &TenantId,
    correlation: &str,
    client: &str,
    grant: Option<&str>,
    acted_for: Option<&str>,
    scope: &str,
    expires_at: i64,
) {
    let mut fields = Map::new();
    fields.insert("actor_kind".into(), json!("client"));
    fields.insert("client".into(), json!(client));
    fields.insert(
        "credential".into(),
        json!(if grant.is_some() {
            "delegation"
        } else {
            "client"
        }),
    );
    fields.insert("grant".into(), json!(grant));
    fields.insert("acted_for".into(), json!(acted_for));
    fields.insert("scope".into(), json!(scope));
    fields.insert("expires_at".into(), json!(expires_at));
    write(app, "token.issued", correlation, tenant, fields);
}

/// `token.refused`: a request for a token was refused, and why. It names the client the request
/// named and no secret.
pub fn token_refused(
    app: &App,
    tenant: &TenantId,
    correlation: &str,
    client: Option<&str>,
    grant: Option<&str>,
    reason: &str,
) {
    let mut fields = Map::new();
    fields.insert("actor_kind".into(), json!("client"));
    fields.insert("client".into(), json!(client));
    fields.insert("grant".into(), json!(grant));
    fields.insert("reason".into(), json!(reason));
    write(app, "token.refused", correlation, tenant, fields);
}

/// `grant.created`: `person` delegated `scopes` to `agent`.
pub fn grant_created(
    app: &App,
    tenant: &TenantId,
    correlation: &str,
    person: &str,
    agent: &str,
    grant: &str,
    scopes: &[String],
    expires_at: i64,
) {
    let mut fields = Map::new();
    fields.insert("actor".into(), json!(person));
    fields.insert("acted_for".into(), json!(person));
    fields.insert("client".into(), json!(agent));
    fields.insert("grant".into(), json!(grant));
    fields.insert("scopes".into(), json!(scopes));
    fields.insert("expires_at".into(), json!(expires_at));
    write(app, "grant.created", correlation, tenant, fields);
}

/// `grant.revoked`: `actor` revoked the delegation `grant` that `person` made to `agent`.
pub fn grant_revoked(
    app: &App,
    tenant: &TenantId,
    correlation: &str,
    actor: &str,
    person: &str,
    agent: &str,
    grant: &str,
) {
    let mut fields = Map::new();
    fields.insert("actor".into(), json!(actor));
    fields.insert("acted_for".into(), json!(person));
    fields.insert("client".into(), json!(agent));
    fields.insert("grant".into(), json!(grant));
    write(app, "grant.revoked", correlation, tenant, fields);
}
