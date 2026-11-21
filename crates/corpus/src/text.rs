//! Words, people and sentences for the population.

use crate::rng::Rng;

pub const FIRST_NAMES: &[&str] = &[
    "Aaliyah",
    "Adrian",
    "Aiko",
    "Alejandro",
    "Amara",
    "Anders",
    "Anya",
    "Arjun",
    "Beatrice",
    "Bram",
    "Camille",
    "Carlos",
    "Chidi",
    "Chloe",
    "Dara",
    "David",
    "Dmitri",
    "Elena",
    "Emeka",
    "Farid",
    "Fatima",
    "Felix",
    "Freya",
    "Gabriel",
    "Greta",
    "Hana",
    "Hassan",
    "Helena",
    "Hiro",
    "Ibrahim",
    "Ingrid",
    "Isabel",
    "Ivan",
    "Jamal",
    "Janelle",
    "Jonas",
    "Josephine",
    "Kai",
    "Kamala",
    "Karim",
    "Katarina",
    "Kofi",
    "Lars",
    "Leila",
    "Liam",
    "Lucia",
    "Magnus",
    "Maria",
    "Mateo",
    "Mei",
    "Nadia",
    "Naomi",
    "Nikolai",
    "Noor",
    "Olga",
    "Omar",
    "Priya",
    "Rafael",
    "Rania",
    "Rosa",
    "Sana",
    "Sasha",
    "Sofia",
    "Stefan",
    "Tariq",
    "Thea",
    "Tomas",
    "Uma",
    "Valentina",
    "Viktor",
    "Wei",
    "Xavier",
    "Yara",
    "Yusuf",
    "Zainab",
    "Zoe",
    "Oskar",
    "Marta",
    "Ravi",
    "Elif",
];

pub const LAST_NAMES: &[&str] = &[
    "Abara",
    "Adeyemi",
    "Andersen",
    "Banerjee",
    "Bergstrom",
    "Castillo",
    "Chen",
    "Dubois",
    "Eriksen",
    "Fernandez",
    "Fischer",
    "Garcia",
    "Haddad",
    "Hoffmann",
    "Ibarra",
    "Ivanova",
    "Jensen",
    "Kapoor",
    "Kim",
    "Kowalski",
    "Laurent",
    "Lindqvist",
    "Mansour",
    "Martinez",
    "Mbeki",
    "Moreau",
    "Nakamura",
    "Novak",
    "Okafor",
    "Olsen",
    "Park",
    "Petrov",
    "Quinn",
    "Rahman",
    "Rossi",
    "Santos",
    "Schmidt",
    "Singh",
    "Sokolov",
    "Suzuki",
    "Tanaka",
    "Torres",
    "Ueda",
    "Vargas",
    "Weber",
    "Wong",
    "Yilmaz",
    "Zhang",
    "Zielinski",
    "Okonkwo",
    "Haugen",
    "Delgado",
    "Nguyen",
    "Popescu",
    "Reyes",
    "Volkov",
];

pub const DOMAINS: &[&str] = &[
    "example.com",
    "example.org",
    "example.net",
    "mail.example",
    "corp.example",
    "people.example",
];

pub const WORDS: &[&str] = &[
    "quarter",
    "launch",
    "brand",
    "regional",
    "annual",
    "policy",
    "update",
    "campaign",
    "product",
    "release",
    "customer",
    "journey",
    "pricing",
    "support",
    "partner",
    "digital",
    "global",
    "local",
    "summer",
    "winter",
    "spring",
    "autumn",
    "welcome",
    "renewal",
    "feature",
    "guide",
    "overview",
    "report",
    "review",
    "plan",
    "roadmap",
    "standard",
    "premium",
    "basic",
    "enterprise",
    "starter",
    "mobile",
    "desktop",
    "network",
    "security",
    "privacy",
    "consent",
    "billing",
    "invoice",
    "shipping",
    "returns",
    "catalogue",
    "archive",
    "draft",
    "final",
    "interim",
    "priority",
    "executive",
    "technical",
    "legal",
    "financial",
    "creative",
    "editorial",
    "analytics",
    "insight",
    "metric",
    "target",
    "forecast",
    "budget",
    "supplier",
    "contract",
    "service",
    "platform",
    "workflow",
    "template",
    "asset",
    "library",
    "gallery",
    "banner",
    "newsletter",
    "webinar",
    "event",
    "survey",
    "feedback",
    "onboarding",
    "training",
    "handbook",
    "benefit",
];

pub const FILLER: &[&str] = &[
    "the", "of", "and", "to", "in", "for", "with", "on", "by", "that", "this", "as", "is", "are",
    "was", "will", "be", "an", "it", "from", "at", "or", "which", "we", "our", "their", "has",
    "have", "can", "should", "must", "all", "each", "any", "new", "more", "after", "before",
];

pub const LOCALES: &[&str] = &[
    "en", "en-GB", "fr", "de", "es", "it", "pt-BR", "ja", "nl", "sv",
];
pub const REGIONS: &[&str] = &[
    "emea", "apac", "amer", "latam", "dach", "nordics", "iberia", "anz",
];
pub const COUNTRIES: &[&str] = &[
    "GB", "US", "DE", "FR", "ES", "IT", "NL", "SE", "JP", "BR", "AU", "CA", "IN", "SG", "ZA",
];
pub const CURRENCIES: &[&str] = &["GBP", "USD", "EUR", "JPY", "SEK", "AUD", "CAD"];
pub const CITIES: &[&str] = &[
    "London",
    "Berlin",
    "Paris",
    "Madrid",
    "Milan",
    "Amsterdam",
    "Stockholm",
    "Tokyo",
    "Sao Paulo",
    "Sydney",
    "Toronto",
    "Mumbai",
    "Singapore",
    "Cape Town",
    "New York",
];

/// Categorical values by the kind of field name.
pub fn categories(name: &str) -> &'static [&'static str] {
    if name.contains("status") || name.contains("state") || name.contains("stage") {
        &[
            "new",
            "active",
            "pending",
            "in_review",
            "approved",
            "done",
            "archived",
        ]
    } else if name.contains("priority") || name.contains("severity") {
        &["low", "normal", "high", "urgent"]
    } else if name.contains("channel") {
        &["email", "web", "phone", "social", "print", "sms"]
    } else if name.contains("outcome") || name.contains("result") {
        &["success", "partial", "failed", "skipped", "retried"]
    } else if name.contains("kind") || name.contains("type") || name.contains("category") {
        &[
            "standard", "custom", "system", "partner", "internal", "external",
        ]
    } else if name.contains("level") || name.contains("tier") || name.contains("band") {
        &["small", "medium", "large", "enterprise"]
    } else if name.contains("purpose") {
        &[
            "marketing_email",
            "sales_call",
            "events",
            "product_updates",
            "research",
        ]
    } else if name.contains("direction") {
        &["above", "below"]
    } else if name.contains("source") || name.contains("origin") {
        &["web", "referral", "event", "partner", "campaign", "import"]
    } else if name.contains("method") || name.contains("mode") {
        &["manual", "scheduled", "triggered", "api"]
    } else {
        &["alpha", "beta", "gamma", "delta", "epsilon", "zeta"]
    }
}

/// One person of the shared population: the same people appear as authors, customers, employees
/// and respondents in several modules, so a join on an email address finds them.
#[derive(Clone, Debug)]
pub struct Person {
    pub first: String,
    pub last: String,
    pub email: String,
    pub phone: String,
}

impl Person {
    pub fn full(&self) -> String {
        format!("{} {}", self.first, self.last)
    }
}

/// A population of `n` people with distinct email addresses.
pub fn people(rng: &mut Rng, n: usize) -> Vec<Person> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let first = *rng.pick(FIRST_NAMES);
        let last = *rng.pick(LAST_NAMES);
        // The index in the local part keeps every address distinct without a lookup.
        let email = format!(
            "{}.{}.{}@{}",
            first.to_lowercase(),
            last.to_lowercase(),
            i,
            rng.pick(DOMAINS)
        );
        let phone = format!(
            "+{}-{:03}-{:04}",
            rng.range(1, 99),
            rng.range(100, 999),
            rng.range(0, 9999)
        );
        out.push(Person {
            first: first.to_string(),
            last: last.to_string(),
            email,
            phone,
        });
    }
    out
}

/// A phrase of `n` words from the vocabulary.
pub fn phrase(rng: &mut Rng, n: usize) -> String {
    let mut out = String::new();
    for i in 0..n {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(rng.pick(WORDS));
    }
    out
}

/// A title: a short phrase with its first letter capitalised.
pub fn title(rng: &mut Rng) -> String {
    let n = rng.range(2, 5) as usize;
    let mut s = phrase(rng, n);
    if let Some(first) = s.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    s
}

/// A paragraph of about `bytes` bytes.
pub fn paragraph(rng: &mut Rng, bytes: usize) -> String {
    let mut out = String::with_capacity(bytes + 16);
    let mut sentence_len = 0;
    while out.len() < bytes {
        let word = if rng.chance(0.55) {
            *rng.pick(FILLER)
        } else {
            *rng.pick(WORDS)
        };
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        sentence_len += 1;
        if sentence_len >= 12 && rng.chance(0.2) {
            out.push('.');
            sentence_len = 0;
        }
    }
    out.truncate(bytes.max(1));
    out
}

/// Lower-case hex digits, `n` of them.
pub fn hex(rng: &mut Rng, n: usize) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    (0..n)
        .map(|_| DIGITS[rng.below(16) as usize] as char)
        .collect()
}
