//! Smart-list filters, stored as JSON in `paste_lists.filter_rule_data` and
//! compiled to an SQL predicate over `paste_items p` joined to `applications a`.

use rusqlite::types::Value;
use serde::{Deserialize, Serialize};

use crate::contacts::Contact;
use crate::model::Kind;

/// Case-insensitive (ASCII) substring test on text, title and note; binds
/// the needle three times.
pub(crate) const CONTAINS_SQL: &str = "(instr(lower(coalesce(CASE WHEN p.secret_kind = 'key' THEN NULL ELSE p.text END, '')), lower(?)) > 0 \
     OR instr(lower(coalesce(p.title, '')), lower(?)) > 0 \
     OR instr(lower(coalesce(p.note, '')), lower(?)) > 0)";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Rule {
    All {
        rules: Vec<Rule>,
    },
    Any {
        rules: Vec<Rule>,
    },
    Not {
        rule: Box<Rule>,
    },
    Kind {
        kind: Kind,
    },
    App {
        bundle_id: String,
    },
    Contains {
        text: String,
    },
    CopiedWithin {
        seconds: i64,
    },
    LargerThan {
        bytes: i64,
    },
    /// First copied since local midnight.
    CreatedToday,
    Secret,
    /// Any contact when `contact` is absent.
    Contact {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        contact: Option<Contact>,
    },
    /// Copied from a web page on this site; see [`site_sql`].
    Site {
        domain: String,
    },
}

impl Rule {
    pub(crate) fn to_sql(&self, now: i64, params: &mut Vec<Value>) -> String {
        match self {
            Rule::All { rules } => join(rules, " AND ", "1", now, params),
            Rule::Any { rules } => join(rules, " OR ", "0", now, params),
            Rule::Not { rule } => format!("NOT ({})", rule.to_sql(now, params)),
            Rule::Kind { kind } => {
                params.push(kind.as_str().to_owned().into());
                "p.kind = ?".into()
            }
            Rule::App { bundle_id } => {
                params.push(bundle_id.clone().into());
                "coalesce(a.bundle_id, '') = ?".into()
            }
            Rule::Contains { text } => {
                params.extend(std::iter::repeat_n(Value::from(text.clone()), 3));
                CONTAINS_SQL.into()
            }
            Rule::CopiedWithin { seconds } => {
                params.push(now.saturating_sub(seconds.saturating_mul(1000)).into());
                "p.last_used_at >= ?".into()
            }
            Rule::LargerThan { bytes } => {
                params.push((*bytes).into());
                "p.byte_size > ?".into()
            }
            Rule::CreatedToday => {
                params.push(now.into());
                params.push(now.into());
                "p.created_at >= unixepoch(? / 1000, 'unixepoch', 'localtime', 'start of day', 'utc') * 1000 \
                 AND p.created_at < unixepoch(? / 1000, 'unixepoch', 'localtime', 'start of day', '+1 day', 'utc') * 1000"
                    .into()
            }
            Rule::Secret => "p.secret_kind = 'key'".into(),
            Rule::Contact { contact } => contact_sql(*contact, params),
            Rule::Site { domain } => site_sql(domain, params),
        }
    }
}

/// Items tagged with `contact`, or with any contact for `None`.
pub(crate) fn contact_sql(contact: Option<Contact>, params: &mut Vec<Value>) -> String {
    match contact {
        Some(c) => {
            params.push(c.as_str().to_owned().into());
            "instr(',' || p.contact || ',', ',' || ? || ',') > 0".into()
        }
        None => "p.contact IS NOT NULL".into(),
    }
}

/// Items copied from `site`: a page on that host or a subdomain of it, or
/// for a word without a dot (`github`) any domain that starts with it.
pub(crate) fn site_sql(site: &str, params: &mut Vec<Value>) -> String {
    let site = site.to_lowercase();
    if site.contains('.') {
        let site = site.strip_prefix("www.").unwrap_or(&site).to_owned();
        params.push(site.clone().into());
        params.push(site.into());
        // Clips without a URL only know the registrable domain.
        return "(on_site(p.source_url, ?) OR (p.source_url IS NULL AND p.source_domain IS ?))"
            .into();
    }
    let escaped = site
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    params.push(format!("{escaped}%").into());
    "coalesce(p.source_domain, '') LIKE ? ESCAPE '\\'".into()
}

/// Registers `on_site(url, site)`: whether the URL's host is `site` or a
/// subdomain of it. The stored domain is only the registrable one, so
/// `docs.github.com` has to be told from `gist.github.com` by the URL.
pub(crate) fn register_functions(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    use rusqlite::functions::FunctionFlags;
    conn.create_scalar_function(
        "on_site",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let (url, site): (Option<String>, String) = (ctx.get(0)?, ctx.get(1)?);
            let Some(host) = url.as_deref().and_then(crate::secrets::host) else {
                return Ok(false);
            };
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            Ok(host == site
                || host
                    .strip_suffix(site.as_str())
                    .is_some_and(|rest| rest.ends_with('.')))
        },
    )
}

fn join(rules: &[Rule], sep: &str, empty: &str, now: i64, params: &mut Vec<Value>) -> String {
    if rules.is_empty() {
        return empty.into();
    }
    let parts: Vec<String> = rules.iter().map(|r| r.to_sql(now, params)).collect();
    format!("({})", parts.join(sep))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortType {
    #[default]
    Recent,
    Oldest,
}

impl SortType {
    pub fn as_str(self) -> &'static str {
        match self {
            SortType::Recent => "recent",
            SortType::Oldest => "oldest",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "recent" => Some(SortType::Recent),
            "oldest" => Some(SortType::Oldest),
            _ => None,
        }
    }

    pub(crate) fn order_sql(self) -> &'static str {
        match self {
            SortType::Recent => "ORDER BY p.last_used_at DESC, p.id DESC",
            SortType::Oldest => "ORDER BY p.created_at ASC, p.id ASC",
        }
    }
}
