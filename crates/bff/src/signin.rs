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
    /// this method. Anything else in the body is dropped here.
    fn credentials(&self, body: &Json) -> Option<Json>;
}

/// A login and a password.
struct Password;

impl SignInRoute for Password {
    fn method(&self) -> &'static str {
        "password"
    }

    fn credentials(&self, body: &Json) -> Option<Json> {
        Some(json!({
            "login": body.get("login")?.as_str()?,
            "password": body.get("password")?.as_str()?,
        }))
    }
}

/// Every method the site offers.
static ROUTES: &[&dyn SignInRoute] = &[&Password];

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
            )
            .expect("a complete request");
        assert_eq!(sent, json!({ "login": "ines", "password": "x" }));
        assert!(route.credentials(&json!({ "login": "ines" })).is_none());
        assert!(route
            .credentials(&json!({ "login": 4, "password": "x" }))
            .is_none());
        assert!(super::route("telepathy").is_none());
    }
}
