//! Deterministic fixtures: installing tenant bundles into the store.
//!
//! A bundle is installed by issuing its intents through the same `execute` path as any request,
//! with the timestamps written in the bundle. Installing the same bundles into an empty store
//! therefore always produces the same state, byte for byte.
//!
//! Development logins (`accounts.dev.json`) have passwords published in the repository. They are
//! installed only in development mode, and every start outside development mode takes away any
//! that a development run left in the same store, together with their sessions.

use crate::app::{App, LOG};
use crate::auth::{is_password, put_dev_account};
use crate::ops::run_intent;
use serde_json::json;
use strata_kernel::{Intent, PrincipalId, TenantId};
use strata_protocol::tenants::{load_all, load_each, LoadedTenant};
use strata_substrate::bundle::{install, Bundle};

/// Install every bundle whose tenant does not exist yet, and install or withdraw the development
/// logins as the mode says. Returns the tenants it created.
///
/// An install a previous process left unfinished (its directory row is `provisioning`) is rolled
/// back first, so its bundle is installed again from nothing below. One bundle that cannot be
/// loaded or installed is logged and skipped: the other tenants still start.
pub fn seed(app: &App) -> Result<Vec<String>, String> {
    for tenant in app
        .store
        .rollback_provisioning()
        .map_err(|e| e.to_string())?
    {
        LOG.warn(
            "seed.rolled_back",
            "",
            json!({ "tenant": tenant.as_str(), "reason": "install was not finished" }),
        );
    }
    let mut created = Vec::new();
    for (dir, loaded) in load_each(&app.cfg.tenants_dir)? {
        let outcome = loaded.and_then(|loaded| {
            let made = install_one(app, &loaded)?;
            Ok((loaded.bundle.id.as_str().to_string(), made))
        });
        match outcome {
            Ok((id, true)) => created.push(id),
            Ok((_, false)) => {}
            Err(error) => LOG.error(
                "seed.tenant_failed",
                "",
                json!({ "bundle": dir.display().to_string(), "error": error }),
            ),
        }
    }
    Ok(created)
}

/// Install a bundle into a store that has no such tenant, through the same `execute` path as any
/// request. The directory marks the tenant `provisioning` from the first commit (creating it)
/// until the last, so a crash in between leaves a state that the next start rolls back. A step
/// that fails removes what was installed, so a tenant is either whole or absent. Returns the
/// number of intents issued.
pub fn install_bundle(app: &App, bundle: &Bundle) -> Result<usize, String> {
    let tenant = bundle.id.clone();
    let mut begun = false;
    let result = install(bundle, &mut |ctx, intent| {
        let creates = matches!(intent, Intent::CreateTenant { .. });
        let output = run_intent(app, &ctx, &intent, false)
            .map(|done| done.transition.output)
            .map_err(|e| e.describe())?;
        if creates {
            begun = true;
            app.store
                .begin_provisioning(&ctx.tenant)
                .map_err(|e| e.to_string())?;
        }
        Ok(output)
    });
    match result {
        Ok(steps) => {
            app.store
                .finish_provisioning(&tenant)
                .map_err(|e| e.to_string())?;
            Ok(steps)
        }
        Err(error) => {
            // Only a tenant this call created is removed; a refused first step touches nothing.
            if begun {
                let _ = app.store.reset_tenant(&tenant);
            }
            Err(format!("installing {tenant}: {error}"))
        }
    }
}

/// Install one tenant's bundle if the tenant does not exist, then its development logins.
/// Returns whether the tenant was created.
fn install_one(app: &App, loaded: &LoadedTenant) -> Result<bool, String> {
    let tenant = loaded.bundle.id.clone();
    let exists = app
        .store
        .load(&tenant, &[])
        .map_err(|e| e.to_string())?
        .tenant
        .is_some();
    if !exists {
        let steps = install_bundle(app, &loaded.bundle)?;
        LOG.info(
            "seed.installed",
            "",
            json!({ "tenant": tenant.as_str(), "intents": steps }),
        );
    }
    if app.cfg.dev {
        install_dev_accounts(app, &tenant, loaded)?;
    } else {
        withdraw_dev_accounts(app, &tenant, loaded)?;
    }
    Ok(!exists)
}

fn install_dev_accounts(app: &App, tenant: &TenantId, loaded: &LoadedTenant) -> Result<(), String> {
    for account in &loaded.dev_accounts {
        let known = app
            .store
            .account(tenant, &account.login.to_lowercase())
            .map_err(|e| e.to_string())?
            .is_some();
        if known {
            continue;
        }
        let principal = PrincipalId::new(account.principal.clone()).map_err(|e| e.to_string())?;
        put_dev_account(
            app,
            tenant,
            &account.login,
            &account.password,
            principal,
            &account.display_name,
            account.email.as_deref(),
        )
        .map_err(|e| e.describe())?;
        LOG.info(
            "seed.dev_account",
            "",
            json!({ "tenant": tenant.as_str(), "login": account.login }),
        );
    }
    Ok(())
}

/// Outside development mode: remove every stored login that still opens with a published
/// development password, and end its sessions. A login of the same name whose password has been
/// changed is no longer a published credential and is left alone.
fn withdraw_dev_accounts(
    app: &App,
    tenant: &TenantId,
    loaded: &LoadedTenant,
) -> Result<(), String> {
    for account in &loaded.dev_accounts {
        let login = account.login.to_lowercase();
        let Some(stored) = app
            .store
            .account(tenant, &login)
            .map_err(|e| e.to_string())?
        else {
            continue;
        };
        if !is_password(&account.password, &stored.password_hash) {
            continue;
        }
        app.store
            .remove_account(tenant, &login)
            .map_err(|e| e.to_string())?;
        let now = app.now();
        for mut session in app.store.live_sessions(now).map_err(|e| e.to_string())? {
            if session.tenant == *tenant && session.principal == stored.principal {
                session.revoked_at = Some(now);
                app.store.put_session(&session).map_err(|e| e.to_string())?;
            }
        }
        LOG.warn(
            "seed.dev_account_withdrawn",
            "",
            json!({ "tenant": tenant.as_str(), "login": login }),
        );
    }
    Ok(())
}

/// Remove one tenant with everything that is its own, and install its bundle again. Other
/// tenants are not touched.
pub fn reset_tenant(app: &App, tenant: &TenantId) -> Result<bool, String> {
    let loaded = load_all(&app.cfg.tenants_dir)?
        .into_iter()
        .find(|loaded| loaded.bundle.id == *tenant)
        .ok_or_else(|| format!("no bundle for {tenant}"))?;
    app.store.reset_tenant(tenant).map_err(|e| e.to_string())?;
    install_one(app, &loaded)
}
