//! Sign in with a login and a password.

use super::{normalise_login, SignInMethod};
use crate::app::{ApiError, App};
use crate::auth::{decoy_hash, is_password, Caller};
use serde_json::Value as Json;
use strata_storage_api::Account;

pub struct Password;

/// The longest login and password a request may carry.
const LOGIN_BYTES: usize = 64;
const PASSWORD_BYTES: usize = 256;

impl SignInMethod for Password {
    fn name(&self) -> &'static str {
        "password"
    }

    fn subject(&self, request: &Json) -> Option<String> {
        let login = request.get("login")?.as_str()?;
        let password = request.get("password")?.as_str()?;
        // Normalising can lengthen a login, so the raw one is bounded too.
        if login.len() > 4 * LOGIN_BYTES || password.len() > PASSWORD_BYTES {
            return None;
        }
        let login = normalise_login(login);
        (!login.is_empty() && login.len() <= LOGIN_BYTES).then_some(login)
    }

    /// Implements: FED-009, H-038, IDN-255
    fn verify(
        &self,
        app: &App,
        caller: &Caller,
        subject: &str,
        request: &Json,
    ) -> Result<Option<Account>, ApiError> {
        let password = request
            .get("password")
            .and_then(Json::as_str)
            .unwrap_or_default();
        // A login in a domain that a provider decides for does not open with a password. The
        // work is the same as for a wrong one (FED-009, IDN-255).
        if app.cfg.federation.domain_bound(&caller.site, subject) {
            is_password(password, decoy_hash());
            return Ok(None);
        }
        let account = app.store.account(&caller.site, subject)?;
        Ok(match account {
            Some(account) => is_password(password, &account.password_hash).then_some(account),
            None => {
                // The same work as for a wrong password, so that the time it takes does not say
                // that there is no such account.
                is_password(password, decoy_hash());
                None
            }
        })
    }
}
