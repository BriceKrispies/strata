//! Creating a tenant while the process runs.
//!
//! The operator's tools post a tenant bundle and the owner's address. The tenant is installed as
//! at start (see [`crate::seed::install_bundle`]), the owner gets a login named by that address,
//! and a message to it says where to sign in. The login has a random password nobody knows, so
//! the way in is the emailed sign-in link of [`crate::signin::link`]: the owner asks for a link
//! at the site's sign-in page with that address, and the link goes to the address and to no one
//! else. The address is the operator's word that the person controls it, so the account counts it
//! as verified.

use crate::app::{ApiError, App, LOG};
use crate::auth::{hash_password, random_hex};
use crate::mail::Mail;
use crate::seed::install_bundle;
use crate::signin::{link::site_origin, normalise_login};
use serde::Deserialize;
use serde_json::{json, Value as Json};
use strata_protocol::api::ErrorCode;
use strata_storage_api::Account;
use strata_substrate::bundle::Bundle;

/// The request of `/internal/platform/tenants`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateTenant {
    /// The tenant bundle, in the form of a `tenant.json` without `template` or `$file` parts.
    pub bundle: Json,
    /// The address of the bundle's owner. It becomes the owner's login.
    pub owner_email: String,
    /// The name shown for the owner. The address when absent.
    #[serde(default)]
    pub owner_name: Option<String>,
    /// The origin of the tenant's site, `http(s)://host[:port]`, where the owner signs in.
    pub origin: String,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Invalid, message)
}

/// Install the tenant and give its owner a login and a message. A tenant that cannot be made
/// whole is removed, so a retry starts from nothing.
pub fn create_tenant(app: &App, request: CreateTenant) -> Result<Json, ApiError> {
    let bundle = Bundle::from_json(&request.bundle.to_string()).map_err(invalid)?;
    let login = normalise_login(&request.owner_email);
    if !login.contains('@') || login.len() > 254 {
        return Err(invalid("The owner needs an email address."));
    }
    let origin = site_origin(&request.origin)
        .ok_or_else(|| invalid("The origin must be like https://host."))?;
    let tenant = bundle.id.clone();
    if app.store.load(&tenant, &[])?.tenant.is_some() {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            format!("The tenant {tenant} exists."),
        ));
    }
    let steps = install_bundle(app, &bundle)
        .map_err(|problem| ApiError::hidden(ErrorCode::Internal, problem))?;
    let account = Account {
        tenant: tenant.clone(),
        login: login.clone(),
        principal: bundle.owner.clone(),
        display_name: request.owner_name.unwrap_or_else(|| login.clone()),
        // A password no one knows: the owner signs in by the emailed link.
        password_hash: hash_password(&random_hex(32)?)?,
        verified_email: Some(login.clone()),
    };
    if let Err(error) = app.store.put_account(&account) {
        let _ = app.store.reset_tenant(&tenant);
        return Err(error.into());
    }
    let mail = Mail {
        to: login.clone(),
        subject: format!("Your site {} is ready", bundle.name),
        text: format!(
            "Your site is ready. Open {origin}/login, give this address and ask for a sign-in \
             link. The link is sent to this address and works once."
        ),
    };
    let notified = match app.cfg.mail.send(&mail) {
        Ok(()) => true,
        Err(error) => {
            LOG.error(
                "provision.not_sent",
                "",
                json!({ "tenant": tenant.as_str(), "error": error.0 }),
            );
            false
        }
    };
    app.audit(
        "platform.tenant_created",
        "",
        json!({ "tenant": tenant.as_str(), "owner": bundle.owner.as_str(), "notified": notified }),
    );
    Ok(json!({
        "tenant": tenant.as_str(),
        "intents": steps,
        "owner_login": login,
        "notified": notified,
    }))
}
