//! The identity provider's side of a SAML sign-in, for the backend tests.
//!
//! A SAML provider is never called by this service: the browser carries the authentication request
//! to it and its response back. So the mock is not a server. It reads the authentication request
//! the way a provider does (HTTP-Redirect binding: base64, raw DEFLATE, XML) and makes the response
//! the way a provider does, and signs the assertion with a test key through the same XML-DSig
//! crate the service verifies with. The certificates and keys are the test ones the browser tests
//! use too (`tests/e2e/saml`); none of them signs anything outside the tests.
//!
//! Every field of what the provider says can be made wrong, one at a time, for the refusals.

#![allow(dead_code)]

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bergshamra_dsig::sign::sign;
use bergshamra_dsig::DsigContext;
use bergshamra_keys::loader::load_rsa_private_pem;
use bergshamra_keys::KeysManager;
use std::io::Read;
use std::sync::atomic::{AtomicU32, Ordering};

/// The certificate and key of the provider the tenant configured.
pub const IDP_CERT: &str = include_str!("../../../../tests/e2e/saml/idp-cert.pem");
pub const IDP_KEY: &str = include_str!("../../../../tests/e2e/saml/idp-key.pem");
/// Another party's certificate and key: what a forger signs with.
pub const OTHER_CERT: &str = include_str!("../../../../tests/e2e/saml/other-cert.pem");
pub const OTHER_KEY: &str = include_str!("../../../../tests/e2e/saml/other-key.pem");

pub const IDP_ENTITY: &str = "https://idp.acme.example/metadata";
pub const SSO_URL: &str = "https://idp.acme.example/sso";

pub const SAML_NS: &str = "urn:oasis:names:tc:SAML:2.0:assertion";
pub const SAMLP_NS: &str = "urn:oasis:names:tc:SAML:2.0:protocol";
pub const SUCCESS: &str = "urn:oasis:names:tc:SAML:2.0:status:Success";

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Seconds since the epoch, now.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// A time as SAML writes it.
pub fn instant(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
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

/// What the browser carried to the provider.
#[derive(Clone, Debug)]
pub struct Request {
    /// The XML of the authentication request.
    pub xml: String,
    pub id: String,
    pub issuer: String,
    pub destination: String,
    pub acs_url: String,
    pub protocol_binding: String,
    pub relay_state: String,
}

/// Read the sign-on address the service sent the browser to, as a provider reads it.
pub fn read_request(redirect: &str) -> Request {
    let url = reqwest::Url::parse(redirect).expect("the redirect is an address");
    assert_eq!(
        format!(
            "{}://{}{}",
            url.scheme(),
            url.host_str().unwrap(),
            url.path()
        ),
        SSO_URL,
        "the browser is sent to the provider's sign-on address"
    );
    let find = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    let message = find("SAMLRequest").expect("the address carries the request");
    let compressed = STANDARD.decode(message).expect("base64");
    let mut xml = String::new();
    flate2::read::DeflateDecoder::new(compressed.as_slice())
        .read_to_string(&mut xml)
        .expect("raw DEFLATE");
    let doc = uppsala::parse(&xml).expect("the request is XML");
    let root = doc.document_element().unwrap();
    let element = doc.element(root).unwrap();
    assert!(
        element.matches_name_ns(SAMLP_NS, "AuthnRequest"),
        "an authentication request"
    );
    let attribute = |name: &str| {
        doc.get_attribute(root, name)
            .unwrap_or_default()
            .to_string()
    };
    let issuer = doc
        .get_elements_by_tag_name_ns(SAML_NS, "Issuer")
        .first()
        .map(|id| doc.text_content_deep(*id))
        .unwrap_or_default();
    Request {
        id: attribute("ID"),
        issuer,
        destination: attribute("Destination"),
        acs_url: attribute("AssertionConsumerServiceURL"),
        protocol_binding: attribute("ProtocolBinding"),
        relay_state: find("RelayState").expect("the address carries the relay state"),
        xml,
    }
}

/// A signature template for the element with this identifier: the one the service's own checks
/// are made against (enveloped, exclusive canonicalisation, RSA with SHA-256).
fn signature_template(reference: &str) -> String {
    format!(
        "<ds:Signature xmlns:ds=\"http://www.w3.org/2000/09/xmldsig#\"><ds:SignedInfo>\
         <ds:CanonicalizationMethod Algorithm=\"http://www.w3.org/2001/10/xml-exc-c14n#\"/>\
         <ds:SignatureMethod Algorithm=\"http://www.w3.org/2001/04/xmldsig-more#rsa-sha256\"/>\
         <ds:Reference URI=\"#{reference}\"><ds:Transforms>\
         <ds:Transform Algorithm=\"http://www.w3.org/2000/09/xmldsig#enveloped-signature\"/>\
         <ds:Transform Algorithm=\"http://www.w3.org/2001/10/xml-exc-c14n#\"/></ds:Transforms>\
         <ds:DigestMethod Algorithm=\"http://www.w3.org/2001/04/xmlenc#sha256\"/>\
         <ds:DigestValue></ds:DigestValue></ds:Reference></ds:SignedInfo>\
         <ds:SignatureValue></ds:SignatureValue></ds:Signature>"
    )
}

/// Sign the first signature template of a document with a private key.
pub fn sign_with(key_pem: &str, template: &str) -> String {
    let mut keys = KeysManager::new();
    keys.add_key(load_rsa_private_pem(key_pem.as_bytes()).expect("a test key"));
    sign(&DsigContext::new_permissive(keys), template).expect("the template is signed")
}

/// What the provider says, every part of it open to being wrong.
#[derive(Clone, Debug)]
pub struct Reply {
    /// The request the response answers: `InResponseTo`, on the response and in the assertion.
    pub request_id: String,
    /// The address the response is for: `Destination`, and the `Recipient` of the confirmation.
    pub acs_url: String,
    pub destination: Option<String>,
    pub recipient: String,
    pub response_in_response_to: Option<String>,
    pub confirmation_in_response_to: String,
    /// The audience: this service's entity identifier.
    pub audience: String,
    pub response_issuer: Option<String>,
    pub issuer: String,
    pub response_id: String,
    pub assertion_id: String,
    /// The text of the name identifier, as XML: it may hold a comment.
    pub name_id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub status: String,
    pub issued: i64,
    pub not_before: Option<i64>,
    pub not_on_or_after: Option<i64>,
    pub confirmation_not_on_or_after: Option<i64>,
}

impl Reply {
    /// A correct reply to a request, for this service.
    pub fn to(request: &Request, audience: &str) -> Reply {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let now = now();
        Reply {
            request_id: request.id.clone(),
            acs_url: request.acs_url.clone(),
            destination: Some(request.acs_url.clone()),
            recipient: request.acs_url.clone(),
            response_in_response_to: Some(request.id.clone()),
            confirmation_in_response_to: request.id.clone(),
            audience: audience.to_string(),
            response_issuer: Some(IDP_ENTITY.into()),
            issuer: IDP_ENTITY.into(),
            response_id: format!("_response-{now}-{n}"),
            assertion_id: format!("_assertion-{now}-{n}"),
            name_id: "subject-1".into(),
            email: Some("pat@bound.example".into()),
            name: Some("Pat Person".into()),
            status: SUCCESS.into(),
            issued: now,
            not_before: Some(now - 5),
            not_on_or_after: Some(now + 300),
            confirmation_not_on_or_after: Some(now + 300),
        }
    }

    /// The assertion, with a signature template after the issuer when `signed` is set.
    pub fn assertion(&self, signed: bool) -> String {
        let attribute = |name: &str, value: &Option<String>| {
            value.as_ref().map_or(String::new(), |value| {
                format!(
                    "<saml:Attribute Name=\"{name}\"><saml:AttributeValue>{value}</saml:AttributeValue>\
                     </saml:Attribute>"
                )
            })
        };
        let time = |name: &str, value: Option<i64>| {
            value.map_or(String::new(), |value| {
                format!(" {name}=\"{}\"", instant(value))
            })
        };
        format!(
            "<saml:Assertion xmlns:saml=\"{SAML_NS}\" ID=\"{id}\" Version=\"2.0\" \
             IssueInstant=\"{issued}\"><saml:Issuer>{issuer}</saml:Issuer>{signature}\
             <saml:Subject><saml:NameID \
             Format=\"urn:oasis:names:tc:SAML:2.0:nameid-format:persistent\">{name_id}</saml:NameID>\
             <saml:SubjectConfirmation Method=\"urn:oasis:names:tc:SAML:2.0:cm:bearer\">\
             <saml:SubjectConfirmationData InResponseTo=\"{in_response_to}\" \
             Recipient=\"{recipient}\"{confirmation_end}/></saml:SubjectConfirmation></saml:Subject>\
             <saml:Conditions{not_before}{not_on_or_after}><saml:AudienceRestriction>\
             <saml:Audience>{audience}</saml:Audience></saml:AudienceRestriction></saml:Conditions>\
             <saml:AuthnStatement AuthnInstant=\"{issued}\"><saml:AuthnContext>\
             <saml:AuthnContextClassRef>urn:oasis:names:tc:SAML:2.0:ac:classes:PasswordProtectedTransport\
             </saml:AuthnContextClassRef></saml:AuthnContext></saml:AuthnStatement>\
             <saml:AttributeStatement>{email}{name}</saml:AttributeStatement></saml:Assertion>",
            id = self.assertion_id,
            issued = instant(self.issued),
            issuer = self.issuer,
            signature = if signed {
                signature_template(&self.assertion_id)
            } else {
                String::new()
            },
            name_id = self.name_id,
            in_response_to = self.confirmation_in_response_to,
            recipient = self.recipient,
            confirmation_end = time("NotOnOrAfter", self.confirmation_not_on_or_after),
            not_before = time("NotBefore", self.not_before),
            not_on_or_after = time("NotOnOrAfter", self.not_on_or_after),
            audience = self.audience,
            email = attribute("email", &self.email),
            name = attribute("name", &self.name),
        )
    }

    /// The assertion, signed by the key. It is signed as a document of its own, so that what is
    /// signed does not depend on what it is put in: its namespaces are its own.
    pub fn signed_assertion(&self, key_pem: &str) -> String {
        let signed = sign_with(key_pem, &self.assertion(true));
        if signed.starts_with("<?xml") {
            if let Some((_, body)) = signed.split_once("?>") {
                return body.trim_start().to_string();
            }
        }
        signed
    }

    /// A response around whatever is given as its content after the status, with a signature
    /// template when `signature` is set.
    pub fn response_around(&self, content: &str, signature: bool) -> String {
        let attribute = |name: &str, value: &Option<String>| {
            value
                .as_ref()
                .map_or(String::new(), |value| format!(" {name}=\"{value}\""))
        };
        format!(
            "<samlp:Response xmlns:samlp=\"{SAMLP_NS}\" xmlns:saml=\"{SAML_NS}\" ID=\"{id}\" \
             Version=\"2.0\" IssueInstant=\"{issued}\"{destination}{in_response_to}>{issuer}\
             {signature}<samlp:Status><samlp:StatusCode Value=\"{status}\"/></samlp:Status>\
             {content}</samlp:Response>",
            id = self.response_id,
            issued = instant(self.issued),
            destination = attribute("Destination", &self.destination),
            in_response_to = attribute("InResponseTo", &self.response_in_response_to),
            issuer = self
                .response_issuer
                .as_ref()
                .map_or(String::new(), |issuer| format!(
                    "<saml:Issuer>{issuer}</saml:Issuer>"
                )),
            signature = if signature {
                signature_template(&self.response_id)
            } else {
                String::new()
            },
            status = self.status,
        )
    }

    /// The usual response: the assertion signed by the provider's key.
    pub fn response(&self) -> String {
        self.response_around(&self.signed_assertion(IDP_KEY), false)
    }

    /// The same, signed by another key.
    pub fn response_signed_by(&self, key_pem: &str) -> String {
        self.response_around(&self.signed_assertion(key_pem), false)
    }

    /// A response whose assertion has no signature at all.
    pub fn response_unsigned(&self) -> String {
        self.response_around(&self.assertion(false), false)
    }

    /// A response that is signed, around an assertion that is not.
    pub fn response_signed_around_unsigned_assertion(&self) -> String {
        sign_with(IDP_KEY, &self.response_around(&self.assertion(false), true))
    }
}

/// What the browser posts: the response as base64.
pub fn encode(xml: &str) -> String {
    STANDARD.encode(xml.as_bytes())
}

/// The signature element of a signed document, as text.
pub fn signature_of(signed: &str) -> String {
    let start = signed.find("<ds:Signature").expect("a signature");
    let end = signed.find("</ds:Signature>").expect("a signature end") + "</ds:Signature>".len();
    signed[start..end].to_string()
}
