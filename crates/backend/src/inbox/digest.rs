//! Digests and the end of an inbox item (NTF-1572, D-6573, ret-inbox-item; component
//! `notification-service`).
//!
//! # Digests
//!
//! A person whose mode for a category and a channel is `digest` (NTF-1571) receives the notices of
//! that category created in each period as one message that lists them, once for each period, with
//! no message for an empty period and no second message for a notice that was in a digest
//! (NTF-1572). The period is the person's `daily` or `weekly` (daily when none is set) and ends at
//! 09:00 in the person's zone (the person setting `time-zone`), on Monday for a weekly digest
//! (D-6573).
//!
//! - **When.** A chain of jobs of kind `digest` ([`JobWork::Digest`]) runs once an hour for a tenant
//!   that has a digest preference, as the retention sweep does: each job delivers what is due and
//!   creates the next in the report that ends it. [`ensure_chain`] starts the chain when a person
//!   sets digest; a chain with nothing left to deliver ends and the next such preference starts
//!   it again.
//! - **What is listed.** The live items of the person in the category that were created before the
//!   start of the current period and are in no digest of the channel yet. Each item records the
//!   digest of each channel it was listed in ([`Item::digested`]), so a notice is listed once per
//!   channel however often the chain runs.
//! - **Once for each period.** The digest has an id derived from the tenant, the person, the
//!   category, the channel and the start of the period; the message is made under that id (the
//!   inbox item is created only when its id is free, the mail is submitted under it as its
//!   idempotency key), and a marker document keeps the id, so a job that runs twice, or is
//!   attempted again after a crash, makes one message.
//! - **The message.** On the inbox channel it is one inbox item (topic [`DIGEST_TOPIC`], no
//!   category) whose text lists the titles, pushed to the person's open sockets unless they are
//!   muted or in quiet hours; the tenant may define the notice template `digest_<category>` (the
//!   hyphens of the category written as underscores) that places the values `items` and `count`,
//!   and without it the text is built in. On the mail channel it is one message of the mail
//!   template `digest_mail_<category>` through the dispatcher; a tenant without that template has
//!   no mail digest and the notices stay unlisted until it defines one.
//!
//! # The end of an inbox item (ret-inbox-item)
//!
//! An inbox item ends when its recipient removes it ([`super::remove`]) or when the record it names
//! is erased (NTF-151). [`end_for_erasure`] is the second end: the erasure of a subject ends every
//! item that names an erased record, whoever its recipient is, and every item of the subject
//! themself, and removes the subject's preferences. An ended item keeps its id as a tombstone with
//! its title, text and values erased, so that a send that is repeated does not make it again
//! (CT-183); the tombstone holds no personal value.
//!
//! Implements: CT-826, CT-828, NTF-1572

use super::*;
use crate::app::LOG;
use crate::dispatch::{self, Recipient, SendInput};
use crate::worker::steps::Outcome;
use strata_storage_api::jobs::{JobReport, Lease, WorkerWrites};

/// The topic of the item that is a digest.
pub const DIGEST_TOPIC: &str = "app.digest";
/// The kind of the documents that mark the digests that were made.
pub const DIGEST_KIND: &str = "notice-digest";
/// The kind of the documents that name a person with an inbox, so that an erasure finds the items
/// that name a record in every inbox.
pub const RECIPIENT_KIND: &str = "inbox-recipient";
/// The time between two runs of the chain.
pub const INTERVAL_MS: i64 = 3_600_000;
/// The hour a period ends, in the person's zone.
const NINE_MS: i64 = 9 * 3_600_000;
const DAY_MS: i64 = 86_400_000;

/// How often a person's digest comes (`digest-period`, NTF-1572).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Period {
    Daily,
    Weekly,
}

impl Period {
    pub fn parse(text: &str) -> Option<Period> {
        match text {
            "daily" => Some(Period::Daily),
            "weekly" => Some(Period::Weekly),
            _ => None,
        }
    }
}

/// The start, in milliseconds since the epoch, of the period that holds `now` for a person whose
/// zone is `offset_minutes` from UTC: 09:00 in the zone, and for a weekly period the Monday at
/// 09:00 (D-6573).
pub fn period_start(period: Period, offset_minutes: i64, now: i64) -> i64 {
    let zone = offset_minutes * 60_000;
    // Local time moved back nine hours, so that a period begins at a midnight.
    let shifted = now + zone - NINE_MS;
    let day = shifted.div_euclid(DAY_MS);
    let first_day = match period {
        Period::Daily => day,
        // The epoch day, a Thursday, is day 0, so the Mondays are the days that are 4 mod 7.
        Period::Weekly => day - (day - 4).rem_euclid(7),
    };
    first_day * DAY_MS + NINE_MS - zone
}

/// The id of the digest of a person, a category and a channel for the period that starts at
/// `start`: the same on every attempt.
pub fn digest_id(
    tenant: &TenantId,
    person: &PrincipalId,
    category: &str,
    channel: &str,
    start: i64,
) -> String {
    derived_uuid(&format!(
        "strata/notice-digest/v1\0{tenant}\0{person}\0{category}\0{channel}\0{start}"
    ))
}

/// The name of the template of a digest on a channel for a category.
pub fn template_name(category: &str, channel: &str) -> String {
    let category = category.replace('-', "_");
    if channel == "mail" {
        format!("digest_mail_{category}")
    } else {
        format!("digest_{category}")
    }
}

/// What the preference of a person for a category on a channel is: the mode and the period.
pub fn preference_with_period(
    app: &App,
    tenant: &TenantId,
    recipient: &PrincipalId,
    category: &str,
    channel: &str,
) -> Result<(Mode, Period), ApiError> {
    let key = preference_key(recipient, category, channel);
    match app.store.doc_get(tenant, PREFERENCE_KIND, &key)? {
        None => Ok((Mode::On, Period::Daily)),
        Some(text) => Ok(read_preference(&key, &text)?),
    }
}

fn read_preference(key: &str, text: &str) -> Result<(Mode, Period), ApiError> {
    let found: Json_ = decode(PREFERENCE_KIND, key, text)?;
    Ok((
        found
            .get("mode")
            .and_then(Json_::as_str)
            .and_then(Mode::parse)
            .unwrap_or(Mode::On),
        found
            .get("period")
            .and_then(Json_::as_str)
            .and_then(Period::parse)
            .unwrap_or(Period::Daily),
    ))
}

/// What one run of the chain did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Run {
    /// The digests made.
    pub digests: u32,
    /// The preferences of mode digest that were read.
    pub preferences: u32,
}

fn tokens_of(state: &TenantState) -> impl Fn(&str) -> Option<String> + '_ {
    move |name: &str| {
        state
            .schema
            .tokens
            .as_ref()
            .and_then(|all| all.get(name))
            .map(|token| token.fallback.clone())
    }
}

/// Deliver the digest of one person, category and channel for the period that starts at `start`,
/// if it is due: there are notices to list and the digest of this period was not made. Returns
/// whether one was made.
#[allow(clippy::too_many_arguments)]
pub fn deliver_digest(
    app: &App,
    tenant: &TenantId,
    state: &TenantState,
    person: &PrincipalId,
    category: &str,
    channel: &str,
    start: i64,
    now: Timestamp,
) -> Result<bool, ApiError> {
    let id = digest_id(tenant, person, category, channel, start);
    let _writing = writing();
    if app.store.doc_get(tenant, DIGEST_KIND, &id)?.is_some() {
        return Ok(false);
    }
    let mut listed: Vec<Item> = load_items(app, tenant, person)?
        .into_iter()
        .filter(|item| {
            item.live()
                && item.category.as_deref() == Some(category)
                && item.created_at < start
                && !item.digested.contains_key(channel)
        })
        .collect();
    if listed.is_empty() {
        return Ok(false);
    }
    // Oldest first, as a reader lists them.
    listed.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    let lines: Vec<String> = listed.iter().map(|item| item.title.clone()).collect();
    let values = BTreeMap::from([
        ("items".to_string(), lines.join("\n")),
        ("count".to_string(), listed.len().to_string()),
    ]);
    let template = template_name(category, channel);
    let tokens = tokens_of(state);
    if channel == "mail" {
        let sent = dispatch::submit(
            app,
            tenant,
            &SendInput {
                template: TemplateId(template),
                recipient: Recipient::Principal {
                    principal: person.clone(),
                },
                category: Some(category.to_string()),
                class: None,
                idempotency_key: format!("digest:{id}"),
                values,
                reference: None,
            },
            now,
        );
        match sent {
            Ok(_) => {}
            Err(error) if error.0.code == ErrorCode::NotFound => {
                LOG.warn(
                    "inbox.digest_without_template",
                    "",
                    json!({ "tenant": tenant.as_str(), "category": category }),
                );
                return Ok(false);
            }
            Err(error) => return Err(error),
        }
    } else {
        let (title, body) = match state
            .schema
            .notice_templates
            .as_ref()
            .and_then(|all| all.get(&TemplateId(template.clone())))
        {
            Some(found) => render(&found.title, found.body.as_str(), &values, &tokens)
                .map_err(|problem| invalid(format!("The template {template}: {problem}.")))?,
            None => (
                format!(
                    "{} new {}",
                    listed.len(),
                    if listed.len() == 1 {
                        "notice"
                    } else {
                        "notices"
                    }
                ),
                plain(&lines.join("\n"), MAX_BODY_BYTES),
            ),
        };
        let push = push_hold(state, person, now.0).unwrap_or(PushState::Sent);
        let item = Item {
            id: id.clone(),
            recipient: person.as_str().to_string(),
            topic: DIGEST_TOPIC.to_string(),
            category: None,
            template,
            title,
            body,
            values: BTreeMap::from([("count".to_string(), listed.len().to_string())]),
            created_at: now.0,
            read_at: None,
            removed_at: None,
            reference: None,
            record: None,
            push,
            digested: BTreeMap::new(),
        };
        if create_item(app, tenant, &item)? && push == PushState::Sent {
            let unread = unread_count(&load_items(app, tenant, person)?, None);
            announce(app, tenant, "arrived", &item, unread, now.0);
        }
        let _ = app
            .store
            .doc_put(tenant, RECIPIENT_KIND, person.as_str(), "{}", true)?;
    }
    for mut item in listed {
        item.digested.insert(channel.to_string(), id.clone());
        save_item(app, tenant, &item)?;
    }
    app.store.doc_put(
        tenant,
        DIGEST_KIND,
        &id,
        &json!({ "channel": channel, "category": category, "start": start, "at": now.0 })
            .to_string(),
        false,
    )?;
    Ok(true)
}

/// Deliver every digest that is due at `now`: for each preference of mode digest, the digest of the
/// period that has just ended.
pub fn run_due(app: &App, tenant: &TenantId, now: Timestamp) -> Result<Run, ApiError> {
    let state = app
        .store
        .load(tenant, &[])?
        .tenant
        .ok_or_else(|| not_found(format!("no tenant {tenant}")))?;
    let mut run = Run::default();
    for (key, text) in app.store.doc_list(tenant, PREFERENCE_KIND)? {
        let (mode, period) = read_preference(&key, &text)?;
        if mode != Mode::Digest {
            continue;
        }
        let mut parts = key.splitn(3, '|');
        let (Some(person), Some(category), Some(channel)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let person = principal_of(person)?;
        run.preferences += 1;
        let offset = match recipient_setting(&state, &person, TIME_ZONE) {
            Some(Value::Str(zone)) => zone_offset_minutes(&zone),
            _ => 0,
        };
        let start = period_start(period, offset, now.0);
        if deliver_digest(app, tenant, &state, &person, category, channel, start, now)? {
            run.digests += 1;
        }
    }
    Ok(run)
}

fn chain_job(step: u64, due: Timestamp) -> NewJob {
    NewJob {
        key: format!("digest:{step}"),
        work: JobWork::Digest { step },
        due_at: due,
        policy: RetryPolicy::default(),
    }
}

/// Start the chain of digest jobs of a tenant when it has none open, due at the next whole hour.
/// Returns whether a job was created.
pub fn ensure_chain(app: &App, tenant: &TenantId, now: Timestamp) -> Result<bool, ApiError> {
    let mut open = false;
    let mut latest: Option<u64> = None;
    for job in app.store.jobs_list(tenant, None, 1_000)? {
        if let JobWork::Digest { step } = &job.work {
            open |= !job.state.ended();
            latest = Some(latest.map_or(*step, |seen| seen.max(*step)));
        }
    }
    if open {
        return Ok(false);
    }
    let step = latest.map_or(0, |seen| seen + 1);
    let due = Timestamp((now.0.div_euclid(INTERVAL_MS) + 1) * INTERVAL_MS);
    Ok(!app
        .store
        .jobs_create(tenant, &[chain_job(step, due)], now)?
        .is_empty())
}

/// Run a job of the chain: deliver what is due, and create the next job when a preference of mode
/// digest remains.
pub fn run(app: &App, tenant: &TenantId, lease: &Lease, step: u64, now: Timestamp) -> Outcome {
    match run_due(app, tenant, now) {
        Ok(done) => {
            let mut writes = WorkerWrites::default();
            if done.preferences > 0 {
                let due = Timestamp((now.0.div_euclid(INTERVAL_MS) + 1) * INTERVAL_MS);
                writes.jobs.push(chain_job(step + 1, due));
            }
            Outcome {
                report: JobReport::Succeeded,
                writes,
            }
        }
        Err(error) => Outcome {
            report: JobReport::Failed {
                error: strata_storage_api::jobs::clip_error(&error.describe()),
                retry: !lease.exhausted,
            },
            writes: WorkerWrites::default(),
        },
    }
}

/// What an erasure ended in the inboxes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ended {
    /// The items that named an erased record or belonged to the subject.
    pub items: u64,
    /// The preferences of the subject that were removed.
    pub preferences: u64,
}

/// End the inbox items that an erasure reaches (ret-inbox-item, NTF-151): every item, of any
/// recipient, that names one of the `erased` records, and every item of the `subject`; and remove
/// the subject's preferences. The items are ended as a removal ends them, and the recipients'
/// open sockets are told.
pub fn end_for_erasure(
    app: &App,
    tenant: &TenantId,
    subject: &PrincipalId,
    erased: &BTreeSet<u64>,
    now: Timestamp,
) -> Result<Ended, ApiError> {
    let mut ended = Ended::default();
    let _writing = writing();
    let mut recipients: BTreeSet<String> = app
        .store
        .doc_list(tenant, RECIPIENT_KIND)?
        .into_iter()
        .map(|(person, _)| person)
        .collect();
    recipients.insert(subject.as_str().to_string());
    for person in recipients {
        let recipient = principal_of(&person)?;
        let own = recipient == *subject;
        let mut told = Vec::new();
        for mut item in load_items(app, tenant, &recipient)? {
            let names_erased = item.record.is_some_and(|record| erased.contains(&record));
            if !item.live() || !(own || names_erased) {
                continue;
            }
            item.removed_at = Some(now.0);
            item.title.clear();
            item.body.clear();
            item.values.clear();
            save_item(app, tenant, &item)?;
            ended.items += 1;
            told.push(item);
        }
        if !told.is_empty() {
            let unread = unread_count(&load_items(app, tenant, &recipient)?, None);
            for item in &told {
                announce(app, tenant, "ended", item, unread, now.0);
            }
        }
    }
    for (key, _) in app.store.doc_list(tenant, PREFERENCE_KIND)? {
        if key.split('|').next() == Some(subject.as_str())
            && app.store.doc_remove(tenant, PREFERENCE_KIND, &key)?
        {
            ended.preferences += 1;
        }
    }
    app.store
        .doc_remove(tenant, RECIPIENT_KIND, subject.as_str())?;
    Ok(ended)
}
