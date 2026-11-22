//! The scales, and the share each object type takes of a population.

use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scale {
    /// A few hundred records, every type present: for tests that need a populated tenant fast.
    Tiny,
    Small,
    Medium,
    Large,
}

impl Scale {
    /// The number of records the scale aims at in all. The count of one object type is its share
    /// of this, with a floor (see [`Scale::floor`]), so the total is a little above it.
    pub fn target(self) -> u64 {
        match self {
            Scale::Tiny => 300,
            Scale::Small => 3_000,
            Scale::Medium => 30_000,
            Scale::Large => 130_000,
        }
    }

    /// The fewest records an object type has, so that every type can be addressed and the
    /// reference pools are never empty.
    pub fn floor(self) -> u64 {
        match self {
            Scale::Tiny => 2,
            Scale::Small => 4,
            Scale::Medium => 6,
            Scale::Large => 10,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Scale::Tiny => "tiny",
            Scale::Small => "small",
            Scale::Medium => "medium",
            Scale::Large => "large",
        }
    }
}

impl fmt::Display for Scale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Scale {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        match text {
            "tiny" => Ok(Scale::Tiny),
            "small" => Ok(Scale::Small),
            "medium" => Ok(Scale::Medium),
            "large" => Ok(Scale::Large),
            other => Err(format!(
                "{other:?} is not a scale; use tiny, small, medium or large"
            )),
        }
    }
}

/// The share of the population an object type takes, in arbitrary units: configuration is a few
/// records, entities are thousands, and the rows that hang off them (lines, log entries,
/// touches, sends) are most of the data. An object type the table does not name takes
/// [`DEFAULT_SHARE`].
pub const SHARES: &[(&str, f64)] = &[
    ("analytics__dashboard", 3.0),
    ("analytics__tile", 15.0),
    ("analytics__metric_definition", 3.0),
    ("analytics__kpi_threshold", 4.0),
    ("analytics__report_schedule", 4.0),
    ("approvals__approval_request", 30.0),
    ("approvals__approval_stage", 60.0),
    ("approvals__delegation", 5.0),
    ("approvals__sla_policy", 1.0),
    ("approvals__escalation", 6.0),
    ("assets__asset", 120.0),
    ("assets__rendition", 150.0),
    ("assets__licence", 8.0),
    ("assets__usage_right", 30.0),
    ("assets__collection", 5.0),
    ("automation__connector", 1.0),
    ("automation__webhook_subscription", 2.0),
    ("automation__schedule", 3.0),
    ("automation__run_log", 150.0),
    ("automation__budget_grant", 1.0),
    ("commerce__category", 1.0),
    ("commerce__product", 25.0),
    ("commerce__sku", 70.0),
    ("commerce__price_list", 70.0),
    ("commerce__promotion", 2.0),
    ("commerce__order", 120.0),
    ("commerce__order_line", 300.0),
    ("commerce__return_request", 15.0),
    ("commerce__redemption", 30.0),
    ("content__article", 60.0),
    ("content__revision", 120.0),
    ("content__page", 25.0),
    ("content__block", 80.0),
    ("content__embargo", 8.0),
    ("crm__contact", 100.0),
    ("crm__account", 15.0),
    ("crm__lead", 50.0),
    ("crm__consent_record", 150.0),
    ("crm__campaign_touch", 250.0),
    ("forms__form_definition", 3.0),
    ("forms__form_submission", 120.0),
    ("forms__consent_capture", 60.0),
    ("forms__attachment", 70.0),
    ("forms__webhook_subscription", 1.0),
    ("localisation__translation_job", 25.0),
    ("localisation__glossary_term", 20.0),
    ("localisation__translation_memory_entry", 120.0),
    ("localisation__translator", 2.0),
    ("localisation__translator_assignment", 30.0),
    ("marketing__campaign", 3.0),
    ("marketing__journey", 4.0),
    ("marketing__journey_step", 15.0),
    ("marketing__audience_member", 120.0),
    ("marketing__email_send", 300.0),
    ("people__employee", 25.0),
    ("people__department", 1.0),
    ("people__cost_centre", 1.0),
    ("people__agency", 1.0),
    ("people__agency_assignment", 8.0),
    ("people__leave_request", 40.0),
    ("publishing__channel", 1.0),
    ("publishing__publish_job", 100.0),
    ("publishing__release", 5.0),
    ("publishing__syndication_target", 1.0),
    ("service__case", 120.0),
    ("service__queue", 1.0),
    ("service__macro", 2.0),
    ("service__knowledge_article", 15.0),
    ("service__sla_clock", 140.0),
    ("taxonomy__category", 3.0),
    ("taxonomy__tag", 10.0),
    ("taxonomy__region", 6.0),
    ("taxonomy__audience", 1.0),
];

pub const DEFAULT_SHARE: f64 = 5.0;

pub fn share_of(object: &str) -> f64 {
    SHARES
        .iter()
        .find(|(id, _)| *id == object)
        .map_or(DEFAULT_SHARE, |(_, share)| *share)
}

/// How hard the draws of a reference to this object type favour the first of its records: the
/// exponent of the Zipf distribution. A few products, authors, queues and contacts take most of
/// the references; the rest of the types are a gentler skew.
pub fn skew_of(object: &str) -> f64 {
    const HOT: &[&str] = &[
        "commerce__product",
        "commerce__sku",
        "crm__contact",
        "crm__account",
        "service__queue",
        "service__case",
        "content__article",
        "people__employee",
        "people__department",
        "assets__asset",
        "publishing__channel",
        "marketing__campaign",
    ];
    if HOT.contains(&object) {
        1.15
    } else {
        0.85
    }
}
