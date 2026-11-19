//! Sign-in methods at the edge, behind one seam.
//!
//! A method is a module that implements [`SignInRoute`] and is listed in [`ROUTES`]: it says
//! what of the browser's request is passed to the backend as the method's credentials. What is
//! the same for every method, the origin and request-token checks, the call to the backend and
//! the session cookie, is done once in `routes::sign_in`.
//!
//! Implements: IDN-001

use serde_json::{json, Value as Json};

/// One way to sign in, as the browser asks for it.
pub trait SignInRoute: Send + Sync {
    /// The name in the address, `/api/sign-in/<name>`, and in the backend's.
    fn method(&self) -> &'static str;

    /// What of the browser's body goes to the backend, or `None` when it is not a request of
    /// this method. Anything else in the body is dropped here. `binding` is the nonce the
    /// browser holds in its link cookie, for the one method that uses it.
    fn credentials(&self, body: &Json, binding: Option<&str>) -> Option<Json>;

    /// What a request that is not one of this method's is told.
    fn refusal(&self) -> &'static str {
        "The name or password is not right."
    }
}

/// A login and a password.
struct Password;

impl SignInRoute for Password {
    fn method(&self) -> &'static str {
        "password"
    }

    fn credentials(&self, body: &Json, _binding: Option<&str>) -> Option<Json> {
        Some(json!({
            "login": body.get("login")?.as_str()?,
            "password": body.get("password")?.as_str()?,
        }))
    }
}

/// A passkey: the challenge that was issued and the browser's answer to it.
struct Passkey;

impl SignInRoute for Passkey {
    fn method(&self) -> &'static str {
        "passkey"
    }

    fn credentials(&self, body: &Json, _binding: Option<&str>) -> Option<Json> {
        body.get("assertion")?.as_object()?;
        Some(json!({
            "challenge": body.get("challenge")?.as_str()?,
            "assertion": body.get("assertion")?,
        }))
    }

    fn refusal(&self) -> &'static str {
        "The passkey was not accepted."
    }
}

/// An emailed link: the challenge and token the link carried, and the nonce of the browser that
/// asked for it, which is in a cookie and not in the body.
struct Link;

impl SignInRoute for Link {
    fn method(&self) -> &'static str {
        "link"
    }

    fn credentials(&self, body: &Json, binding: Option<&str>) -> Option<Json> {
        Some(json!({
            "challenge": body.get("challenge")?.as_str()?,
            "token": body.get("token")?.as_str()?,
            "binding": binding?,
        }))
    }

    fn refusal(&self) -> &'static str {
        "The link was not accepted. Ask for a new one."
    }
}

/// An identity provider: the `state` and `code` the provider sent the browser back with, and the
/// nonce of the browser that started the sign-in, which is in a cookie and not in the body.
struct Oidc;

impl SignInRoute for Oidc {
    fn method(&self) -> &'static str {
        "oidc"
    }

    fn credentials(&self, body: &Json, binding: Option<&str>) -> Option<Json> {
        Some(json!({
            "state": body.get("state")?.as_str()?,
            "code": body.get("code")?.as_str()?,
            "binding": binding?,
        }))
    }

    fn refusal(&self) -> &'static str {
        "The sign-in with the identity provider was not accepted."
    }
}

/// A SAML provider: the `RelayState` and the response its page posted to the assertion consumer
/// address, which the gateway moved into the fragment of the sign-in page, and the nonce of the
/// browser that started the sign-in, which is in a cookie and not in the body.
struct Saml;

impl SignInRoute for Saml {
    fn method(&self) -> &'static str {
        "saml"
    }

    fn credentials(&self, body: &Json, binding: Option<&str>) -> Option<Json> {
        Some(json!({
            "relay_state": body.get("relay_state")?.as_str()?,
            "response": body.get("response")?.as_str()?,
            "binding": binding?,
        }))
    }

    fn refusal(&self) -> &'static str {
        "The sign-in with the identity provider was not accepted."
    }
}

/// Every method the site offers.
static ROUTES: &[&dyn SignInRoute] = &[&Password, &Passkey, &Link, &Oidc, &Saml];

pub fn route(method: &str) -> Option<&'static dyn SignInRoute> {
    ROUTES
        .iter()
        .copied()
        .find(|route| route.method() == method)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_method_passes_on_only_its_own_fields() {
        let route = route("password").expect("password is offered");
        let sent = route
            .credentials(
                &json!({ "login": "ines", "password": "x", "tenant": "other", "admin": true }),
                None,
            )
            .expect("a complete request");
        assert_eq!(sent, json!({ "login": "ines", "password": "x" }));
        assert!(route
            .credentials(&json!({ "login": "ines" }), None)
            .is_none());
        assert!(route
            .credentials(&json!({ "login": 4, "password": "x" }), None)
            .is_none());
        assert!(super::route("telepathy").is_none());
    }

    #[test]
    fn a_passkey_passes_on_the_challenge_and_the_assertion_only() {
        let route = route("passkey").expect("passkey is offered");
        let sent = route
            .credentials(
                &json!({ "challenge": "c", "assertion": { "id": "x" }, "tenant": "other" }),
                None,
            )
            .expect("a complete request");
        assert_eq!(
            sent,
            json!({ "challenge": "c", "assertion": { "id": "x" } })
        );
        assert!(route
            .credentials(&json!({ "challenge": "c" }), None)
            .is_none());
        assert!(route
            .credentials(&json!({ "challenge": 4, "assertion": {} }), None)
            .is_none());
    }

    #[test]
    fn an_identity_provider_sign_in_takes_its_browser_binding_from_the_cookie_and_never_from_the_body(
    ) {
        let route = route("oidc").expect("identity providers are offered");
        let body =
            json!({ "state": "s", "code": "c", "binding": "from-the-body", "tenant": "other" });
        assert_eq!(
            route.credentials(&body, Some("from-the-cookie")),
            Some(json!({ "state": "s", "code": "c", "binding": "from-the-cookie" }))
        );
        assert!(
            route.credentials(&body, None).is_none(),
            "a browser with no cookie is not the one that started it"
        );
        assert!(route
            .credentials(&json!({ "state": "s" }), Some("x"))
            .is_none());
    }

    #[test]
    fn a_saml_sign_in_takes_its_browser_binding_from_the_cookie_and_never_from_the_body() {
        let route = route("saml").expect("SAML providers are offered");
        let body = json!({
            "relay_state": "r",
            "response": "PFJlc3BvbnNlLz4=",
            "binding": "from-the-body",
            "tenant": "other",
        });
        assert_eq!(
            route.credentials(&body, Some("from-the-cookie")),
            Some(json!({
                "relay_state": "r",
                "response": "PFJlc3BvbnNlLz4=",
                "binding": "from-the-cookie",
            }))
        );
        assert!(
            route.credentials(&body, None).is_none(),
            "a browser with no cookie is not the one that started it"
        );
        assert!(route
            .credentials(&json!({ "relay_state": "r" }), Some("x"))
            .is_none());
    }

    #[test]
    fn a_link_takes_its_browser_binding_from_the_cookie_and_never_from_the_body() {
        let route = route("link").expect("links are offered");
        let body = json!({ "challenge": "c", "token": "t", "binding": "from-the-body" });
        assert_eq!(
            route.credentials(&body, Some("from-the-cookie")),
            Some(json!({ "challenge": "c", "token": "t", "binding": "from-the-cookie" }))
        );
        assert!(
            route.credentials(&body, None).is_none(),
            "a browser with no cookie is not the one that asked"
        );
    }
}
