//! Which host names serve which tenant, and the origins their pages are served from.
//!
//! The specification names a domain binding (`domain-binding`, `bind-domain`) that the control
//! plane is to own; none of it is implemented, and the backend held no host data at all, so the
//! BFF could learn its sites only from an environment variable read at start. This is the minimal
//! store that closes that gap: one document per host, under the tenant it serves, of kind
//! [`KIND`] in the store's small-document table (`Store::doc_put`), holding the origins that host
//! is reached at. No schema change is needed, and a tenant's reset or removal takes its hosts
//! with it.
//!
//! One internal route, `/internal/platform/hosts`, behind the service token like every internal
//! route, does both jobs:
//!
//! - `{}` lists every binding of every tenant: `{ "hosts": [ { "host", "tenant", "origins" } ] }`.
//!   This is what the BFF fetches (see `strata-bff`'s `hosts` module).
//! - `{ "bind": { "tenant", "origin" } }` binds the origin's host to the tenant (adding the
//!   origin to the host's list) and then lists. A host is bound to one tenant only: binding it to a
//!   second is a conflict.
//!
//! Creating a tenant at run time (`provision::create_tenant`) is to call [`bind_origin`] with the
//! origin it was given, so the BFF learns the new site on its next refresh.

use crate::app::{ApiError, App, Shared};
use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json_};
use strata_kernel::TenantId;
use strata_protocol::api::ErrorCode;

/// The kind of document that holds a host binding, keyed by the host name.
pub const KIND: &str = "host";

/// The body of `/internal/platform/hosts`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostsRequest {
    #[serde(default)]
    pub bind: Option<Bind>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bind {
    pub tenant: String,
    /// `http(s)://host[:port]`.
    pub origin: String,
}

/// What is kept for one host.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Binding {
    origins: Vec<String>,
}

/// The host of an origin in the one spelling that is compared: `(origin, host)` for
/// `http(s)://host[:port]`, lower case, no path.
pub fn parse_origin(origin: &str) -> Option<(String, String)> {
    let origin = origin.trim_end_matches('/').to_ascii_lowercase();
    let authority = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))?;
    let plain = !authority.is_empty()
        && authority
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'));
    if !plain {
        return None;
    }
    let host = match authority.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => authority,
    };
    (!host.is_empty()).then(|| (origin.clone(), host.to_string()))
}

fn invalid(message: &str) -> ApiError {
    ApiError::new(ErrorCode::Invalid, message)
}

/// Every binding of every tenant, in host order.
pub fn list(app: &App) -> Result<Json_, ApiError> {
    let mut hosts = Vec::new();
    for tenant in app.store.tenants()? {
        for (host, value) in app.store.doc_list(&tenant.id, KIND)? {
            let Ok(binding) = serde_json::from_str::<Binding>(&value) else {
                continue;
            };
            hosts.push((host, tenant.id.clone(), binding.origins));
        }
    }
    hosts.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(json!({
        "hosts": hosts
            .into_iter()
            .map(|(host, tenant, origins)| json!({ "host": host, "tenant": tenant.as_str(), "origins": origins }))
            .collect::<Vec<_>>(),
    }))
}

/// Bind the host of `origin` to `tenant`, and keep the origin among the host's origins. Binding a
/// host again with another origin adds it; binding it to a tenant other than the one that holds it
/// is a conflict. Returns the binding as the list shows it.
pub fn bind_origin(app: &App, tenant: &TenantId, origin: &str) -> Result<Json_, ApiError> {
    let (origin, host) = parse_origin(origin)
        .ok_or_else(|| invalid("The origin must be like https://host or http://host:port."))?;
    if app.store.tenant_summary(tenant)?.is_none() {
        return Err(ApiError::new(
            ErrorCode::NotFound,
            format!("The tenant {tenant} does not exist."),
        ));
    }
    for other in app.store.tenants()? {
        if other.id != *tenant && app.store.doc_get(&other.id, KIND, &host)?.is_some() {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                format!("The host {host} is bound to another tenant."),
            ));
        }
    }
    let mut binding = match app.store.doc_get(tenant, KIND, &host)? {
        Some(text) => serde_json::from_str::<Binding>(&text).unwrap_or_default(),
        None => Binding::default(),
    };
    if !binding.origins.contains(&origin) {
        binding.origins.push(origin);
        binding.origins.sort();
    }
    let text = serde_json::to_string(&binding)
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))?;
    app.store.doc_put(tenant, KIND, &host, &text, false)?;
    app.audit(
        "platform.host_bound",
        "",
        json!({ "tenant": tenant.as_str(), "host": host }),
    );
    Ok(json!({ "host": host, "tenant": tenant.as_str(), "origins": binding.origins }))
}

/// `/internal/platform/hosts`: bind when asked to, then list. Mounted in the router; the service
/// token is checked by the gate in front of every internal route.
pub async fn route(
    State(app): State<Shared>,
    Json(request): Json<HostsRequest>,
) -> Result<Json<Json_>, ApiError> {
    app.blocking(move |app| -> Result<Json_, ApiError> {
        let bound = match &request.bind {
            Some(bind) => {
                let tenant = TenantId::new(bind.tenant.as_str())
                    .map_err(|_| invalid("The tenant is not a tenant id."))?;
                Some(bind_origin(app, &tenant, &bind.origin)?)
            }
            None => None,
        };
        let mut reply = list(app)?;
        if let (Some(bound), Some(object)) = (bound, reply.as_object_mut()) {
            object.insert("bound".into(), bound);
        }
        Ok(reply)
    })
    .await?
    .map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_origin_is_read_as_its_scheme_and_host_and_nothing_else() {
        assert_eq!(
            parse_origin("https://Blog.Example/"),
            Some(("https://blog.example".into(), "blog.example".into()))
        );
        assert_eq!(
            parse_origin("http://localhost:4100"),
            Some(("http://localhost:4100".into(), "localhost".into()))
        );
        for bad in [
            "",
            "blog.example",
            "ftp://blog.example",
            "https://",
            "https://blog.example/path",
            "https://user@blog.example",
            "https://blog example",
            "javascript:alert(1)",
        ] {
            assert_eq!(parse_origin(bad), None, "{bad:?}");
        }
    }
}
