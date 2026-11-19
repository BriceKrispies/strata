//! Deterministic fixtures: installing tenant bundles into the store.
//!
//! A bundle is installed by issuing its intents through the same `execute` path as any request,
//! with the timestamps written in the bundle. Installing the same bundles into an empty store
//! therefore always produces the same state, byte for byte.

use crate::app::{App, LOG};
use crate::auth::put_dev_account;
use crate::ops::run_intent;
use serde_json::json;
use strata_kernel::PrincipalId;
use strata_protocol::tenants::load_all;
use strata_substrate::bundle::install;

/// Install every bundle whose tenant does not exist yet. Returns the tenants it created.
pub fn seed(app: &App) -> Result<Vec<String>, String> {
    let mut created = Vec::new();
    for loaded in load_all(&app.cfg.tenants_dir)? {
        let tenant = loaded.bundle.id.clone();
        let exists = app
            .store
            .load(&tenant, &[])
            .map_err(|e| e.to_string())?
            .tenant
            .is_some();
        if !exists {
            let steps = install(&loaded.bundle, &mut |ctx, intent| {
                run_intent(app, &ctx, &intent, false)
                    .map(|done| done.transition.output)
                    .map_err(|e| e.0.message)
            })
            .map_err(|e| format!("installing {tenant}: {e}"))?;
            LOG.info(
                "seed.installed",
                "",
                json!({ "tenant": tenant.as_str(), "intents": steps }),
            );
            created.push(tenant.as_str().to_string());
        }
        if app.cfg.dev {
            for account in &loaded.dev_accounts {
                let known = app
                    .store
                    .account(&tenant, &account.login)
                    .map_err(|e| e.to_string())?
                    .is_some();
                if known {
                    continue;
                }
                let principal =
                    PrincipalId::new(account.principal.clone()).map_err(|e| e.to_string())?;
                put_dev_account(
                    app,
                    &tenant,
                    &account.login,
                    &account.password,
                    principal,
                    &account.display_name,
                    account.email.as_deref(),
                )
                .map_err(|e| e.0.message)?;
                LOG.info(
                    "seed.dev_account",
                    "",
                    json!({ "tenant": tenant.as_str(), "login": account.login }),
                );
            }
        }
    }
    Ok(created)
}

/// Remove everything and install the bundles again.
pub fn reset(app: &App) -> Result<Vec<String>, String> {
    app.store.reset().map_err(|e| e.to_string())?;
    seed(app)
}
