//! Checking a SAML response.
//!
//! The XML signature itself is checked by the `bergshamra-dsig` crate, a pure-Rust XML-DSig
//! implementation; nothing here hand-rolls canonicalisation, digests or signature algorithms. This
//! module decides what it is asked to check and what is checked beyond it:
//!
//! - **The message.** It is read with document type declarations, and so external entities, turned
//!   off (FED-251), at a bounded size.
//! - **One assertion, the one that is signed.** The response holds exactly one assertion, a direct
//!   child of the response, and it carries its own signature. The signature is checked with the
//!   provider's configured certificates only, never a key the message brings, and its single
//!   reference must resolve to that very element, by its identifier. A signed response around an
//!   unsigned assertion, a second assertion, an assertion moved elsewhere, and a signature over
//!   another element are all refused (FED-005, FED-252). Everything below is read from that
//!   element and nowhere else.
//! - **The subject** is the complete text of the name identifier, comments excluded (FED-253).
//! - **The claims.** Issuer is the configured one (FED-254); the audience is this service's entity
//!   identifier and the recipient and the destination are its assertion consumer address
//!   (FED-008); the assertion answers the request that was made (FED-007); and it is inside its
//!   validity with a small skew.
//!
//! Replay is the caller's: it keeps the identifier of an accepted assertion until it would no
//! longer be accepted anyway (FED-006).
//!
//! Implements: FED-005, FED-006, FED-007, FED-008, FED-251, FED-252, FED-253, FED-254, H-016

use super::config::SamlProvider;
use crate::signin::federated::Asserted;
use bergshamra_dsig::verify::verify_all_document_with_source;
use bergshamra_dsig::{DsigContext, VerifyResult};
use bergshamra_keys::loader::load_x509_cert_pem;
use bergshamra_keys::KeysManager;
use uppsala::{Document, NodeId, Parser};

pub const SAML: &str = "urn:oasis:names:tc:SAML:2.0:assertion";
pub const SAMLP: &str = "urn:oasis:names:tc:SAML:2.0:protocol";
pub const DSIG: &str = "http://www.w3.org/2000/09/xmldsig#";
const BEARER: &str = "urn:oasis:names:tc:SAML:2.0:cm:bearer";
const SUCCESS: &str = "urn:oasis:names:tc:SAML:2.0:status:Success";

/// The largest response that is read.
pub const MAX_XML_BYTES: usize = 256 * 1024;
/// The longest subject identifier.
const MAX_SUBJECT_BYTES: usize = 255;
/// The longest email address (RFC 5321).
const MAX_EMAIL_BYTES: usize = 254;

/// Why a response was refused. It is for the log, never for the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Malformed,
    Encrypted,
    NoAssertion,
    ManyAssertions,
    Position,
    Unsigned,
    Signature,
    Status,
    Issuer,
    Audience,
    Recipient,
    Destination,
    InResponseTo,
    Expired,
    NotYetValid,
    Subject,
    Conditions,
    Configuration,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Refusal::Malformed => "saml_malformed",
            Refusal::Encrypted => "saml_encrypted_assertion",
            Refusal::NoAssertion => "saml_no_assertion",
            Refusal::ManyAssertions => "saml_many_assertions",
            Refusal::Position => "saml_assertion_position",
            Refusal::Unsigned => "saml_unsigned",
            Refusal::Signature => "saml_signature",
            Refusal::Status => "saml_status",
            Refusal::Issuer => "saml_issuer",
            Refusal::Audience => "saml_audience",
            Refusal::Recipient => "saml_recipient",
            Refusal::Destination => "saml_destination",
            Refusal::InResponseTo => "saml_in_response_to",
            Refusal::Expired => "saml_expired",
            Refusal::NotYetValid => "saml_not_yet_valid",
            Refusal::Subject => "saml_subject",
            Refusal::Conditions => "saml_conditions",
            Refusal::Configuration => "saml_configuration",
        }
    }
}

/// What the response must be for.
pub struct Expect<'a> {
    pub provider: &'a SamlProvider,
    /// This service's entity identifier: the audience the assertion must name.
    pub entity_id: &'a str,
    /// This service's assertion consumer address: the recipient and the destination.
    pub acs_url: &'a str,
    /// The identifier of the authentication request the response must answer.
    pub request_id: &'a str,
    /// Now, in seconds since the epoch.
    pub now: i64,
}

/// An assertion that passed every check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    /// The assertion's identifier, which replay protection remembers.
    pub assertion_id: String,
    /// The last second, since the epoch, at which this assertion could still be accepted.
    pub accepted_until: i64,
    pub who: Asserted,
}

/// Days from 1970-01-01 to a civil date (proleptic Gregorian).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        _ => 28,
    }
}

/// A SAML time: `YYYY-MM-DDThh:mm:ss`, an optional fraction and `Z`. SAML requires UTC, so any
/// other zone is not a time. The fraction is dropped.
pub fn instant(text: &str) -> Option<i64> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if !text.is_ascii() || bytes.len() < 20 || *bytes.last()? != b'Z' {
        return None;
    }
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let number = |from: usize, to: usize| -> Option<i64> {
        let digits = text.get(from..to)?;
        if digits.bytes().all(|b| b.is_ascii_digit()) {
            digits.parse().ok()
        } else {
            None
        }
    };
    let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let (hour, minute, second) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
    let fraction = &text[19..text.len() - 1];
    let fraction_ok = fraction.is_empty()
        || (fraction.len() > 1
            && fraction.starts_with('.')
            && fraction[1..].bytes().all(|b| b.is_ascii_digit()));
    if !fraction_ok
        || !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// The time a SAML message carries as an attribute, when it has the attribute at all. `Err` is an
/// attribute that is not a time.
fn time_attr(doc: &Document<'_>, element: NodeId, name: &str) -> Result<Option<i64>, Refusal> {
    match plain_attr(doc, element, name) {
        None => Ok(None),
        Some(text) => instant(text).map(Some).ok_or(Refusal::Malformed),
    }
}

/// The value of an attribute that has no namespace. An attribute spelled twice, whether in one
/// namespace or two, is none at all: the one this module reads must be the one the signature
/// covers.
fn plain_attr<'d>(doc: &'d Document<'_>, element: NodeId, name: &str) -> Option<&'d str> {
    let mut named = doc
        .element(element)?
        .attributes
        .iter()
        .filter(|attribute| *attribute.name.local_name == *name);
    let first = named.next()?;
    if named.next().is_some() || first.name.namespace_uri.is_some() {
        return None;
    }
    Some(&*first.value)
}

fn is_element(doc: &Document<'_>, id: NodeId, namespace: &str, local: &str) -> bool {
    doc.element(id)
        .is_some_and(|element| element.matches_name_ns(namespace, local))
}

/// The one child element of a name: `None` when there is none, an error when there are several.
fn only_child(
    doc: &Document<'_>,
    parent: NodeId,
    namespace: &str,
    local: &str,
) -> Result<Option<NodeId>, Refusal> {
    let found = doc.child_elements_by_name_ns(parent, namespace, local);
    match found.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(*one)),
        _ => Err(Refusal::Malformed),
    }
}

/// The text of an element that holds text only: every text and CDATA part, in order, with
/// comments and processing instructions left out. An element inside it is not text.
fn text_of(doc: &Document<'_>, element: NodeId) -> Result<String, Refusal> {
    if doc
        .children_iter(element)
        .any(|child| doc.element(child).is_some())
    {
        return Err(Refusal::Malformed);
    }
    Ok(doc.text_content_deep(element))
}

fn within(time: i64, now: i64, skew: i64) -> Result<(), Refusal> {
    if time > now + skew {
        Err(Refusal::NotYetValid)
    } else {
        Ok(())
    }
}

/// Check that the signature of the assertion is the provider's and covers the assertion.
///
/// Implements: FED-005, FED-252
fn check_signature(
    doc: &Document<'_>,
    xml: &str,
    provider: &SamlProvider,
    assertion: NodeId,
    assertion_id: &str,
) -> Result<(), Refusal> {
    let own = doc.child_elements_by_name_ns(assertion, DSIG, "Signature");
    match own.as_slice() {
        [] => return Err(Refusal::Unsigned),
        [_] => {}
        _ => return Err(Refusal::Signature),
    }
    let mut keys = KeysManager::new();
    for pem in &provider.certificates {
        keys.add_key(load_x509_cert_pem(pem.as_bytes()).map_err(|_| Refusal::Configuration)?);
    }
    if keys.is_empty() {
        return Err(Refusal::Configuration);
    }
    // The secure defaults: only the configured keys, never one the message brings in its key
    // information; references only to the document element, an ancestor of the signature or a
    // sibling of it; a digest computed here for every reference; duplicate identifiers refused.
    let context = DsigContext::new(keys);
    let results = verify_all_document_with_source(&context, doc, Some(xml))
        .map_err(|_| Refusal::Signature)?;
    let expected = format!("#{assertion_id}");
    let mut covered = false;
    for result in &results {
        match result {
            // A signature of the message that does not verify makes the whole message suspect,
            // whichever element it was meant for.
            VerifyResult::Invalid { .. } => return Err(Refusal::Signature),
            VerifyResult::Valid {
                signature_node,
                references,
                ..
            } => {
                if doc.parent(*signature_node) == Some(assertion) {
                    let [reference] = references.as_slice() else {
                        return Err(Refusal::Signature);
                    };
                    if reference.uri == expected
                        && reference.resolved_node == Some(assertion)
                        && reference.digest_verified
                    {
                        covered = true;
                    }
                }
            }
        }
    }
    if covered {
        Ok(())
    } else {
        Err(Refusal::Signature)
    }
}

/// The audience restrictions, each of which must name this service.
///
/// Implements: FED-008
fn check_conditions(
    doc: &Document<'_>,
    conditions: NodeId,
    expect: &Expect<'_>,
) -> Result<i64, Refusal> {
    let skew = expect.provider.skew_secs;
    if let Some(not_before) = time_attr(doc, conditions, "NotBefore")? {
        within(not_before, expect.now, skew)?;
    }
    let not_on_or_after = time_attr(doc, conditions, "NotOnOrAfter")?.ok_or(Refusal::Conditions)?;
    if expect.now - skew >= not_on_or_after {
        return Err(Refusal::Expired);
    }
    // A condition this service does not understand is a condition it cannot say is met.
    for child in doc.children_iter(conditions) {
        if doc.element(child).is_some()
            && !["AudienceRestriction", "OneTimeUse", "ProxyRestriction"]
                .iter()
                .any(|name| is_element(doc, child, SAML, name))
        {
            return Err(Refusal::Conditions);
        }
    }
    let restrictions = doc.child_elements_by_name_ns(conditions, SAML, "AudienceRestriction");
    if restrictions.is_empty() {
        return Err(Refusal::Audience);
    }
    for restriction in restrictions {
        let named = doc
            .child_elements_by_name_ns(restriction, SAML, "Audience")
            .into_iter()
            .any(|audience| {
                text_of(doc, audience).is_ok_and(|text| text.trim() == expect.entity_id)
            });
        if !named {
            return Err(Refusal::Audience);
        }
    }
    Ok(not_on_or_after)
}

/// The subject of the assertion and the end of the confirmation that this service may rely on: a
/// bearer confirmation for this address, answering this request.
///
/// Implements: FED-007, FED-008, FED-253
fn check_subject(
    doc: &Document<'_>,
    assertion: NodeId,
    expect: &Expect<'_>,
) -> Result<(String, i64), Refusal> {
    let skew = expect.provider.skew_secs;
    let subject = only_child(doc, assertion, SAML, "Subject")?.ok_or(Refusal::Subject)?;
    let name_id = only_child(doc, subject, SAML, "NameID")?.ok_or(Refusal::Subject)?;
    let identifier = text_of(doc, name_id).map_err(|_| Refusal::Subject)?;
    if identifier.trim().is_empty()
        || identifier.len() > MAX_SUBJECT_BYTES
        || identifier.chars().any(char::is_control)
    {
        return Err(Refusal::Subject);
    }

    // The first reason a confirmation was set aside is the one that is kept, for the log.
    let mut reason = Refusal::Subject;
    for confirmation in doc.child_elements_by_name_ns(subject, SAML, "SubjectConfirmation") {
        if plain_attr(doc, confirmation, "Method") != Some(BEARER) {
            continue;
        }
        let Some(data) = only_child(doc, confirmation, SAML, "SubjectConfirmationData")? else {
            continue;
        };
        if plain_attr(doc, data, "Recipient") != Some(expect.acs_url) {
            if reason == Refusal::Subject {
                reason = Refusal::Recipient;
            }
            continue;
        }
        if plain_attr(doc, data, "InResponseTo") != Some(expect.request_id) {
            if reason == Refusal::Subject {
                reason = Refusal::InResponseTo;
            }
            continue;
        }
        let Some(not_on_or_after) = time_attr(doc, data, "NotOnOrAfter")? else {
            continue;
        };
        if expect.now - skew >= not_on_or_after {
            if reason == Refusal::Subject {
                reason = Refusal::Expired;
            }
            continue;
        }
        if let Some(not_before) = time_attr(doc, data, "NotBefore")? {
            within(not_before, expect.now, skew)?;
        }
        return Ok((identifier, not_on_or_after));
    }
    Err(reason)
}

/// The first value of the attribute of this name.
fn attribute(doc: &Document<'_>, assertion: NodeId, name: &str) -> Option<String> {
    for statement in doc.child_elements_by_name_ns(assertion, SAML, "AttributeStatement") {
        for found in doc.child_elements_by_name_ns(statement, SAML, "Attribute") {
            if plain_attr(doc, found, "Name") != Some(name) {
                continue;
            }
            let value = doc
                .child_elements_by_name_ns(found, SAML, "AttributeValue")
                .into_iter()
                .find_map(|value| text_of(doc, value).ok())?;
            let value = value.trim();
            return (!value.is_empty()).then(|| value.to_string());
        }
    }
    None
}

/// Check a response, given as the XML text the provider's message decoded to.
///
/// Implements: FED-005, FED-007, FED-008, FED-251, FED-252, FED-253, FED-254
pub fn verify(xml: &str, expect: &Expect<'_>) -> Result<Verified, Refusal> {
    if xml.len() > MAX_XML_BYTES {
        return Err(Refusal::Malformed);
    }
    let skew = expect.provider.skew_secs;
    let doc = Parser::new()
        .with_forbid_dtd(true)
        .parse(xml)
        .map_err(|_| Refusal::Malformed)?;
    let response = doc.document_element().ok_or(Refusal::Malformed)?;
    if !is_element(&doc, response, SAMLP, "Response") {
        return Err(Refusal::Malformed);
    }
    if !doc
        .get_elements_by_tag_name_ns(SAML, "EncryptedAssertion")
        .is_empty()
    {
        return Err(Refusal::Encrypted);
    }

    // What the response says of itself is not signed unless the provider signed it, so none of
    // it is relied on; it can only make this service refuse more.
    if plain_attr(&doc, response, "Destination").is_some_and(|d| d != expect.acs_url) {
        return Err(Refusal::Destination);
    }
    if plain_attr(&doc, response, "InResponseTo").is_some_and(|id| id != expect.request_id) {
        return Err(Refusal::InResponseTo);
    }
    if let Some(issuer) = only_child(&doc, response, SAML, "Issuer")? {
        if text_of(&doc, issuer)?.trim() != expect.provider.idp_entity_id {
            return Err(Refusal::Issuer);
        }
    }
    let status = only_child(&doc, response, SAMLP, "Status")?.ok_or(Refusal::Status)?;
    let code = only_child(&doc, status, SAMLP, "StatusCode")?.ok_or(Refusal::Status)?;
    if plain_attr(&doc, code, "Value") != Some(SUCCESS) {
        return Err(Refusal::Status);
    }

    // One assertion, a child of the response, with a signature of its own over itself.
    let assertions = doc.get_elements_by_tag_name_ns(SAML, "Assertion");
    let assertion = match assertions.as_slice() {
        [] => return Err(Refusal::NoAssertion),
        [one] => *one,
        _ => return Err(Refusal::ManyAssertions),
    };
    if doc.parent(assertion) != Some(response) {
        return Err(Refusal::Position);
    }
    let assertion_id = plain_attr(&doc, assertion, "ID")
        .filter(|id| {
            id.len() <= 256
                && id
                    .bytes()
                    .next()
                    .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        })
        .ok_or(Refusal::Malformed)?
        .to_string();
    check_signature(&doc, xml, expect.provider, assertion, &assertion_id)?;

    // From here on, only the signed element is read.
    if plain_attr(&doc, assertion, "Version") != Some("2.0") {
        return Err(Refusal::Malformed);
    }
    let issued = time_attr(&doc, assertion, "IssueInstant")?.ok_or(Refusal::Malformed)?;
    within(issued, expect.now, skew)?;
    let issuer = only_child(&doc, assertion, SAML, "Issuer")?.ok_or(Refusal::Issuer)?;
    if text_of(&doc, issuer)?.trim() != expect.provider.idp_entity_id {
        return Err(Refusal::Issuer);
    }
    if doc
        .child_elements_by_name_ns(assertion, SAML, "AuthnStatement")
        .is_empty()
    {
        return Err(Refusal::Malformed);
    }
    let conditions = only_child(&doc, assertion, SAML, "Conditions")?.ok_or(Refusal::Conditions)?;
    let conditions_end = check_conditions(&doc, conditions, expect)?;
    let (subject, confirmation_end) = check_subject(&doc, assertion, expect)?;

    let email = attribute(&doc, assertion, &expect.provider.email_attribute)
        .filter(|email| email.len() <= MAX_EMAIL_BYTES && email.contains('@'));
    let name = attribute(&doc, assertion, &expect.provider.name_attribute);
    Ok(Verified {
        assertion_id,
        accepted_until: conditions_end.min(confirmation_end) + skew,
        who: Asserted {
            subject,
            email_verified: expect.provider.email_verified && email.is_some(),
            email,
            name,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saml_time_is_utc_and_exact() {
        assert_eq!(instant("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(instant("2026-11-19T10:30:15Z"), Some(1_795_084_215));
        assert_eq!(instant("2026-11-19T10:30:15.789Z"), Some(1_795_084_215));
        assert_eq!(instant("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        assert_eq!(instant("2026-02-29T00:00:00Z"), None, "not a leap year");
        assert_eq!(instant("2026-11-19T10:30:15+01:00"), None, "another zone");
        assert_eq!(instant("2026-11-19T10:30:15"), None, "no zone");
        assert_eq!(instant("2026-13-19T10:30:15Z"), None);
        assert_eq!(instant("2026-11-19T24:00:00Z"), None);
        assert_eq!(instant("2026-11-19T10:30:15.Z"), None);
        assert_eq!(instant("yesterday"), None);
        assert_eq!(instant("2026-11-19T10:30:15Z\u{e9}"), None);
    }
}
