//! Smart-list filters, stored as JSON in `paste_lists.filter_rule_data` and
//! compiled to an SQL predicate over `paste_items p` joined to `applications a`.

use rusqlite::types::Value;
use serde::{Deserialize, Serialize};

use crate::model::Kind;

/// Case-insensitive (ASCII) substring test on text and title; binds the
/// needle twice.
pub(crate) const CONTAINS_SQL: &str = "(instr(lower(coalesce(p.text, '')), lower(?)) > 0 \
     OR instr(lower(coalesce(p.title, '')), lower(?)) > 0)";

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
                params.push(text.clone().into());
                params.push(text.clone().into());
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
            Rule::Secret => "p.secret_provider IS NOT NULL".into(),
        }
    }
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
