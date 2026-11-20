//! Sign in with a SAML 2.0 identity provider, started by this service.
//!
//! The flow is the web browser SSO profile, service-provider initiated (D-8, FED-007):
//!
//! 1. **Start.** The person chooses a provider on the sign-in page. The backend keeps a pending
//!    request, found by the hash of a `RelayState` value, that holds the identifier of the
//!    authentication request it is about to send, the hash of a binding nonce and the address the
//!    sign-in started at, and answers with the provider's sign-on address carrying the
//!    authentication request (HTTP-Redirect binding: deflated, base64, in the query). The binding
//!    nonce goes to the starting browser as a `__Host-` cookie and is in no address.
//! 2. **Response.** The provider's page posts the response to this service's assertion consumer
//!    address (HTTP-POST binding). The gateway moves it into the fragment of the sign-in page, and
//!    the page sends it, from the site's own origin and with the request token every sign-in
//!    carries, to `/api/sign-in/saml`. The pending request is taken out of the store in the step
//!    that reads it, so a response answers once, and it is accepted only through its lifetime,
//!    from the browser that started it and at the address it started at.
//! 3. **Assertion.** The response must answer that request (`InResponseTo`), and its assertion is
//!    checked as `assertion` states. Its identifier is then remembered until the assertion would
//!    no longer be accepted anyway, so a second presentation is refused (FED-006).
//! 4. **Policy.** What the verified assertion means for the realm is `federated::resolve`'s, the
//!    same for every protocol: domain binding, linking and first sign-in.
//!
//! A response that answers no pending request is refused. Sign-in started by the provider is not
//! offered at all, which is what the specification asks of a tenant that has not turned it on
//! (FED-007).
//!
//! Implements: FED-005, FED-006, FED-007, FED-008, FED-009, FED-011, FED-012, FED-251, FED-252,
//! FED-253, FED-254, FED-257, FED-258, H-016, H-050, H-051, H-052, D-8

pub mod assertion;
pub mod config;

pub use config::SamlProvider;

use super::federated::{self, Admission};
use super::link::{encode, same_site_path, site_origin, well_formed};
use super::{start_rules, stepup, throttled, SignInMethod};
use crate::app::{ApiError, App, LOG};
use crate::auth::{identify, random_hex, same_secret, token_hash, Caller, Identity};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Value as Json};
use std::io::Write;
use strata_kernel::Timestamp;
use strata_protocol::api::ErrorCode;
use strata_storage_api::{Account, SamlRequest, ThrottleVerdict};

pub struct SamlMethod;

/// The path at the site that a provider posts the response to.
pub const ACS_PATH: &str = "/federation/saml/acs";
/// The most characters of a base64 response that are read: the largest response, as base64.
const RESPONSE_CHARS: usize = assertion::MAX_XML_BYTES / 3 * 4 + 64;

fn now_secs(app: &App) -> i64 {
    app.now().0 / 1000
}

fn refuse(caller: &Caller, provider: &str, reason: &str) -> Result<Option<Account>, ApiError> {
    federated::refuse(caller, "saml", provider, reason)
}

/// The part of a provider's configuration that decides who gets in, for the policy the protocols
/// share (`federated::resolve`).
fn admission(provider: &SamlProvider) -> Admission<'_> {
    Admission {
        protocol: "saml",
        provider: &provider.id,
        domains: &provider.domains,
        create_on_first_sign_in: provider.create_on_first_sign_in,
        default_role: provider.default_role.as_deref(),
        link_verified_email: provider.link_verified_email,
    }
}

/// The SAML providers the sign-in page offers for the address it is served at (FED-010).
///
/// Implements: FED-010
pub fn providers(app: &App, caller: &Caller) -> Result<Json, ApiError> {
    let list: Vec<Json> = app
        .cfg
        .federation
        .saml_providers(&caller.site)
        .iter()
        .filter(|provider| provider.enabled)
        .map(|provider| json!({ "id": provider.id, "label": provider.label }))
        .collect();
    Ok(json!({ "providers": list }))
}

/// A time as SAML writes it: UTC, to the second.
fn format_instant(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    // Civil date from a day count (Howard Hinnant's algorithm, proleptic Gregorian).
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Text as an XML attribute value or text node holds it.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// The authentication request: who is asking, where the response goes and how.
fn authn_request(
    id: &str,
    issued: i64,
    destination: &str,
    acs_url: &str,
    entity_id: &str,
) -> String {
    format!(
        "<samlp:AuthnRequest xmlns:samlp=\"{}\" xmlns:saml=\"{}\" ID=\"{}\" Version=\"2.0\" \
         IssueInstant=\"{}\" Destination=\"{}\" \
         ProtocolBinding=\"urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST\" \
         AssertionConsumerServiceURL=\"{}\"><saml:Issuer>{}</saml:Issuer>\
         <samlp:NameIDPolicy AllowCreate=\"true\"/></samlp:AuthnRequest>",
        assertion::SAMLP,
        assertion::SAML,
        escape(id),
        format_instant(issued),
        escape(destination),
        escape(acs_url),
        escape(entity_id),
    )
}

/// The message of the HTTP-Redirect binding: raw DEFLATE, then base64.
fn redirect_message(xml: &str) -> Result<String, ApiError> {
    let unavailable =
        |why: std::io::Error| ApiError::hidden(ErrorCode::Internal, format!("deflate: {why}"));
    let mut encoder =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(xml.as_bytes()).map_err(unavailable)?;
    let bytes = encoder.finish().map_err(unavailable)?;
    Ok(STANDARD.encode(bytes))
}

/// Begin a sign-in at a provider: keep the pending request and answer with the address to send the
/// browser to. The reply holds the binding nonce for the browser's cookie, which the gateway keeps
/// to itself, and the return address when it is a path on this site (FED-250).
///
/// Implements: FED-007, FED-011, FED-012, FED-250, H-051
pub fn start(
    app: &App,
    caller: &Caller,
    identity: &Identity,
    body: &Json,
) -> Result<Json, ApiError> {
    let Some(origin) = caller.origin.as_deref().and_then(site_origin) else {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "This request did not come from the site.",
        ));
    };
    let not_available = || ApiError::new(ErrorCode::Invalid, "That provider is not available.");
    let id = body
        .get("provider")
        .and_then(Json::as_str)
        .filter(|id| id.len() <= 32)
        .ok_or_else(not_available)?;
    let provider = app
        .cfg
        .federation
        .saml_provider(&caller.site, id)
        .filter(|provider| provider.enabled)
        .ok_or_else(not_available)?;
    if !config::sso_ok(&provider.sso_url, app.cfg.dev) {
        LOG.warn(
            "saml.provider_unreachable",
            &caller.correlation,
            json!({ "tenant": caller.site.as_str(), "provider": provider.id, "error": "sso_url" }),
        );
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            "That provider did not answer. Use another way to sign in.",
        ));
    }

    let link = body.get("link").and_then(Json::as_bool).unwrap_or(false);
    let principal = match (&identity.session, link) {
        (Some(session), true) => {
            // Linking a way of signing in needs a recent authentication, as adding a factor does.
            stepup::require_fresh(app, caller, identity)?;
            Some(session.principal.clone())
        }
        (None, true) => {
            return Err(ApiError::new(
                ErrorCode::Unauthenticated,
                "Sign in to link another way of signing in.",
            ))
        }
        _ => None,
    };

    let rules = start_rules(app, "saml-start", &caller.source);
    if let ThrottleVerdict::Refused { .. } =
        app.store
            .throttle_reserve(&caller.site, &rules, app.now())?
    {
        return Err(throttled());
    }

    let relay_state = random_hex(32)?;
    let request_id = format!("_{}", random_hex(16)?);
    let binding = match body.get("binding").and_then(Json::as_str) {
        Some(binding) if well_formed(binding) => binding.to_string(),
        _ => random_hex(32)?,
    };
    app.store.put_saml_request(&SamlRequest {
        tenant: caller.site.clone(),
        id_hash: token_hash(&relay_state),
        provider: provider.id.clone(),
        request_id: request_id.clone(),
        binding_hash: token_hash(&binding),
        origin: origin.clone(),
        principal,
        expires_at: Timestamp(app.now().0 + app.cfg.passkey_challenge_ttl_ms),
    })?;

    let acs_url = format!("{origin}{ACS_PATH}");
    let xml = authn_request(
        &request_id,
        now_secs(app),
        &provider.sso_url,
        &acs_url,
        &provider.entity_id(&origin),
    );
    let message = redirect_message(&xml)?;
    let separator = if provider.sso_url.contains('?') {
        '&'
    } else {
        '?'
    };
    let ttl = (app.cfg.passkey_challenge_ttl_ms / 1000).max(1);
    LOG.info(
        "saml.started",
        &caller.correlation,
        json!({ "tenant": caller.site.as_str(), "provider": provider.id, "link": link }),
    );
    let mut reply = json!({
        "redirect": format!(
            "{}{separator}SAMLRequest={}&RelayState={}",
            provider.sso_url,
            encode(&message),
            encode(&relay_state),
        ),
        "binding": binding,
        "max_age": ttl,
    });
    if let Some(next) = body
        .get("next")
        .and_then(Json::as_str)
        .and_then(same_site_path)
    {
        reply["return_to"] = json!(next);
    }
    Ok(reply)
}

/// This service's metadata for a provider at an origin: the entity identifier and the assertion
/// consumer address the provider is to be configured with, and that assertions must be signed.
/// Anyone may ask; it holds nothing secret.
///
/// Implements: FED-008, CT-069, H-051
pub fn metadata(app: &App, caller: &Caller, body: &Json) -> Result<Json, ApiError> {
    let not_found = || ApiError::new(ErrorCode::NotFound, "No such provider.");
    let provider = body
        .get("provider")
        .and_then(Json::as_str)
        .filter(|id| id.len() <= 32)
        .and_then(|id| app.cfg.federation.saml_provider(&caller.site, id))
        .filter(|provider| provider.enabled)
        .ok_or_else(not_found)?;
    let origin = body
        .get("origin")
        .and_then(Json::as_str)
        .and_then(site_origin)
        .ok_or_else(not_found)?;
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <md:EntityDescriptor xmlns:md=\"urn:oasis:names:tc:SAML:2.0:metadata\" entityID=\"{}\">\
         <md:SPSSODescriptor AuthnRequestsSigned=\"false\" WantAssertionsSigned=\"true\" \
         protocolSupportEnumeration=\"urn:oasis:names:tc:SAML:2.0:protocol\">\
         <md:NameIDFormat>urn:oasis:names:tc:SAML:1.1:nameid-format:unspecified</md:NameIDFormat>\
         <md:AssertionConsumerService Binding=\"urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST\" \
         Location=\"{}{ACS_PATH}\" index=\"0\" isDefault=\"true\"/>\
         </md:SPSSODescriptor></md:EntityDescriptor>",
        escape(&provider.entity_id(&origin)),
        escape(&origin),
    );
    Ok(json!({ "metadata": xml }))
}

/// The response as the XML text it carries, or `None` when it is not base64 of UTF-8.
fn decode_response(encoded: &str) -> Option<String> {
    let compact: String = encoded
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    let bytes = STANDARD.decode(compact.as_bytes()).ok()?;
    String::from_utf8(bytes).ok()
}

impl SignInMethod for SamlMethod {
    fn name(&self) -> &'static str {
        "saml"
    }

    /// The `RelayState` the response carries. The account is not known until the assertion is
    /// checked, so the limit on an account counts the pending request; the source's and the
    /// realm's count the attempt.
    fn subject(&self, request: &Json) -> Option<String> {
        let text = |name: &str| request.get(name)?.as_str();
        text("response")
            .filter(|response| !response.is_empty() && response.len() <= RESPONSE_CHARS)?;
        text("binding").filter(|binding| well_formed(binding))?;
        text("relay_state")
            .filter(|state| well_formed(state))
            .map(str::to_string)
    }

    /// Implements: FED-005, FED-006, FED-007, FED-008, FED-011, FED-252, FED-254, H-052
    fn verify(
        &self,
        app: &App,
        caller: &Caller,
        subject: &str,
        request: &Json,
    ) -> Result<Option<Account>, ApiError> {
        let text = |name: &str| request.get(name).and_then(Json::as_str).unwrap_or_default();
        // The request is taken in the step that reads it: whatever follows, this response has
        // used it. A response that answers none is refused (FED-007).
        let Some(pending) = app
            .store
            .take_saml_request(&caller.site, &token_hash(subject))?
        else {
            return refuse(caller, "", "no_pending_request");
        };
        // Every check is made, and compared in constant time, whichever one fails.
        let in_time = app.now() <= pending.expires_at;
        let browser = same_secret(&token_hash(text("binding")), &pending.binding_hash);
        let site = caller.origin.as_deref().and_then(site_origin).as_deref()
            == Some(pending.origin.as_str());
        if !(in_time && browser && site) {
            return refuse(caller, &pending.provider, "state");
        }
        let Some(provider) = app
            .cfg
            .federation
            .saml_provider(&caller.site, &pending.provider)
            .filter(|provider| provider.enabled)
        else {
            return refuse(caller, &pending.provider, "provider_not_enabled");
        };
        // A request to link is answered only to the person who made it.
        if let Some(principal) = &pending.principal {
            let identity = identify(app, caller)?;
            if !identity
                .session
                .as_ref()
                .is_some_and(|session| session.principal == *principal)
            {
                return refuse(caller, &provider.id, "link_session");
            }
            if stepup::require_fresh(app, caller, &identity).is_err() {
                return refuse(caller, &provider.id, "link_not_fresh");
            }
        }

        let Some(xml) = decode_response(text("response")) else {
            return refuse(caller, &provider.id, assertion::Refusal::Malformed.code());
        };
        let entity_id = provider.entity_id(&pending.origin);
        let acs_url = format!("{}{ACS_PATH}", pending.origin);
        let verified = match assertion::verify(
            &xml,
            &assertion::Expect {
                provider,
                entity_id: &entity_id,
                acs_url: &acs_url,
                request_id: &pending.request_id,
                now: now_secs(app),
            },
        ) {
            Ok(verified) => verified,
            Err(reason) => return refuse(caller, &provider.id, reason.code()),
        };

        // The identifier is remembered only now that everything else holds, so that a message
        // that is refused cannot use up an identifier, and it is remembered in the step that
        // checks it, so that two presentations at once are one (FED-006).
        let first = app.store.put_saml_assertion(
            &caller.site,
            &provider.id,
            &token_hash(&verified.assertion_id),
            Timestamp(verified.accepted_until * 1000),
        )?;
        if !first {
            return refuse(caller, &provider.id, "saml_replayed");
        }
        federated::resolve(
            app,
            caller,
            &admission(provider),
            pending.principal.as_ref(),
            &verified.who,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_is_written_as_saml_reads_it() {
        assert_eq!(format_instant(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_instant(1_795_084_215), "2026-11-19T10:30:15Z");
        assert_eq!(format_instant(1_709_164_800), "2024-02-29T00:00:00Z");
        for seconds in [0, 1, 86_399, 951_782_400, 1_795_084_215, 4_102_444_799] {
            assert_eq!(
                assertion::instant(&format_instant(seconds)),
                Some(seconds),
                "{seconds}"
            );
        }
    }

    #[test]
    fn what_is_put_in_the_request_is_escaped() {
        let xml = authn_request(
            "_a",
            0,
            "https://idp.example/sso?a=1&b=2",
            "https://site.example/federation/saml/acs",
            "urn:\"x\"<y>",
        );
        assert!(xml.contains("a=1&amp;b=2"), "{xml}");
        assert!(xml.contains("urn:&quot;x&quot;&lt;y&gt;"), "{xml}");
        assert!(uppsala_parses(&xml));
    }

    fn uppsala_parses(xml: &str) -> bool {
        uppsala::parse(xml).is_ok()
    }

    #[test]
    fn a_response_is_base64_of_text_and_may_be_wrapped_in_lines() {
        let encoded = STANDARD.encode("<Response/>");
        let wrapped = format!("{}\r\n{}", &encoded[..4], &encoded[4..]);
        assert_eq!(decode_response(&wrapped).as_deref(), Some("<Response/>"));
        assert_eq!(decode_response("not base64!"), None);
        assert_eq!(decode_response(&STANDARD.encode([0xff, 0xfe])), None);
    }
}
