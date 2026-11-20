//! Outgoing mail, behind one seam.
//!
//! The backend writes a message and hands it to a [`Mailer`]. There are two: [`Smtp`], which
//! relays through a mail server and is used only when the operator configures one, and
//! [`DevSink`], which keeps the message in memory (and, when told where, as an `.eml` file) and
//! sends nothing. The sink is the default, so that no development or test run can reach a real
//! address.
//!
//! Implements: H-034, IDN-006

use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Mutex;

/// A message to one address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    pub text: String,
}

/// Why a message was not delivered to the transport.
#[derive(Debug)]
pub struct MailError(pub String);

/// Something that delivers a message.
pub trait Mailer: Send + Sync + Debug {
    /// Hand the message over. A transport that talks to a server does so off the caller's
    /// thread and logs a delivery that fails, so that how long this takes and whether it fails
    /// say nothing about the address.
    fn send(&self, mail: &Mail) -> Result<(), MailError>;

    /// The messages a development sink holds, oldest first. A transport that sends mail holds
    /// none.
    fn outbox(&self) -> Vec<Mail> {
        Vec::new()
    }
}

/// The development sink: messages are kept, never sent.
#[derive(Debug, Default)]
pub struct DevSink {
    kept: Mutex<Vec<Mail>>,
    /// Where each message is also written as `<n>.eml`, when set.
    dir: Option<PathBuf>,
}

impl DevSink {
    /// A sink that keeps messages in memory only.
    pub fn memory() -> Self {
        DevSink::default()
    }

    /// A sink that also writes each message to a file in `dir`.
    pub fn files(dir: impl Into<PathBuf>) -> Self {
        DevSink {
            kept: Mutex::new(Vec::new()),
            dir: Some(dir.into()),
        }
    }
}

/// A message in the plain-text form of an `.eml` file.
fn eml(mail: &Mail) -> String {
    // A header value cannot hold a line break, whatever the address or subject held.
    let line = |text: &str| text.replace(['\r', '\n'], " ");
    format!(
        "To: {}\nSubject: {}\nContent-Type: text/plain; charset=utf-8\n\n{}\n",
        line(&mail.to),
        line(&mail.subject),
        mail.text
    )
}

impl Mailer for DevSink {
    fn send(&self, mail: &Mail) -> Result<(), MailError> {
        let mut kept = self
            .kept
            .lock()
            .map_err(|_| MailError("the outbox is poisoned".into()))?;
        // A development sink does not grow without bound.
        if kept.len() >= 200 {
            kept.remove(0);
        }
        kept.push(mail.clone());
        if let Some(dir) = &self.dir {
            std::fs::create_dir_all(dir).map_err(|e| MailError(e.to_string()))?;
            let at = strata_protocol::log::now_ms();
            std::fs::write(dir.join(format!("{at}-{}.eml", kept.len())), eml(mail))
                .map_err(|e| MailError(e.to_string()))?;
        }
        Ok(())
    }

    fn outbox(&self) -> Vec<Mail> {
        self.kept
            .lock()
            .map(|kept| kept.clone())
            .unwrap_or_default()
    }
}

/// Relay through an SMTP server, with the `lettre` crate.
pub struct Smtp {
    transport: lettre::SmtpTransport,
    from: lettre::message::Mailbox,
    /// A few workers and a bounded queue: a hung mail server holds the workers, not a thread per
    /// message, and a message that finds the queue full is refused.
    pool: crate::lab::Pool,
}

/// Messages sent at the same time, and messages waiting behind them.
const SMTP_WORKERS: usize = 4;
const SMTP_QUEUE: usize = 64;
/// How long one exchange with the mail server may take before it is given up.
const SMTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

impl Debug for Smtp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The server's address carries its credentials, so it is not printed.
        f.debug_struct("Smtp").field("from", &self.from).finish()
    }
}

impl Smtp {
    /// `url` is `smtp://host:port`, `smtps://user:password@host:465` or
    /// `smtp://user:password@host:587?tls=required`; `from` is the sender's address.
    pub fn new(url: &str, from: &str) -> Result<Self, String> {
        let transport = lettre::SmtpTransport::from_url(url)
            .map_err(|e| format!("STRATA_SMTP_URL: {e}"))?
            .timeout(Some(SMTP_TIMEOUT))
            .build();
        let from = from.parse().map_err(|e| format!("STRATA_MAIL_FROM: {e}"))?;
        Ok(Smtp {
            transport,
            from,
            pool: crate::lab::Pool::new("mail", SMTP_WORKERS, SMTP_QUEUE),
        })
    }
}

impl Mailer for Smtp {
    fn send(&self, mail: &Mail) -> Result<(), MailError> {
        use lettre::Transport;
        let to: lettre::message::Mailbox = mail
            .to
            .parse()
            .map_err(|e| MailError(format!("recipient: {e}")))?;
        let message = lettre::Message::builder()
            .from(self.from.clone())
            .to(to)
            .subject(mail.subject.clone())
            .header(lettre::message::header::ContentType::TEXT_PLAIN)
            .body(mail.text.clone())
            .map_err(|e| MailError(e.to_string()))?;
        let transport = self.transport.clone();
        self.pool
            .submit(move || {
                if let Err(error) = transport.send(&message) {
                    crate::app::LOG.error(
                        "mail.failed",
                        "",
                        serde_json::json!({ "error": error.to_string() }),
                    );
                }
            })
            .map_err(|_| MailError("the mail queue is full".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sink_keeps_what_it_is_given_and_a_line_break_does_not_split_a_header() {
        let sink = DevSink::memory();
        let mail = Mail {
            to: "a@x.example\nBcc: b@x.example".into(),
            subject: "Hello".into(),
            text: "Body".into(),
        };
        sink.send(&mail).unwrap();
        assert_eq!(sink.outbox(), vec![mail.clone()]);
        assert!(!eml(&mail).contains("\nBcc:"));
    }
}
