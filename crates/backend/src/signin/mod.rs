//! Sign-in methods, behind one seam.
//!
//! A method is a module that implements [`SignInMethod`] and is listed in [`METHODS`]: it says
//! which account an attempt is for and whether the credential is right. Everything that is the
//! same for every method happens here, once, in [`sign_in`]: the limits on failed attempts, the
//! uniform refusal, the audit entry and the start of the session. A passkey, an emailed link or
//! an identity provider is one more module and one more line in the list.
//!
//! Implements: IDN-001

pub mod link;
pub mod passkey;
pub mod password;

use crate::app::{ApiError, App, LOG};
use crate::auth::{self, Caller};
use serde_json::{json, Value as Json};
use strata_kernel::canon;
use strata_protocol::api::{ErrorCode, LoginReply};
use strata_storage_api::{Account, ThrottleRule, ThrottleSettle, ThrottleVerdict};
use unicode_normalization::UnicodeNormalization;

/// One way to sign in.
pub trait SignInMethod: Send + Sync {
    /// The name in the route, the audit entry and the telemetry line: `password`.
    fn name(&self) -> &'static str;

    /// The account an attempt names, in the form the limits count it, or `None` when the request
    /// is not one this method could accept (a field missing or too long).
    fn subject(&self, request: &Json) -> Option<String>;

    /// Whether the credential is right for the subject. It costs the same when the subject has
    /// no account as when the credential is wrong (IDN-255). `Ok(None)` is a refusal; an error
    /// is the store failing.
    fn verify(
        &self,
        app: &App,
        caller: &Caller,
        subject: &str,
        request: &Json,
    ) -> Result<Option<Account>, ApiError>;
}

/// Every method the backend offers.
static METHODS: &[&dyn SignInMethod] = &[
    &password::Password,
    &passkey::PasskeyMethod,
    &link::LinkMethod,
];

pub fn method(name: &str) -> Option<&'static dyn SignInMethod> {
    METHODS.iter().copied().find(|method| method.name() == name)
}

/// A login in the one form that is stored, looked up and counted: Unicode compatibility
/// normalised (NFKC), lower-cased, normalised again because folding case can produce text that
/// is not normalised, and trimmed. Two spellings that a person would read as one login, such as
/// full-width letters, a ligature or a no-break space, come out equal, so none of them is a way
/// round a limit on the account.
///
/// Uses the `unicode-normalization` crate, which implements the Unicode normalisation forms.
pub fn normalise_login(raw: &str) -> String {
    let folded: String = raw.nfkc().collect::<String>().to_lowercase();
    folded.nfkc().collect::<String>().trim().to_string()
}

/// The one refusal of a method, which says nothing about which part of the attempt was wrong.
fn refused(method: &str) -> ApiError {
    ApiError::new(
        ErrorCode::Unauthenticated,
        match method {
            "passkey" => "The passkey was not accepted.",
            "link" => "The link was not accepted. Ask for a new one.",
            _ => "The name or password is not right.",
        },
    )
}

/// One answer for every limit that can stop an attempt, so that it does not say which one did.
pub(crate) fn throttled() -> ApiError {
    ApiError::new(
        ErrorCode::Throttled,
        "Too many attempts. Wait a while and try again.",
    )
}

/// The limits an attempt counts against: the account, the source across all accounts and the
/// realm across all sources. Each key is scoped by the tenant in the store.
pub(crate) fn rules(app: &App, method: &str, subject: &str, source: &str) -> [ThrottleRule; 3] {
    let limits = &app.cfg.throttle;
    [
        ThrottleRule {
            // The account is hashed: the key is the same length whatever was typed, and the
            // store holds no login that does not exist.
            key: format!("account:{method}:{}", canon::hash_bytes(subject.as_bytes())),
            max: limits.account_max,
            window_ms: limits.window_ms,
            lock_ms: limits.account_lock_ms,
            lock_resets: true,
        },
        ThrottleRule {
            key: format!("source:{method}:{source}"),
            max: limits.source_max,
            window_ms: limits.window_ms,
            lock_ms: limits.window_ms,
            lock_resets: false,
        },
        ThrottleRule {
            key: format!("realm:{method}"),
            max: limits.realm_max,
            window_ms: limits.window_ms,
            lock_ms: limits.realm_interval_ms,
            lock_resets: false,
        },
    ]
}

/// Check a sign-in attempt with the named method and start a session.
///
/// The attempt is counted against its limits first, in one atomic step of the store, and only
/// then is the credential checked. So attempts that arrive together are counted together: no
/// more than the limit are ever checked, whatever the timing. A right credential gives its slot
/// back; a wrong one keeps it.
///
/// Implements: IDN-008, IDN-250, IDN-251, IDN-255
pub fn sign_in(
    app: &App,
    caller: &Caller,
    method_name: &str,
    request: &Json,
) -> Result<LoginReply, ApiError> {
    let Some(method) = method(method_name) else {
        return Err(ApiError::new(
            ErrorCode::NotFound,
            "No such sign-in method.",
        ));
    };
    let name = method.name();
    let Some(tenant) = app.store.load(&caller.site, &[])?.tenant else {
        return Err(ApiError::new(ErrorCode::NotFound, "no such site"));
    };
    let Some(subject) = method.subject(request) else {
        app.decision(name, "refused", &["well-formed"], &caller.correlation);
        return Err(refused(name));
    };

    let rules = rules(app, name, &subject, &caller.source);
    if let ThrottleVerdict::Refused { .. } =
        app.store
            .throttle_reserve(&caller.site, &rules, app.now())?
    {
        app.decision(
            name,
            "throttled",
            &["well-formed", "throttle"],
            &caller.correlation,
        );
        LOG.warn(
            "login.throttled",
            &caller.correlation,
            json!({ "tenant": caller.site.as_str() }),
        );
        return Err(throttled());
    }

    let Some(account) = method.verify(app, caller, &subject, request)? else {
        app.decision(
            name,
            "refused",
            &["well-formed", "throttle", "credential"],
            &caller.correlation,
        );
        LOG.warn(
            "login.refused",
            &caller.correlation,
            json!({ "tenant": caller.site.as_str() }),
        );
        return Err(refused(name));
    };

    // The credential was right. The account forgets its failures; the source and the realm
    // only get this attempt's slot back, so that a right login does not wipe what a spray
    // from the same place has used up.
    let [account_rule, source_rule, realm_rule] = &rules;
    app.store.throttle_settle(
        &caller.site,
        &account_rule.key,
        account_rule.max,
        ThrottleSettle::Clear,
    )?;
    for rule in [source_rule, realm_rule] {
        app.store
            .throttle_settle(&caller.site, &rule.key, rule.max, ThrottleSettle::Refund)?;
    }
    app.decision(
        name,
        "accepted",
        &["well-formed", "throttle", "credential"],
        &caller.correlation,
    );
    auth::start_session(app, caller, &tenant, account, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: IDN-008
    #[test]
    fn spellings_of_one_login_normalise_to_one() {
        let one = normalise_login("ines");
        for variant in [
            " Ines ",
            "INES",
            "\u{FF49}\u{FF4E}\u{FF45}\u{FF53}",
            "\u{00A0}ines\u{00A0}",
            "\u{3000}Ines",
            "i\u{006E}es\n",
        ] {
            assert_eq!(normalise_login(variant), one, "{variant:?}");
        }
        assert_eq!(normalise_login("\u{FB01}x"), "fix", "a ligature");
        assert_ne!(normalise_login("ines2"), one);
        assert!(normalise_login("   ").is_empty());
    }

    #[test]
    fn the_registry_names_each_method_once() {
        let names: Vec<&str> = METHODS.iter().map(|m| m.name()).collect();
        let unique: std::collections::BTreeSet<_> = names.iter().collect();
        assert_eq!(names.len(), unique.len());
        assert!(method("password").is_some());
        assert!(method("passkey").is_some());
        assert!(method("link").is_some());
        assert!(method("telepathy").is_none());
    }
}
