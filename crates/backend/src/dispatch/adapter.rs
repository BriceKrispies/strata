//! The local mail adapter (EML-001, EML-002): a mail catcher that needs no cloud account, and the
//! contract the dispatcher expects of any adapter, stated by the rows of `cases-mail-conformance`.
//!
//! An adapter takes a rendered message with an idempotency key and answers one of: accepted, with
//! the reference of the provider; the same reference again for a key it accepted already, and no
//! second delivery (EML-006); a temporary refusal (EML-007); a permanent refusal with its reason
//! (EML-008); a refusal before anything is sent for a message above its size limit or a header
//! that holds a line break (EML-1566); or a configuration failure for a credential it rejects,
//! which marks no message failed. It also reads a delivery report of its provider: a report whose
//! signature does not verify is refused (EML-019), and one that verifies becomes a
//! [`DeliveryReport`] naming the message, for [`super::reports::apply`].
//!
//! [`LocalAdapter`] keeps what it accepts in memory (it is the development catcher with the rules
//! of a provider) and signs its reports with HMAC-SHA256 under a key it is given. It implements
//! [`Mailer`], so the dispatcher's own path (the worker, the retry policy, the mail log) can run
//! against it: the idempotency key of a message is the provider reference the dispatcher put in
//! the header `X-Strata-Message`.

use super::render::header_problem;
use super::reports::{DeliveryReport, ReportKind};
use crate::mail::{Mail, MailError, Mailer, CONFIGURATION, PERMANENT};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Mutex;
use strata_kernel::Timestamp;

/// The most bytes of one message the local adapter accepts.
pub const DEFAULT_LIMIT_BYTES: usize = 262_144;

/// What an adapter answers for a message it takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accepted {
    /// The reference of the provider for the message.
    pub reference: String,
    /// Whether the key was accepted before: the reference is the first one and nothing was
    /// delivered again.
    pub repeated: bool,
}

/// Why an adapter did not take a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The provider will take the message later, or did not answer.
    Temporary(String),
    /// The provider refuses the message for good.
    Permanent(String),
    /// The message is above the size limit of the adapter; nothing was sent.
    Oversized,
    /// A subject, an address or a header holds a line break; nothing was sent.
    HeaderBreak(String),
    /// The provider rejects the credential: the deployment is misconfigured, not the message.
    Configuration(String),
}

/// Why a delivery report was not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReportRefusal {
    /// The signature is not the provider's.
    BadSignature,
    /// The body is not a report.
    Malformed(String),
}

#[derive(Debug, Default)]
struct Inner {
    /// Idempotency key to the reference of the first acceptance.
    accepted: BTreeMap<String, String>,
    caught: Vec<Mail>,
    /// How many of the next submissions the provider refuses for a temporary reason, and whether
    /// each is taken (the answer was lost) or not (nothing arrived).
    refuse_next: u32,
    answer_lost: bool,
    /// Addresses the provider rejects for good.
    rejected: Vec<String>,
    next: u64,
}

/// The mail catcher: no network, no account.
#[derive(Debug)]
pub struct LocalAdapter {
    /// The credential the provider accepts.
    credential: String,
    /// The credential the dispatcher presents, read from configuration at the time of sending
    /// (EML-004).
    presented: Mutex<String>,
    limit: usize,
    signing_key: Vec<u8>,
    inner: Mutex<Inner>,
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let inner_pad: Vec<u8> = block.iter().map(|b| b ^ 0x36).collect();
    let outer_pad: Vec<u8> = block.iter().map(|b| b ^ 0x5c).collect();
    let mut inner = Sha256::new();
    inner.update(&inner_pad);
    inner.update(data);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(&outer_pad);
    outer.update(inner);
    outer.finalize().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether two texts are equal, taking the same time whatever the first difference.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

impl LocalAdapter {
    /// An adapter that accepts `credential` and signs its reports under `signing_key`.
    pub fn new(credential: &str, signing_key: &[u8]) -> Self {
        LocalAdapter {
            credential: credential.to_string(),
            presented: Mutex::new(credential.to_string()),
            limit: DEFAULT_LIMIT_BYTES,
            signing_key: signing_key.to_vec(),
            inner: Mutex::new(Inner::default()),
        }
    }

    /// The same adapter with another size limit.
    pub fn with_limit(mut self, bytes: usize) -> Self {
        self.limit = bytes;
        self
    }

    /// Present another credential from now on, as a rotated or wrong one in the configuration.
    pub fn present(&self, credential: &str) {
        if let Ok(mut presented) = self.presented.lock() {
            *presented = credential.to_string();
        }
    }

    /// Refuse the next `count` submissions for a temporary reason. With `answer_lost` the
    /// provider takes each message and only its answer is lost; without it nothing arrives.
    pub fn refuse_next(&self, count: u32, answer_lost: bool) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.refuse_next = count;
            inner.answer_lost = answer_lost;
        }
    }

    /// Reject an address for good from now on.
    pub fn reject_address(&self, address: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.rejected.push(address.to_string());
        }
    }

    /// The messages the adapter holds, oldest first.
    pub fn caught(&self) -> Vec<Mail> {
        self.inner
            .lock()
            .map(|inner| inner.caught.clone())
            .unwrap_or_default()
    }

    /// Take a message under an idempotency key.
    pub fn submit(&self, key: &str, mail: &Mail) -> Result<Accepted, Refusal> {
        let presented = self
            .presented
            .lock()
            .map(|p| p.clone())
            .map_err(|_| Refusal::Temporary("the adapter is poisoned".into()))?;
        if !same(&presented, &self.credential) {
            return Err(Refusal::Configuration(
                "the provider rejects the credential".into(),
            ));
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| Refusal::Temporary("the adapter is poisoned".into()))?;
        if let Some(first) = inner.accepted.get(key) {
            return Ok(Accepted {
                reference: first.clone(),
                repeated: true,
            });
        }
        let named = [("the recipient", &mail.to), ("the subject", &mail.subject)];
        for (what, value) in named {
            if let Some(problem) = header_problem(value) {
                return Err(Refusal::HeaderBreak(format!("{what} {problem}")));
            }
        }
        for (name, value) in &mail.headers {
            if header_problem(name).is_some() || header_problem(value).is_some() {
                return Err(Refusal::HeaderBreak(format!("the header {name}")));
            }
        }
        let size = mail.to.len()
            + mail.subject.len()
            + mail.text.len()
            + mail.html.as_ref().map_or(0, String::len)
            + mail
                .headers
                .iter()
                .map(|(name, value)| name.len() + value.len())
                .sum::<usize>();
        if size > self.limit {
            return Err(Refusal::Oversized);
        }
        if inner.rejected.iter().any(|address| *address == mail.to) {
            return Err(Refusal::Permanent("the mailbox does not exist".into()));
        }
        let lost = inner.refuse_next > 0 && inner.answer_lost;
        if inner.refuse_next > 0 {
            inner.refuse_next -= 1;
            if !lost {
                return Err(Refusal::Temporary(
                    "the provider will take the message later".into(),
                ));
            }
        }
        inner.next += 1;
        let reference = format!("local-{}", inner.next);
        inner.accepted.insert(key.to_string(), reference.clone());
        inner.caught.push(mail.clone());
        if lost {
            return Err(Refusal::Temporary("no answer before the deadline".into()));
        }
        Ok(Accepted {
            reference,
            repeated: false,
        })
    }

    /// A delivery report as the provider sends it: its body and its signature (hexadecimal
    /// HMAC-SHA256 of the body). `message` is the idempotency key the message was submitted
    /// with, `kind` is `delivered`, `bounce`, `soft-bounce` or `complaint`.
    pub fn signed_report(
        &self,
        event: &str,
        message: &str,
        kind: &str,
        at: i64,
    ) -> (Vec<u8>, String) {
        let body = json!({ "event": event, "message": message, "kind": kind, "at": at })
            .to_string()
            .into_bytes();
        let signature = hex(&hmac_sha256(&self.signing_key, &body));
        (body, signature)
    }

    /// Read a report of the provider: refused when the signature is not the provider's
    /// (EML-019), otherwise the event it carries, naming the message by the reference the
    /// dispatcher submitted it with.
    pub fn read_report(
        &self,
        body: &[u8],
        signature: &str,
    ) -> Result<DeliveryReport, ReportRefusal> {
        let expected = hex(&hmac_sha256(&self.signing_key, body));
        if !same(&expected, signature) {
            return Err(ReportRefusal::BadSignature);
        }
        let value: serde_json::Value =
            serde_json::from_slice(body).map_err(|e| ReportRefusal::Malformed(e.to_string()))?;
        let text = |name: &str| {
            value
                .get(name)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| ReportRefusal::Malformed(format!("no {name}")))
        };
        let kind = match text("kind")?.as_str() {
            "delivered" => ReportKind::Delivered,
            "bounce" => ReportKind::Bounce { permanent: true },
            "soft-bounce" => ReportKind::Bounce { permanent: false },
            "complaint" => ReportKind::Complaint,
            other => return Err(ReportRefusal::Malformed(format!("kind {other}"))),
        };
        let at = value
            .get("at")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| ReportRefusal::Malformed("no at".into()))?;
        Ok(DeliveryReport {
            event: text("event")?,
            reference: text("message")?,
            kind,
            at: Timestamp(at),
        })
    }
}

impl Mailer for LocalAdapter {
    fn send(&self, mail: &Mail) -> Result<(), MailError> {
        let key = mail
            .headers
            .iter()
            .find(|(name, _)| name == "X-Strata-Message")
            .map(|(_, value)| value.clone())
            .ok_or_else(|| MailError(format!("{PERMANENT}the message has no idempotency key")))?;
        match self.submit(&key, mail) {
            Ok(_) => Ok(()),
            Err(Refusal::Temporary(why)) => Err(MailError(why)),
            Err(Refusal::Permanent(why)) => Err(MailError(format!("{PERMANENT}{why}"))),
            Err(Refusal::Oversized) => Err(MailError(format!(
                "{PERMANENT}the message is above the size limit"
            ))),
            Err(Refusal::HeaderBreak(why)) => Err(MailError(format!("{PERMANENT}{why}"))),
            Err(Refusal::Configuration(why)) => Err(MailError(format!("{CONFIGURATION}{why}"))),
        }
    }

    fn outbox(&self) -> Vec<Mail> {
        self.caught()
    }
}
