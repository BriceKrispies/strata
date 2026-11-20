//! The step `send-digest` of an automation (NTF-1572, NTF-3200): one message for each person who
//! holds notices of a category created in a window, listing them, and none for a person who holds
//! none. The message is an ordinary mail job of the dispatcher, made as the automation principal
//! and keyed by the step key and the person, so a step attempted again sends no second message to
//! anyone and the notices of the window are in at most one digest of the step.
//!
//! The persons are those the step's recipient names: a principal, every member of a segment, or,
//! with no recipient, every member of the tenant. An address has no inbox and so no notices, and
//! is refused. The template receives the tokens `count` (how many notices) and `notices` (their
//! titles, one to a line, at most [`LISTED`]).

use crate::app::App;
use crate::dispatch::{self, Recipient};
use crate::inbox;
use std::collections::BTreeMap;
use strata_kernel::{PrincipalId, TemplateId, TenantId, Timestamp, WorkflowId};
use strata_model::types::SendRecipient;

/// The most titles one digest lists.
const LISTED: usize = 50;
/// The most persons a digest of a segment reaches.
const BOUND: usize = 100_000;
/// The longest window, in milliseconds (the contract send-digest: 31 days).
const LONGEST_WINDOW_MS: i64 = 31 * 24 * 60 * 60 * 1000;

/// What a digest step asks for (the fields of `send-digest-step`).
pub struct Digest<'a> {
    pub category: &'a str,
    pub template: &'a TemplateId,
    /// The window: notices created at or after `since` and before `until`, in milliseconds.
    pub since: i64,
    pub until: i64,
    pub recipient: Option<&'a SendRecipient>,
    pub reference: Option<&'a str>,
}

/// The persons whose notices the digest gathers.
fn persons(
    app: &App,
    tenant: &TenantId,
    recipient: Option<&SendRecipient>,
) -> Result<Vec<PrincipalId>, (String, bool)> {
    match recipient {
        Some(SendRecipient::Principal { principal }) => Ok(vec![principal.clone()]),
        Some(SendRecipient::Address { .. }) => Err((
            "an address has no inbox, so a digest cannot name one".to_string(),
            false,
        )),
        Some(SendRecipient::Segment { segment }) => {
            let mut out: Vec<PrincipalId> = Vec::new();
            let mut after: Option<PrincipalId> = None;
            loop {
                let page = app
                    .store
                    .segment_members(tenant, *segment, after.as_ref(), 1_000)
                    .map_err(|e| (e.to_string(), true))?;
                out.extend(page.iter().map(|member| member.person.clone()));
                if page.len() < 1_000 || out.len() >= BOUND {
                    break;
                }
                after = page.last().map(|member| member.person.clone());
            }
            Ok(out)
        }
        None => {
            let state = app
                .store
                .load(tenant, &[])
                .map_err(|e| (e.to_string(), true))?;
            let tenant_state = state
                .tenant
                .ok_or_else(|| (format!("no tenant {tenant}"), false))?;
            Ok(tenant_state.members.keys().cloned().collect())
        }
    }
}

/// The text of the notices of one person, one title to a line.
fn listing(items: &[inbox::Item]) -> String {
    let mut lines: Vec<String> = items
        .iter()
        .take(LISTED)
        .map(|item| item.title.replace(['\r', '\n'], " "))
        .collect();
    if items.len() > LISTED {
        lines.push(format!("and {} more", items.len() - LISTED));
    }
    lines.join("\n")
}

/// Send the digest. Answers how many persons were sent a message, or why not and whether an
/// attempt made again may succeed.
pub fn run(
    app: &App,
    tenant: &TenantId,
    automation: &WorkflowId,
    digest: &Digest<'_>,
    step_key: &str,
    now: Timestamp,
) -> Result<u32, (String, bool)> {
    if digest.until <= digest.since || digest.until - digest.since > LONGEST_WINDOW_MS {
        return Err((
            "the window of a digest is empty or longer than 31 days".to_string(),
            false,
        ));
    }
    let mut sent = 0u32;
    for person in persons(app, tenant, digest.recipient)? {
        let items = inbox::digest_items(
            app,
            tenant,
            &person,
            digest.category,
            digest.since,
            digest.until,
        )
        .map_err(|e| (e.describe(), true))?;
        if items.is_empty() {
            continue;
        }
        let values = BTreeMap::from([
            ("count".to_string(), items.len().to_string()),
            ("notices".to_string(), listing(&items)),
        ]);
        dispatch::step_send_to(
            app,
            tenant,
            automation,
            digest.template,
            Recipient::Principal {
                principal: person.clone(),
            },
            Some(digest.category),
            values,
            digest.reference,
            &format!("{step_key}:{person}"),
            now,
        )?;
        sent += 1;
    }
    Ok(sent)
}
