//! Mail templates, notice templates and tokens: the text a tenant sends and the named values it
//! places in text, held as tenant metadata in the tenant schema (EML-012, NTF-1570, CTK-001;
//! D-6580, D-6581).
//!
//! The schema holds them in the optional members `mail-templates`, `notice-templates` and
//! `tokens`, each left out of the encoding while it holds nothing, so a schema that never declared
//! one hashes as it did before, and every template and token is under the schema version like the
//! rest of the schema (the contracts `define-mail-template`, `define-notice-template` and
//! `define-token` say "stored as tenant metadata under the schema version").
//!
//! They change through six schema changes: [`SchemaChange::DefineMailTemplate`],
//! [`SchemaChange::RemoveMailTemplate`], [`SchemaChange::DefineNoticeTemplate`],
//! [`SchemaChange::RemoveNoticeTemplate`], [`SchemaChange::DefineToken`] and
//! [`SchemaChange::RemoveToken`]. Every define is define-or-replace; removing what does not exist
//! is refused.
//!
//! As with the definitions of [`crate::definitions`], the checks are made on the schema that holds
//! the definition ([`problems`]): a template id is used by one kind of template only (a mail
//! template and a notice template share one `template-id` space), a template has a one-line
//! subject or title and a body that is blocks of the schema of [`crate::blocks`] (D-7900): the
//! blocks conform, the values a template places are declared and typed, a link carries an allowed
//! scheme, nesting stays within the bound of the Markdown parser, and every token the body places
//! is one the schema defines (CTK-005). A token has a name and an expression that parses in the
//! tenant rule language (CTK-002); the language has no way to name another token, so a token
//! cannot refer to itself through others (CTK-003). The checks are made on the schema that holds
//! the template, so removing a token a template places is refused too (CTK-004).

use crate::blocks;
use crate::expr;
use crate::limits;
use crate::schema::TenantSchema;
use crate::types::{
    AutomationStep, EntityKind, MailTemplateDecl, NoticeTemplateDecl, SchemaChange, TemplateId,
    TokenDecl,
};
use std::collections::BTreeMap;

/// Whether `change` is one this module applies.
pub fn is_template_change(change: &SchemaChange) -> bool {
    matches!(
        change,
        SchemaChange::DefineMailTemplate(_)
            | SchemaChange::RemoveMailTemplate(_)
            | SchemaChange::DefineNoticeTemplate(_)
            | SchemaChange::RemoveNoticeTemplate(_)
            | SchemaChange::DefineToken(_)
            | SchemaChange::RemoveToken(_)
    )
}

/// Remove `key` from an optional table; a table left empty is no table at all, so a schema
/// without such definitions reads and hashes as one that never had them.
fn remove_from<K: Ord, V>(table: &mut Option<BTreeMap<K, V>>, key: &K) -> Option<V> {
    let map = table.as_mut()?;
    let removed = map.remove(key);
    if map.is_empty() {
        *table = None;
    }
    removed
}

/// Apply one change of this module to a working copy and describe it.
pub fn apply(schema: &mut TenantSchema, change: &SchemaChange) -> Result<String, String> {
    Ok(match change {
        SchemaChange::DefineMailTemplate(decl) => {
            schema
                .mail_templates
                .get_or_insert_with(BTreeMap::new)
                .insert(decl.template.clone(), decl.clone());
            format!("define mail template {}", decl.template)
        }
        SchemaChange::RemoveMailTemplate(id) => {
            remove_from(&mut schema.mail_templates, id)
                .ok_or_else(|| format!("no mail template {id}"))?;
            format!("remove mail template {id}")
        }
        SchemaChange::DefineNoticeTemplate(decl) => {
            schema
                .notice_templates
                .get_or_insert_with(BTreeMap::new)
                .insert(decl.template.clone(), decl.clone());
            format!("define notice template {}", decl.template)
        }
        SchemaChange::RemoveNoticeTemplate(id) => {
            remove_from(&mut schema.notice_templates, id)
                .ok_or_else(|| format!("no notice template {id}"))?;
            format!("remove notice template {id}")
        }
        SchemaChange::DefineToken(decl) => {
            schema
                .tokens
                .get_or_insert_with(BTreeMap::new)
                .insert(decl.name.clone(), decl.clone());
            format!("define token {}", decl.name)
        }
        SchemaChange::RemoveToken(name) => {
            remove_from(&mut schema.tokens, name).ok_or_else(|| format!("no token {name}"))?;
            format!("remove token {name}")
        }
        other => {
            return Err(format!(
                "{other:?} is not a change of a template or a token"
            ))
        }
    })
}

/// Whether `name` is a template id or a token name: lower-case letters, digits and underscores,
/// beginning with a letter, at most 64 bytes (the pattern of the ids of a product, D-6582).
fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 64
        && matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The first reason a one-line text (a subject or a title) is not one, or `None`.
fn line_problem(text: &str) -> Option<&'static str> {
    if text.trim().is_empty() {
        Some("is empty")
    } else if text.contains(['\r', '\n']) {
        Some("has a line break")
    } else if text.len() > 998 {
        Some("is longer than 998 bytes")
    } else {
        None
    }
}

fn template_problems(
    schema: &TenantSchema,
    kind: &str,
    key: &TemplateId,
    id: &TemplateId,
    head: (&str, &str),
    body: &str,
    problems: &mut Vec<String>,
) {
    let at = format!("{kind} template {key}");
    if key != id {
        problems.push(format!("{at}: is stored under another id, {id}"));
    }
    if !is_name(id.as_str()) {
        problems.push(format!(
            "{at}: the id is lower-case letters, digits and underscores, beginning with a letter"
        ));
    }
    if let Some(reason) = line_problem(head.1) {
        problems.push(format!("{at}: the {} {reason}", head.0));
    }
    // The body is blocks of the schema of `crate::blocks` (D-7900): the blocks conform, the values
    // it places are typed, and every token it places is one the tenant defines (CTK-005).
    let analysis = blocks::analyse(body);
    for reason in &analysis.problems {
        problems.push(format!("{at}: the body {reason}"));
    }
    for name in &analysis.tokens {
        if !schema
            .tokens
            .as_ref()
            .is_some_and(|tokens| tokens.contains_key(name))
        {
            problems.push(format!(
                "{at}: the body places the token {name}, which the tenant does not define"
            ));
        }
    }
}

/// The problems of the templates and tokens of a schema (EML-012, NTF-1570, CTK-001, CTK-002):
/// each is stored under its own id; a template id is not used by a mail template and a notice
/// template both; a template has a one-line subject or title and a body that is JSON text; a
/// token has a name and an expression that parses; and no automation step sends a notice
/// template as mail or creates a notice from a mail template.
pub fn problems(schema: &TenantSchema, problems: &mut Vec<String>) {
    for (key, decl) in schema.mail_templates.iter().flatten() {
        template_problems(
            schema,
            "mail",
            key,
            &decl.template,
            ("subject", &decl.subject),
            decl.body.as_str(),
            problems,
        );
        if schema
            .notice_templates
            .as_ref()
            .is_some_and(|notices| notices.contains_key(key))
        {
            problems.push(format!(
                "template {key}: is both a mail template and a notice template"
            ));
        }
    }
    for (key, decl) in schema.notice_templates.iter().flatten() {
        template_problems(
            schema,
            "notice",
            key,
            &decl.template,
            ("title", &decl.title),
            decl.body.as_str(),
            problems,
        );
    }
    for (key, token) in schema.tokens.iter().flatten() {
        let at = format!("token {key}");
        if *key != token.name {
            problems.push(format!(
                "{at}: is stored under another name, {}",
                token.name
            ));
        }
        if !is_name(&token.name) {
            problems.push(format!(
                "{at}: the name is lower-case letters, digits and underscores, beginning with a letter"
            ));
        }
        if let Err(e) = expr::parse(token.expression.as_str()) {
            problems.push(format!("{at}: expression: {e}"));
        }
        if token.fallback.len() > limits::MAX_STRING_BYTES {
            problems.push(format!(
                "{at}: the fallback is longer than {} bytes",
                limits::MAX_STRING_BYTES
            ));
        }
    }
    for automation in schema.automations.iter().flat_map(BTreeMap::values) {
        for (name, step) in &automation.steps {
            let (template, wants_mail) = match &step.action {
                AutomationStep::SendMail { template, .. } => (template, true),
                AutomationStep::CreateNotice { template, .. } => (template, false),
                _ => continue,
            };
            let mail = schema
                .mail_templates
                .as_ref()
                .is_some_and(|t| t.contains_key(template));
            let notice = schema
                .notice_templates
                .as_ref()
                .is_some_and(|t| t.contains_key(template));
            if wants_mail && notice && !mail {
                problems.push(format!(
                    "automation {}.{name}: sends the notice template {template} as mail",
                    automation.id
                ));
            }
            if !wants_mail && mail && !notice {
                problems.push(format!(
                    "automation {}.{name}: creates a notice from the mail template {template}",
                    automation.id
                ));
            }
        }
    }
}

/// One template of the schema, as [`list`] shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateEntry<'a> {
    /// `EntityKind::MailTemplate` or `EntityKind::NoticeTemplate`.
    pub kind: EntityKind,
    pub id: &'a TemplateId,
    /// The subject of a mail template, the title of a notice template.
    pub head: &'a str,
    /// The blocks, as JSON text.
    pub body: &'a str,
}

/// Every template of the schema, mail templates first, each kind in id order.
pub fn list(schema: &TenantSchema) -> Vec<TemplateEntry<'_>> {
    let mail =
        schema
            .mail_templates
            .iter()
            .flatten()
            .map(|(id, t): (&TemplateId, &MailTemplateDecl)| TemplateEntry {
                kind: EntityKind::MailTemplate,
                id,
                head: &t.subject,
                body: t.body.as_str(),
            });
    let notice = schema.notice_templates.iter().flatten().map(
        |(id, t): (&TemplateId, &NoticeTemplateDecl)| TemplateEntry {
            kind: EntityKind::NoticeTemplate,
            id,
            head: &t.title,
            body: t.body.as_str(),
        },
    );
    mail.chain(notice).collect()
}

/// Every token of the schema, in name order.
pub fn tokens(schema: &TenantSchema) -> Vec<&TokenDecl> {
    schema.tokens.iter().flatten().map(|(_, t)| t).collect()
}
