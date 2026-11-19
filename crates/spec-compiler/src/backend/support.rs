//! The `support` module: the small types the construct modules share (who is acting, what a
//! policy decides, the error of checked arithmetic in an expression). It does not depend on the
//! specification and is the same in every generated crate. The generated crate depends on no
//! runtime; a deployment's authorizer builds a `Caller` from whatever it
//! authenticated.

use proc_macro2::TokenStream;
use quote::quote;

use super::Backend;
use crate::diagnostic::Diagnostic;
use crate::ir::Spec;

/// The support backend.
pub struct SupportBackend;

impl Backend for SupportBackend {
    fn module(&self) -> &'static str {
        "support"
    }

    fn check(&self, _spec: &Spec) -> Vec<Diagnostic> {
        Vec::new()
    }

    fn emit(&self, _spec: &Spec) -> TokenStream {
        quote! {
            #![doc = " Types shared by the policy, validation, quota, retention, retry and audit modules."]

            use std::fmt;

            /// Who is acting: the principal that policies and audits read.
            #[derive(Debug, Clone, PartialEq, Eq, Hash)]
            pub struct Caller {
                /// The kind of principal, such as `member`, `operator` or `service`.
                pub kind: String,
                /// The principal's id.
                pub id: String,
                /// The tenant the principal acts for, if any.
                pub tenant: Option<String>,
                /// The principal's role, if any.
                pub role: Option<String>,
            }

            /// What a contract's policies decide about a request.
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub enum Decision {
                /// The request is permitted.
                Allow,
                /// The request is refused.
                Deny,
            }

            impl Decision {
                /// True when the request is permitted.
                pub fn is_allowed(self) -> bool {
                    self == Self::Allow
                }
            }

            /// An expression's arithmetic overflowed or divided by zero.
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub struct Overflow;

            impl fmt::Display for Overflow {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("arithmetic overflow or division by zero in an expression")
                }
            }

            impl std::error::Error for Overflow {}
        }
    }
}
