//! Normalisation on write (WG-5597): a field that declares one stores its text in that form, so
//! two spellings of one value store one value, and a unique constraint sees them as one.
//!
//! `nfc` stores Unicode Normalization Form C. `emoji` stores an emoji in its VS16 form: each
//! text-default emoji character (Emoji=Yes with Emoji_Presentation=No, such as U+2764 or U+00A9)
//! takes U+FE0F after it, so `❤` and `❤️` store the same text. The form is applied only where the
//! result is one emoji (`is_emoji` in the tenant rule language); any other text is stored in NFC
//! and gets no VS16. The validation that follows sees the stored form, so `is_emoji`, `len` and
//! the unique constraint all judge what is kept.

use crate::schema::ObjectTypeDef;
use crate::types::{FieldId, Normalisation, Value};
use std::collections::BTreeMap;
use strata_model_core::tenant_rule_language::is_emoji;
use unicode_normalization::UnicodeNormalization;
use unicode_properties::emoji::{EmojiStatus, UnicodeEmoji};

const VS16: char = '\u{FE0F}';

/// The stored form of a text under a normalisation.
pub fn normalised(form: Normalisation, text: &str) -> String {
    let nfc: String = text.nfc().collect();
    match form {
        Normalisation::Nfc => nfc,
        Normalisation::Emoji => emoji_form(&nfc),
    }
}

/// Stores every text value of `fields` in the form its field of `object` declares.
pub fn normalise_fields(object: &ObjectTypeDef, fields: &mut BTreeMap<FieldId, Value>) {
    for (id, value) in fields.iter_mut() {
        let Some(form) = object.fields.get(id).and_then(|def| def.normalise) else {
            continue;
        };
        if let Value::Str(text) = value {
            let stored = normalised(form, text);
            if stored != *text {
                *text = stored;
            }
        }
    }
}

/// The VS16 form of a text when the text is one emoji once its text-default emoji characters take
/// VS16; the text itself (in NFC) otherwise.
fn emoji_form(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        out.push(c);
        if text_default_emoji(c) && chars.get(i + 1) != Some(&VS16) {
            out.push(VS16);
        }
    }
    if is_emoji(&out) {
        out
    } else {
        text.to_string()
    }
}

/// Emoji=Yes without Emoji_Presentation: an emoji only with VS16 (or a skin tone modifier) after it.
fn text_default_emoji(c: char) -> bool {
    matches!(
        c.emoji_status(),
        EmojiStatus::EmojiOther
            | EmojiStatus::EmojiOtherAndEmojiComponent
            | EmojiStatus::EmojiModifierBase
    )
}
