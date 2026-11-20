//! The capability model of the `routes` module: what the generated authorizer checks a call's
//! capabilities against, the one function that checks them, and the operation catalogue.
//!
//! A contract's structured auth names the capabilities a call needs (`auth.requires`). The
//! specification's `capabilities`, `plans`, `roles`, `grants`, `tokenScopes`, `delegations` and
//! `featureFlags` become constant tables, and `permitted` (generated) admits a call only when,
//! for every capability it requires:
//!
//! - **plan entitlement:** the capability is core, or the plan of the tenant the call is made in
//!   entitles it (`AuthHoles::entitlement` supplies the tenant's plan and flag settings);
//! - **flag rollout:** no feature flag gating it is off for the tenant (a flag the tenant has
//!   no setting for has its `default`, off unless stated);
//! - **principal permission:** the caller holds it. An anonymous caller (or the bearer of a
//!   signed address) holds what the platform roles held by `anonymous` confer, a service what
//!   those held by `service` confer, a party proved by signature or provisioning token what
//!   those held by `external` confer. A principal established by a session or a delegation holds
//!   what `AuthHoles::holding` says: the platform roles it holds (whose capabilities the
//!   specification lists), its tenant roles and its teams' roles with their permissions (tenant
//!   data), and its live grants, each conferring the grant's capabilities while it lasts, to a
//!   holder of the grant's grantee role, in the grant's own tenant when the grant is per tenant;
//! - **delegation bounds:** a call under a delegation uses only capabilities the delegation
//!   passes on and a scope of its token allows, until the delegation expires.
//!
//! The authorizer calls it after the scheme's credential, origin, request token, roles, step-up
//! and tenant scope are checked, so every capability check is in that one place.
//!
//! **The operation catalogue** (AGT-001..003): `OPERATIONS` lists every contract reachable from
//! outside its process with its reference, name, description, access and input schema (a JSON
//! Schema of its inputs, under their wire names), and `catalogue` marks each one for a principal
//! with whether it may be attempted: it is public, reachable by the caller's scheme (or
//! anonymous), and `permitted` admits it.
//!
//! **Refusals.** A served contract reachable from outside its process that requires no
//! capability is refused with `capability.required` when the specification states capabilities
//! at all, and a required capability the specification does not declare with
//! `capability.unknown` (Exegra's codes).

use std::collections::BTreeMap;

use proc_macro2::{Literal, TokenStream};
use quote::quote;
use serde_json::{json, Map, Value};

use crate::diagnostic::Diagnostic;
use crate::ir::{Contract, Exposure, RoleLevel, Spec, TypeKind};
use crate::resolve::{Builtin, TypeRef};
use crate::wire::{self, WireCase};

/// Code of a served contract reachable from outside its process that requires no capability
/// (Exegra's code).
pub const CAPABILITY_REQUIRED: &str = "capability.required";
/// Code of a required capability the specification does not declare (Exegra's code).
pub const CAPABILITY_UNKNOWN: &str = "capability.unknown";

/// The names the capability model claims in the `routes` module: functions and constants.
pub const HELPERS: &[&str] = &[
    "CAPABILITIES",
    "PLANS",
    "PLATFORM_ROLES",
    "ANONYMOUS_CAPABILITIES",
    "SERVICE_CAPABILITIES",
    "EXTERNAL_CAPABILITIES",
    "GRANTS",
    "TOKEN_SCOPES",
    "DELEGATIONS",
    "FLAGS",
    "OPERATIONS",
    "listed",
    "held",
    "permitted",
    "catalogue",
    "operation",
];

/// The types the capability model claims in the `routes` module.
pub const TYPES: &[&str] = &[
    "CapabilitySpec",
    "GrantSpec",
    "FlagSpec",
    "Entitlement",
    "TenantRole",
    "Team",
    "HeldGrant",
    "ActingFor",
    "Holding",
    "Asker",
    "CapabilityRefusal",
    "Operation",
    "CatalogueEntry",
];

/// The refusals of the capability model for the served contracts `served`.
pub fn check(spec: &Spec, served: &[&Contract]) -> Vec<Diagnostic> {
    let access = &spec.constructs.access;
    let mut out = Vec::new();
    for c in served {
        let (Some(exposure), Some(auth)) = (c.exposure, &c.auth) else {
            continue;
        };
        if exposure == Exposure::Local {
            continue;
        }
        if auth.requires.is_empty() {
            if !access.capabilities.is_empty() {
                out.push(Diagnostic::new(
                    CAPABILITY_REQUIRED,
                    &c.id,
                    format!(
                        "Contract \"{}\" is reachable from outside its process and requires no capability; name the capabilities a call needs in auth.requires, so the authorizer checks the plan, the flags and the principal against them.",
                        c.id
                    ),
                ));
            }
            continue;
        }
        for capability in &auth.requires {
            if access.capability(capability).is_none() {
                out.push(Diagnostic::new(
                    CAPABILITY_UNKNOWN,
                    &c.id,
                    format!(
                        "Contract \"{}\" requires capability \"{capability}\", which the specification does not declare.",
                        c.id
                    ),
                ));
            }
        }
    }
    out
}

/// A list of string literals.
fn strs<'a>(items: impl IntoIterator<Item = &'a String>) -> TokenStream {
    let items: Vec<&String> = items.into_iter().collect();
    quote! { &[#(#items),*] }
}

/// The capabilities the platform roles held by `holder` confer, each once, in order.
fn held_by(spec: &Spec, holder: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for role in &spec.constructs.access.roles {
        if role.level != RoleLevel::Platform || !role.holders.iter().any(|h| h == holder) {
            continue;
        }
        for c in &role.capabilities {
            if !out.contains(c) {
                out.push(c.clone());
            }
        }
    }
    out
}

/// The tables, the holding and refusal types and `permitted`.
pub fn items(spec: &Spec) -> TokenStream {
    let access = &spec.constructs.access;
    let capabilities: Vec<TokenStream> = access
        .capabilities
        .iter()
        .map(|c| {
            let id = &c.id;
            let core = c.core;
            let flags = strs(
                access
                    .flags
                    .iter()
                    .filter(|f| f.gates.contains(&c.id))
                    .map(|f| &f.id),
            );
            quote! { CapabilitySpec { id: #id, core: #core, flags: #flags } }
        })
        .collect();
    let pair = |id: &String, list: &Vec<String>| {
        let list = strs(list);
        quote! { (#id, #list) }
    };
    let plans: Vec<TokenStream> = access
        .plans
        .iter()
        .map(|p| pair(&p.id, &p.capabilities))
        .collect();
    let platform_roles: Vec<TokenStream> = access
        .roles
        .iter()
        .filter(|r| r.level == RoleLevel::Platform)
        .map(|r| pair(&r.id, &r.capabilities))
        .collect();
    let anonymous = strs(&held_by(spec, "anonymous"));
    let service = strs(&held_by(spec, "service"));
    let external = strs(&held_by(spec, "external"));
    let grants: Vec<TokenStream> = access
        .grants
        .iter()
        .map(|g| {
            let id = &g.id;
            let grantee = g.grantee.clone().unwrap_or_default();
            let caps = strs(&g.capabilities);
            let per_tenant = g.per_tenant;
            quote! { GrantSpec { id: #id, grantee: #grantee, capabilities: #caps, per_tenant: #per_tenant } }
        })
        .collect();
    let scopes: Vec<TokenStream> = access
        .token_scopes
        .iter()
        .map(|s| pair(&s.name, &s.capabilities))
        .collect();
    let delegations: Vec<TokenStream> = access
        .delegations
        .iter()
        .map(|d| pair(&d.id, &d.capabilities))
        .collect();
    let flags: Vec<TokenStream> = access
        .flags
        .iter()
        .map(|f| {
            let id = &f.id;
            let on = f.default_on;
            quote! { FlagSpec { id: #id, default_on: #on } }
        })
        .collect();
    quote! {
        /// One capability of the specification: whether every plan has it (`core`), and the
        /// feature flags that gate it.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct CapabilitySpec {
            /// The capability id.
            pub id: &'static str,
            /// Whether every plan entitles it.
            pub core: bool,
            /// The flags that gate it per tenant.
            pub flags: &'static [&'static str],
        }

        /// One grant of the specification: the role that receives it, what it confers, and
        /// whether it confers it in one tenant only.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct GrantSpec {
            /// The grant id.
            pub id: &'static str,
            /// The role whose holders receive it.
            pub grantee: &'static str,
            /// The capabilities it confers while it lasts.
            pub capabilities: &'static [&'static str],
            /// Whether it confers them in the one tenant it names.
            pub per_tenant: bool,
        }

        /// One feature flag of the specification and its value for a tenant with no setting.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct FlagSpec {
            /// The flag id.
            pub id: &'static str,
            /// Whether it is on for a tenant that has no setting of its own.
            pub default_on: bool,
        }

        /// The capabilities of the specification.
        pub const CAPABILITIES: &[CapabilitySpec] = &[#(#capabilities),*];
        /// Each plan and the capabilities it entitles beyond the core ones.
        pub const PLANS: &[(&str, &[&str])] = &[#(#plans),*];
        /// Each platform role and the capabilities it confers. Tenant roles are tenant data:
        /// `AuthHoles::holding` supplies them with their permissions.
        pub const PLATFORM_ROLES: &[(&str, &[&str])] = &[#(#platform_roles),*];
        /// What an anonymous caller, or the bearer of a signed address, holds: the capabilities
        /// of the platform roles anonymous callers hold.
        pub const ANONYMOUS_CAPABILITIES: &[&str] = #anonymous;
        /// What a platform service holds: the capabilities of the platform roles services hold.
        pub const SERVICE_CAPABILITIES: &[&str] = #service;
        /// What a party proved by signature or provisioning token holds: the capabilities of the
        /// platform roles external parties hold.
        pub const EXTERNAL_CAPABILITIES: &[&str] = #external;
        /// The grants of the specification.
        pub const GRANTS: &[GrantSpec] = &[#(#grants),*];
        /// Each token scope, by the name a token carries it under, and the capabilities it allows.
        pub const TOKEN_SCOPES: &[(&str, &[&str])] = &[#(#scopes),*];
        /// Each delegation and the capabilities it passes on.
        pub const DELEGATIONS: &[(&str, &[&str])] = &[#(#delegations),*];
        /// The feature flags of the specification.
        pub const FLAGS: &[FlagSpec] = &[#(#flags),*];

        /// What a tenant is entitled to: its plan and its own flag settings, as the deployment
        /// holds them.
        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct Entitlement {
            /// The id of the tenant's plan.
            pub plan: String,
            /// The tenant's own setting of each flag it has one for: on or off.
            pub flags: Vec<(String, bool)>,
        }

        /// A tenant role as the tenant holds it: its id and the capabilities among its
        /// permissions (its object-type permissions are the kernel's to check).
        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct TenantRole {
            /// The role id.
            pub id: String,
            /// The capabilities it grants.
            pub capabilities: Vec<String>,
        }

        /// A team of a tenant a principal belongs to, with the roles the team holds.
        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct Team {
            /// The team id.
            pub id: String,
            /// The roles the team holds, which its members hold through it.
            pub roles: Vec<TenantRole>,
        }

        /// A grant a principal has received, as the deployment holds it.
        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct HeldGrant {
            /// The id of the grant of the specification it is one of.
            pub grant: String,
            /// The tenant it names, for a grant per tenant.
            pub tenant: Option<String>,
            /// When it ends, in milliseconds since the Unix epoch.
            pub expires_ms: i64,
        }

        /// The delegation a principal acts under and the scopes of its token.
        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct ActingFor {
            /// The id of the delegation of the specification it is one of.
            pub delegation: String,
            /// The scopes its token carries.
            pub scopes: Vec<String>,
            /// When it ends, in milliseconds since the Unix epoch.
            pub expires_ms: i64,
        }

        /// What a principal holds, as the deployment knows it: under a delegation, what the
        /// person it acts for holds, and the delegation.
        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct Holding {
            /// The platform roles it holds.
            pub platform_roles: Vec<String>,
            /// The tenant roles assigned to it directly.
            pub tenant_roles: Vec<TenantRole>,
            /// The teams it belongs to.
            pub teams: Vec<Team>,
            /// The grants it has received and not had withdrawn.
            pub grants: Vec<HeldGrant>,
            /// The delegation it acts under, when it acts for someone.
            pub acting_for: Option<ActingFor>,
        }

        /// Who asks, as far as capabilities go.
        #[derive(Clone, Copy, Debug)]
        pub enum Asker<'a> {
            /// Nobody: an anonymous call, or the bearer of a signed address.
            Anonymous,
            /// A platform service.
            Service,
            /// A party proved by signature or provisioning token.
            External,
            /// A principal established by a session or a delegation, with what it holds.
            Principal(&'a Holding),
        }

        /// Why a call is refused a capability it requires.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum CapabilityRefusal {
            /// The tenant's plan does not entitle it (or the specification does not declare it).
            NotEntitled(&'static str),
            /// A flag that gates it is off for the tenant.
            FlagOff {
                /// The capability.
                capability: &'static str,
                /// The flag.
                flag: &'static str,
            },
            /// The caller does not hold it.
            NotHeld(&'static str),
            /// The delegation the caller acts under does not pass it on.
            BeyondDelegation(&'static str),
        }

        impl CapabilityRefusal {
            /// A message safe to show the caller.
            pub fn message(&self) -> &'static str {
                match self {
                    CapabilityRefusal::NotEntitled(_) => "the tenant's plan does not include this",
                    CapabilityRefusal::FlagOff { .. } => "this is not switched on for the tenant",
                    CapabilityRefusal::NotHeld(_) => "the caller does not hold the permission this needs",
                    CapabilityRefusal::BeyondDelegation(_) => "the delegation does not extend to this",
                }
            }
        }

        /// The list a table gives `key`, or none.
        fn listed(table: &'static [(&'static str, &'static [&'static str])], key: &str) -> &'static [&'static str] {
            table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v).unwrap_or(&[])
        }

        /// The capabilities `asker` holds in `tenant` at `now_ms`: its roles', its teams' roles'
        /// and its live grants' (each only for a holder of its grantee role, and for a grant per
        /// tenant only in that tenant).
        pub fn held(asker: Asker<'_>, tenant: Option<&str>, now_ms: i64) -> std::collections::BTreeSet<String> {
            let mut out = std::collections::BTreeSet::new();
            fn fixed(list: &[&str], out: &mut std::collections::BTreeSet<String>) {
                out.extend(list.iter().map(|c| c.to_string()));
            }
            match asker {
                Asker::Anonymous => fixed(ANONYMOUS_CAPABILITIES, &mut out),
                Asker::Service => fixed(SERVICE_CAPABILITIES, &mut out),
                Asker::External => fixed(EXTERNAL_CAPABILITIES, &mut out),
                Asker::Principal(holding) => {
                    for role in &holding.platform_roles {
                        fixed(listed(PLATFORM_ROLES, role), &mut out);
                    }
                    for role in &holding.tenant_roles {
                        out.extend(role.capabilities.iter().cloned());
                    }
                    for team in &holding.teams {
                        for role in &team.roles {
                            out.extend(role.capabilities.iter().cloned());
                        }
                    }
                    for held in &holding.grants {
                        let Some(grant) = GRANTS.iter().find(|g| g.id == held.grant) else {
                            continue;
                        };
                        let live = now_ms < held.expires_ms;
                        let grantee = holding.platform_roles.iter().any(|r| r == grant.grantee)
                            || holding.tenant_roles.iter().any(|r| r.id == grant.grantee);
                        let here = !grant.per_tenant
                            || (held.tenant.is_some() && held.tenant.as_deref() == tenant);
                        if live && grantee && here {
                            fixed(grant.capabilities, &mut out);
                        }
                    }
                }
            }
            out
        }

        /// The one check of capabilities: whether a call requiring `requires`, made by `asker`
        /// in `tenant` (whose plan and flags `entitlement` gives) at `now_ms`, is admitted. Per
        /// capability, in order: plan entitlement, flag rollout, the principal's permission,
        /// delegation bounds.
        pub fn permitted(
            requires: &[&'static str],
            asker: Asker<'_>,
            tenant: Option<&str>,
            entitlement: Option<&Entitlement>,
            now_ms: i64,
        ) -> Result<(), CapabilityRefusal> {
            if requires.is_empty() {
                return Ok(());
            }
            let holds = held(asker, tenant, now_ms);
            let bound: Option<std::collections::BTreeSet<&'static str>> = match asker {
                Asker::Principal(holding) => holding.acting_for.as_ref().map(|acting| {
                    if now_ms >= acting.expires_ms {
                        return std::collections::BTreeSet::new();
                    }
                    let scoped: std::collections::BTreeSet<&'static str> = acting
                        .scopes
                        .iter()
                        .flat_map(|scope| listed(TOKEN_SCOPES, scope).iter().copied())
                        .collect();
                    listed(DELEGATIONS, &acting.delegation)
                        .iter()
                        .copied()
                        .filter(|c| scoped.contains(c))
                        .collect()
                }),
                _ => None,
            };
            for &capability in requires {
                let Some(spec) = CAPABILITIES.iter().find(|c| c.id == capability) else {
                    return Err(CapabilityRefusal::NotEntitled(capability));
                };
                if !spec.core && !entitlement.is_some_and(|e| listed(PLANS, &e.plan).contains(&capability)) {
                    return Err(CapabilityRefusal::NotEntitled(capability));
                }
                for &flag in spec.flags {
                    let default_on = FLAGS.iter().find(|f| f.id == flag).is_some_and(|f| f.default_on);
                    let on = entitlement
                        .and_then(|e| e.flags.iter().find(|(id, _)| id == flag).map(|(_, on)| *on))
                        .unwrap_or(default_on);
                    if !on {
                        return Err(CapabilityRefusal::FlagOff { capability, flag });
                    }
                }
                if !holds.contains(capability) {
                    return Err(CapabilityRefusal::NotHeld(capability));
                }
                if let Some(bound) = &bound {
                    if !bound.contains(capability) {
                        return Err(CapabilityRefusal::BeyondDelegation(capability));
                    }
                }
            }
            Ok(())
        }
    }
}

/// The operation catalogue: `Operation`, `OPERATIONS`, `CatalogueEntry`, `catalogue` and
/// `operation`.
pub fn catalogue_items(spec: &Spec, served: &[&Contract]) -> TokenStream {
    let cases = wire::type_cases(spec);
    let operations: Vec<TokenStream> = served
        .iter()
        .filter(|c| c.exposure.is_some_and(|e| e != Exposure::Local) && c.auth.is_some())
        .map(|c| {
            let id = c.reference.clone().unwrap_or_else(|| c.id.clone());
            let contract = &c.id;
            let name = &c.name;
            let description = c.description.clone().unwrap_or_default();
            let access = super::routes::access_tokens(c);
            let schema = Literal::string(&input_schema(spec, &cases, c).to_string());
            quote! {
                Operation {
                    id: #id,
                    contract: #contract,
                    name: #name,
                    description: #description,
                    access: #access,
                    input_schema: #schema,
                }
            }
        })
        .collect();
    quote! {
        /// One operation of the catalogue (AGT-001): a contract reachable from outside its
        /// process, with its description and the JSON Schema of its input.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct Operation {
            /// The contract's reference, such as `CT-012`.
            pub id: &'static str,
            /// The contract's id in the specification.
            pub contract: &'static str,
            /// Its name.
            pub name: &'static str,
            /// Its description.
            pub description: &'static str,
            /// Who may call it and from where, with the capabilities it requires.
            pub access: strata_runtime::Access,
            /// The JSON Schema of its request body.
            pub input_schema: &'static str,
        }

        /// Every operation of the catalogue, in the specification's order.
        pub const OPERATIONS: &[Operation] = &[#(#operations),*];

        /// One operation as the catalogue shows it to a principal (AGT-002).
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct CatalogueEntry {
            /// The operation.
            pub operation: &'static Operation,
            /// Whether the principal may attempt it.
            pub may_attempt: bool,
        }

        /// The operation with reference or id `id`.
        pub fn operation(id: &str) -> Option<&'static Operation> {
            OPERATIONS.iter().find(|o| o.id == id || o.contract == id)
        }

        /// The catalogue as a principal holding `holding` sees it when it asks with `scheme` in
        /// `tenant` (whose plan and flags `entitlement` gives) at `now_ms`: every operation,
        /// marked with whether it may be attempted. It may when it is public, its scheme is the
        /// caller's (or anonymous), a delegated caller acts under a known delegation, and
        /// [`permitted`] admits its capabilities: the check the authorizer makes on the call.
        pub fn catalogue(
            scheme: strata_runtime::Scheme,
            holding: &Holding,
            tenant: Option<&str>,
            entitlement: Option<&Entitlement>,
            now_ms: i64,
        ) -> Vec<CatalogueEntry> {
            use strata_runtime::{Exposure, Scheme};
            OPERATIONS
                .iter()
                .map(|operation| {
                    let access = &operation.access;
                    let anonymous = access.scheme == Scheme::Anonymous;
                    let reachable = access.exposure == Exposure::Public && (access.scheme == scheme || anonymous);
                    let delegated = anonymous || scheme != Scheme::Delegation || holding.acting_for.is_some();
                    let asker = if anonymous { Asker::Anonymous } else { Asker::Principal(holding) };
                    let may_attempt = reachable
                        && delegated
                        && permitted(access.requires, asker, tenant, entitlement, now_ms).is_ok();
                    CatalogueEntry { operation, may_attempt }
                })
                .collect()
        }
    }
}

/// The JSON Schema of the request body of contract `c`: an object of its inputs under their wire
/// names, the ones that are not optional required.
fn input_schema(spec: &Spec, cases: &BTreeMap<String, WireCase>, c: &Contract) -> Value {
    let case = wire::contract_case(spec, c);
    let mut properties = Map::new();
    let mut required = Vec::new();
    for f in &c.inputs {
        let name = case.name(&f.name);
        let mut schema = match &f.type_name {
            Some(t) => type_schema(spec, cases, &TypeRef::parse(t), &mut Vec::new()),
            None => json!({}),
        };
        if let (Some(description), Value::Object(o)) = (&f.description, &mut schema) {
            o.insert("description".into(), Value::String(description.clone()));
        }
        if !f.optional {
            required.push(Value::String(name.clone()));
        }
        properties.insert(name, schema);
    }
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

/// The JSON Schema of a type: a built-in by its JSON form, an id or scalar by its
/// representation, an enum by its values, a struct as an object of its fields under the wire
/// names of its case. A struct already being described (a recursive type) and a type specc
/// cannot resolve are any value.
fn type_schema(
    spec: &Spec,
    cases: &BTreeMap<String, WireCase>,
    t: &TypeRef,
    seen: &mut Vec<String>,
) -> Value {
    match t {
        TypeRef::Builtin(b) => builtin_schema(*b),
        TypeRef::List(inner) => {
            json!({ "type": "array", "items": type_schema(spec, cases, inner, seen) })
        }
        TypeRef::Option(inner) => type_schema(spec, cases, inner, seen),
        TypeRef::Shared(inner) => type_schema(spec, cases, inner, seen),
        TypeRef::Set(inner) => {
            json!({ "type": "array", "uniqueItems": true, "items": type_schema(spec, cases, inner, seen) })
        }
        TypeRef::Map(_, value) => {
            json!({ "type": "object", "additionalProperties": type_schema(spec, cases, value, seen) })
        }
        TypeRef::Declared(name) => {
            let Some(decl) = spec.type_decl(name) else {
                return json!({});
            };
            match &decl.kind {
                TypeKind::Id { repr } | TypeKind::Scalar { repr } => repr
                    .as_deref()
                    .and_then(Builtin::parse)
                    .map_or_else(|| json!({}), builtin_schema),
                TypeKind::Enum { values } => json!({ "enum": values }),
                TypeKind::Struct { fields } => {
                    if seen.contains(name) {
                        return json!({});
                    }
                    seen.push(name.clone());
                    let case = cases.get(name).copied().unwrap_or(WireCase::Kebab);
                    let mut properties = Map::new();
                    let mut required = Vec::new();
                    for f in fields {
                        let wire_name = case.name(&f.name);
                        if !f.optional {
                            required.push(Value::String(wire_name.clone()));
                        }
                        properties.insert(
                            wire_name,
                            type_schema(spec, cases, &TypeRef::parse(&f.type_name), seen),
                        );
                    }
                    seen.pop();
                    json!({ "type": "object", "properties": properties, "required": required })
                }
                // A union is written by its encoding (tagged or untagged); the catalogue states no
                // narrower schema than "any JSON" for it rather than a wrong one.
                TypeKind::Union { .. } => json!({}),
            }
        }
    }
}

/// The JSON Schema of a built-in type, by the JSON its generated Rust type is written as.
fn builtin_schema(b: Builtin) -> Value {
    match b {
        Builtin::Bool => json!({ "type": "boolean" }),
        Builtin::U8
        | Builtin::U16
        | Builtin::U32
        | Builtin::U64
        | Builtin::I32
        | Builtin::I64
        | Builtin::Timestamp
        | Builtin::Duration => json!({ "type": "integer" }),
        Builtin::F64 => json!({ "type": "number" }),
        Builtin::String => json!({ "type": "string" }),
        Builtin::Uuid => json!({ "type": "string", "format": "uuid" }),
        Builtin::Bytes => {
            json!({ "type": "array", "items": { "type": "integer", "minimum": 0, "maximum": 255 } })
        }
        Builtin::Json => json!({}),
    }
}
