//! Recognises clips that are a phone number, an email address or a postal
//! address. Precision over recall: only short clips count, so a number
//! inside a long mail body does not tag the whole mail.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

pub(crate) const MAX_CHARS: usize = 400;
const MAX_LINES: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Contact {
    Phone,
    Email,
    Address,
}

impl Contact {
    pub fn as_str(self) -> &'static str {
        match self {
            Contact::Phone => "phone",
            Contact::Email => "email",
            Contact::Address => "address",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "phone" => Contact::Phone,
            "email" => Contact::Email,
            "address" => Contact::Address,
            _ => return None,
        })
    }
}

/// The stored form: `"phone,address"`, or `None` when there is nothing.
pub(crate) fn join(found: &[Contact]) -> Option<String> {
    (!found.is_empty()).then(|| {
        found
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>()
            .join(",")
    })
}

pub(crate) fn split(stored: Option<&str>) -> Vec<Contact> {
    stored
        .unwrap_or_default()
        .split(',')
        .filter_map(Contact::parse)
        .collect()
}

struct Patterns {
    run: Regex,
    date: Regex,
    us: Regex,
    national: Regex,
    international: Regex,
    email: Regex,
    address: Vec<Regex>,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| {
        let re = |s: &str| Regex::new(s).expect("contact pattern compiles");
        // Capitalised, so `string 12, 1234 items` is not a street.
        let street_de = r"(?:[A-ZÄÖÜ][^\W\d_]*(?:strasse|straße|str\.|weg|gasse|platz|allee|ring)|Weg|Gasse|Platz|Allee|Ring)";
        let street_en = r"\d{1,5}[A-Za-z]?(?: [A-Z][a-z]+)+ (?:St|Street|Rd|Road|Ave|Avenue|Blvd|Boulevard|Dr|Drive|Ln|Lane|Way|Ct|Court|Pl|Place|Close|Crescent|Terrace|Square|Gardens)\b\.?";
        Patterns {
            // Digits with single separators; a phone number is one such run.
            run: re(r"[+(]?\d(?: ?[\d()\-./])*"),
            date: re(r"(?:^|\D)(?:\d{1,2}[./\-]\d{1,2}[./\-]\d{2,4}|\d{4}[./\-]\d{1,2}[./\-]\d{1,2})(?:\D|$)"),
            us: re(r"^(?:\([2-9]\d{2}\) ?|[2-9]\d{2}-)\d{3}-\d{4}$"),
            national: re(r"^(?:0\d{1,4}|\(0\d{1,4}\))(?:[ /\-]\d{2,8}){1,4}$"),
            international: re(r"^\+\d[\d \-()]*\d$"),
            email: re(r"\b[A-Za-z0-9._%+\-]+@[A-Za-z0-9\-]+(?:\.[A-Za-z0-9\-]+)*\.[A-Za-z]{2,}\b"),
            address: vec![
                re(&format!(
                    r"\b{street_de} ?\d{{1,4}} ?[a-z]?(?:\s*,\s*|\s+)(?:[A-Z]{{1,2}}-)?\d{{4,5}} +[^\W\d_]{{2,}}"
                )),
                re(&format!(r"\b{street_en}(?s:.{{0,80}}?)\b[A-Z]{{2}} \d{{5}}(?:-\d{{4}})?\b")),
                re(&format!(
                    r"\b{street_en}(?s:.{{0,80}}?)\b[A-Z]{{1,2}}\d[A-Z\d]? \d[A-Z]{{2}}\b"
                )),
            ],
        }
    })
}

/// Contact kinds in the clip, in `phone, email, address` order.
pub fn detect(text: &str) -> Vec<Contact> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > MAX_CHARS || text.lines().count() > MAX_LINES {
        return Vec::new();
    }
    let p = patterns();
    let mut found = Vec::new();
    if p.run.find_iter(text).any(|m| is_phone(text, m.range())) {
        found.push(Contact::Phone);
    }
    if p.email.find_iter(text).any(|m| {
        let before = &text[..m.start()];
        let after = text[m.end()..].chars().next();
        !matches!(after, Some(':' | '/' | '@'))
            && (before.ends_with("mailto:") || !before.ends_with([':', '/', '@', '=', '?', '&']))
    }) {
        found.push(Contact::Email);
    }
    if p.address.iter().any(|re| re.is_match(text)) {
        found.push(Contact::Address);
    }
    found
}

fn is_phone(text: &str, range: std::ops::Range<usize>) -> bool {
    let p = patterns();
    let before = text[..range.start].chars().next_back();
    if before.is_some_and(|c| c.is_alphanumeric() || "._-/@#$€£=+&".contains(c)) {
        return false;
    }
    let run = text[range.clone()].trim_end_matches(['-', '.', '/', '(', ' ']);
    let after = text[range.start + run.len()..].chars().next();
    if after.is_some_and(|c| c.is_alphanumeric() || "_@%€$£".contains(c)) {
        return false;
    }
    let digits = run.chars().filter(char::is_ascii_digit).count();
    // Dots belong to dates, versions, IPs and prices, not to phone numbers here.
    if run.contains('.')
        || p.date.is_match(run)
        || run.matches('(').count() != run.matches(')').count()
    {
        return false;
    }
    p.us.is_match(run)
        || (p.national.is_match(run) && (9..=12).contains(&digits))
        || (p.international.is_match(run) && (8..=15).contains(&digits))
}

#[cfg(test)]
mod tests {
    use super::*;
    use Contact::*;

    #[test]
    fn detects_contacts_and_rejects_lookalikes() {
        let cases: &[(&str, &[Contact])] = &[
            ("+41 79 123 45 67", &[Phone]),
            ("+41791234567", &[Phone]),
            ("Tel: +49 (0)30 1234567", &[Phone]),
            ("079 123 45 67", &[Phone]),
            ("044 123 45 67", &[Phone]),
            ("030 1234567", &[Phone]),
            ("030/1234567", &[Phone]),
            ("0151 23456789", &[Phone]),
            ("(415) 555-0123", &[Phone]),
            ("+1 415-555-0123", &[Phone]),
            ("anna.muster@example.ch", &[Email]),
            ("Mail me: <bob+tag@mail.example.co.uk>.", &[Email]),
            ("Bahnhofstrasse 12, 8001 Zürich", &[Address]),
            ("Musterstr. 5a\n10115 Berlin", &[Address]),
            ("Am Ring 3\nA-1010 Wien", &[Address]),
            ("123 Main St, Springfield, IL 62704", &[Address]),
            ("10 Downing Street\nLondon SW1A 2AA", &[Address]),
            (
                "Anna Muster\nLindenweg 4\n3000 Bern\n+41 31 123 45 67\nanna@example.ch",
                &[Phone, Email, Address],
            ),
            // Dates, times, versions, IPs, IBANs, cards, timestamps, IDs, prices.
            ("09.11.2026", &[]),
            ("09/11/2026 14:30", &[]),
            ("2026-11-18", &[]),
            ("v0.12.3 and 1.2.3", &[]),
            ("192.0.2.1", &[]),
            ("010.000.000.001", &[]),
            ("CH93 0076 2011 6238 5295 7", &[]),
            ("DE89 3704 0044 0532 0130 00", &[]),
            ("4111 1111 1111 1111", &[]),
            ("0000 1111 2222 3333", &[]),
            ("1731916800", &[]),
            ("0123456789", &[]),
            ("order 4711 0815", &[]),
            ("CHF 1 234 567.50", &[]),
            ("$1,299.00", &[]),
            ("https://user:pass@example.com/x", &[]),
            ("git@github.com:flix-wg/cliprust.git", &[]),
            ("https://host/?user=alice@example.com", &[]),
            ("let s = \"string 12, 1234 items\";", &[]),
            ("Meet at 12 Main St tomorrow", &[]),
        ];
        for (text, want) in cases {
            assert_eq!(detect(text), *want, "{text:?}");
        }
        let long = format!("{}\nCall 079 123 45 67", "lorem ipsum ".repeat(40));
        assert_eq!(detect(&long), []);
        assert_eq!(split(join(&[Phone, Address]).as_deref()), [Phone, Address]);
    }
}
