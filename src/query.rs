//! The search grammar shared by the app and the CLI.
//!
//! - Plain words must each occur in the text, title or note.
//! - `from:mail` or `@mail` keeps items copied from the best stored app whose
//!   name, a word of it or a bundle-id component starts with "mail". Only the
//!   first six words are looked at; a word that names no stored app stays text.
//! - `is:phone`, `is:email`, `is:address`, or `is:contact` for any of them,
//!   keeps clips recognised as that contact type. The last one wins.
//! - `site:github.com` or `source:github.com` keeps clips copied from that
//!   site or a page under it (`site:docs.github.com`); a word without a dot
//!   (`site:git`) matches every domain that starts with it. The last one wins.

use crate::store::{AppCount, Find, Store};
use crate::{Contact, Result};

/// Words after this many are never read as an app.
const APP_WORDS: usize = 6;

/// One whitespace-separated word of a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Term<'a> {
    Text(&'a str),
    /// The app name after `from:` or `@`.
    App(&'a str),
    /// `None` for `is:contact`.
    Contact(Option<Contact>),
    /// The domain after `site:` or `source:`.
    Site(&'a str),
}

/// Each word with what it means; nothing is looked up.
pub fn parse(query: &str) -> Vec<(&str, Term<'_>)> {
    query.split_whitespace().map(|w| (w, term(w))).collect()
}

fn term(word: &str) -> Term<'_> {
    let lower = word.to_ascii_lowercase();
    if let Some(prefix) = ["site:", "source:"].iter().find(|p| lower.starts_with(*p))
        && word.len() > prefix.len()
    {
        return Term::Site(&word[prefix.len()..]);
    }
    let app = match word.get(..5) {
        Some(p) if p.eq_ignore_ascii_case("from:") => Some(&word[5..]),
        _ => word.strip_prefix('@'),
    };
    if let Some(name) = app.filter(|n| !n.is_empty()) {
        return Term::App(name);
    }
    match word.get(..3) {
        Some(p) if p.eq_ignore_ascii_case("is:") => match word[3..].to_ascii_lowercase().as_str() {
            "contact" => Term::Contact(None),
            name => Contact::parse(name).map_or(Term::Text(word), |c| Term::Contact(Some(c))),
        },
        _ => Term::Text(word),
    }
}

/// `phone`, `email`, `address`, or `any` / `contact` for any of them.
pub fn contact_named(name: &str) -> Option<Option<Contact>> {
    match name {
        "any" | "contact" => Some(None),
        _ => Contact::parse(name).map(Some),
    }
}

/// The wire name of a contact filter: `phone`, `email`, `address` or `any`.
pub fn contact_name(contact: Option<Contact>) -> &'static str {
    contact.map_or("any", Contact::as_str)
}

/// A query with its app looked up in the store.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Search {
    /// The plain words, including `from:` words that named no stored app.
    pub text: String,
    pub app: Option<AppCount>,
    /// The word that named `app`.
    pub app_term: Option<String>,
    /// From the last `is:` word; `Some(None)` is any contact.
    pub contact: Option<Option<Contact>>,
    /// Every `is:` word; none of them is searched as text.
    pub contact_terms: Vec<String>,
    /// From the last `site:` word, lowercased.
    pub site: Option<String>,
}

impl Search {
    /// With `app_given` the caller already narrowed to an app, so `from:`
    /// words stay text.
    pub fn resolve(store: &Store, query: &str, app_given: bool) -> Result<Self> {
        let mut s = Search::default();
        let mut words = Vec::new();
        for (word, term) in parse(query) {
            match term {
                Term::Contact(c) => {
                    s.contact = Some(c);
                    s.contact_terms.push(word.to_owned());
                }
                Term::Site(site) => s.site = Some(site.to_lowercase()),
                _ => words.push((word, term)),
            }
        }
        if !app_given {
            for (word, term) in words.iter().take(APP_WORDS) {
                if let Term::App(name) = term
                    && let Some(hit) = store.apps(Some(name))?.into_iter().next()
                {
                    s.app = Some(hit);
                    s.app_term = Some((*word).to_owned());
                    break;
                }
            }
        }
        let text: Vec<&str> = words
            .into_iter()
            .map(|(w, _)| w)
            .filter(|w| s.app_term.as_deref() != Some(*w))
            .collect();
        s.text = text.join(" ");
        Ok(s)
    }

    /// The text, app and contact as a filter; the caller adds the rest.
    pub fn find(&self) -> Find<'_> {
        Find {
            text: Some(&self.text),
            app: self.app.as_ref().map(|a| a.bundle_id.as_str()),
            contact: self.contact,
            site: self.site.as_deref(),
            ..Find::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppInfo, Capture};

    #[test]
    fn queries_parse_and_resolve() {
        use Term::*;
        let cases: &[(&str, &[Term])] = &[
            ("rust  code", &[Text("rust"), Text("code")]),
            ("FROM:Mail @code", &[App("Mail"), App("code")]),
            (
                "from: @ is: is:any",
                &[Text("from:"), Text("@"), Text("is:"), Text("is:any")],
            ),
            (
                "IS:Phone is:email is:address is:contact is:fax",
                &[
                    Contact(Some(crate::Contact::Phone)),
                    Contact(Some(crate::Contact::Email)),
                    Contact(Some(crate::Contact::Address)),
                    Contact(None),
                    Text("is:fax"),
                ],
            ),
            (
                "site:GitHub SOURCE:x.com site:",
                &[Site("GitHub"), Site("x.com"), Text("site:")],
            ),
        ];
        for (query, terms) in cases {
            let parsed: Vec<Term> = parse(query).into_iter().map(|(_, t)| t).collect();
            assert_eq!(&parsed, terms, "{query}");
        }

        let mut store = Store::open_in_memory().unwrap();
        let mut cap = Capture::from_text("+41 79 123 45 67".into());
        cap.app = Some(AppInfo {
            bundle_id: "com.apple.mail".into(),
            name: Some("Mail".into()),
            path: None,
        });
        store.ingest(&cap, 1).unwrap();
        for (n, url) in ["https://docs.github.com/en", "https://github.company"]
            .iter()
            .enumerate()
        {
            let mut page = Capture::from_text(format!("page {n}"));
            page.source_url = Some(url.to_string());
            store.ingest(&page, 2 + n as i64).unwrap();
        }
        for (query, hits) in [
            ("site:git", 2),
            ("site:gitlab.com source:docs.GitHub.com", 1),
            ("site:github.com", 1),
            ("site:gitlab.com", 0),
        ] {
            let s = Search::resolve(&store, query, false).unwrap();
            assert_eq!(store.find(&s.find(), 10, 0).unwrap().len(), hits, "{query}");
        }
        let s = Search::resolve(&store, "@nope is:email 79 from:mai is:phone", false).unwrap();
        assert_eq!(
            (s.text.as_str(), s.app_term.as_deref(), s.contact),
            (
                "@nope 79",
                Some("from:mai"),
                Some(Some(crate::Contact::Phone))
            )
        );
        let found = Search::resolve(&store, "79 from:mai is:contact", false).unwrap();
        assert_eq!(store.find(&found.find(), 10, 0).unwrap().len(), 1);
        let given = Search::resolve(&store, "from:mai", true).unwrap();
        assert_eq!((given.text.as_str(), given.app), ("from:mai", None));
    }
}
