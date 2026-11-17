//! Recognises API keys and tokens on the clipboard and names them from context.

use std::ops::Range;
use std::sync::OnceLock;

use regex::Regex;

use crate::model::{Capture, Kind, Secret};

/// Clips longer than this are documents or logs, not a copied key.
const MAX_CLIP_BYTES: usize = 8 * 1024;

/// First match wins, so more specific prefixes come first.
const PATTERNS: &[(&str, &str)] = &[
    ("Anthropic API key", r"\bsk-ant-[A-Za-z0-9_-]{32,}"),
    (
        "OpenAI API key",
        r"\bsk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_-]{32,}",
    ),
    (
        "GitHub token",
        r"\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{60,})",
    ),
    ("GitLab token", r"\bglpat-[A-Za-z0-9_-]{20,}"),
    ("AWS access key", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
    (
        "Stripe key",
        r"\b(?:sk|rk|pk)_(?:live|test)_[A-Za-z0-9]{20,}",
    ),
    ("Slack token", r"\bxox[abposr]-[A-Za-z0-9-]{10,}"),
    ("Google API key", r"\bAIza[0-9A-Za-z_-]{35}"),
    ("Hugging Face token", r"\bhf_[A-Za-z0-9]{30,}"),
    ("Groq API key", r"\bgsk_[A-Za-z0-9]{40,}"),
    ("npm token", r"\bnpm_[A-Za-z0-9]{36}"),
    (
        "SendGrid API key",
        r"\bSG\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43}",
    ),
    ("Resend API key", r"\bre_[A-Za-z0-9_]{24,}"),
    (
        "JSON Web Token",
        r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
    ),
    ("Private key", r"-----BEGIN (?:[A-Z]+ )*PRIVATE KEY-----"),
];

/// Words in a page URL or title that make an unrecognised random token a key.
const CONTEXT_HINTS: &[&str] = &["key", "token", "secret", "credential", "api", "auth"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    /// `None` for a random-looking token that needs a context hint to count.
    pub provider: Option<&'static str>,
    pub range: Range<usize>,
}

/// Context of the page a clip came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageContext {
    pub url: String,
    pub title: Option<String>,
}

fn patterns() -> &'static [(&'static str, Regex)] {
    static COMPILED: OnceLock<Vec<(&str, Regex)>> = OnceLock::new();
    COMPILED.get_or_init(|| {
        PATTERNS
            .iter()
            .map(|(name, re)| (*name, Regex::new(re).expect("secret pattern compiles")))
            .collect()
    })
}

pub fn detect(text: &str) -> Option<Detection> {
    if text.len() > MAX_CLIP_BYTES {
        return None;
    }
    for (provider, re) in patterns() {
        if let Some(m) = re.find(text)
            && (provider == &"Private key" || looks_random(m.as_str()))
        {
            return Some(Detection {
                provider: Some(provider),
                range: m.range(),
            });
        }
    }
    let token = text.trim();
    let start = text.len() - text.trim_start().len();
    let candidate = (20..=256).contains(&token.len())
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.=+/:".contains(c))
        && !token.contains("://")
        && looks_random(token)
        && entropy(token) >= 3.5;
    candidate.then(|| Detection {
        provider: None,
        range: start..start + token.len(),
    })
}

/// Marks `cap` as a secret when its text holds a key. `page` is only called
/// once something looks like a key, because asking a browser is slow.
pub fn annotate(cap: &mut Capture, page: impl FnOnce() -> Option<PageContext>) {
    if !matches!(cap.kind, Kind::Text | Kind::Url) {
        return;
    }
    let Some(text) = cap.text.as_deref() else {
        return;
    };
    let Some(found) = detect(text) else { return };
    if cap.source_url.is_none()
        && let Some(page) = page()
    {
        cap.source_url = Some(page.url);
        cap.source_title = cap.source_title.take().or(page.title);
    }
    let provider = match found.provider {
        Some(provider) => provider,
        None if has_hint(cap.source_url.as_deref()) || has_hint(cap.source_title.as_deref()) => {
            "API key"
        }
        None => return,
    };
    let site = cap
        .source_url
        .as_deref()
        .and_then(host)
        .or(cap.source_title.as_deref())
        .or(cap.app.as_ref().and_then(|a| a.name.as_deref()));
    let name = assigned_name(&text[..found.range.start]).map_or_else(
        || match site {
            Some(site) => format!("{provider} · {site}"),
            None => provider.to_owned(),
        },
        str::to_owned,
    );
    let masked = format!(
        "{}{}{}",
        &text[..found.range.start],
        mask(&text[found.range.clone()]),
        &text[found.range.end..]
    );
    cap.secret = Some(Secret {
        provider: provider.to_owned(),
        name,
        masked,
    });
}

pub fn mask(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() < 16 {
        return "••••".to_owned();
    }
    let head: String = chars[..6].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

/// `OPENAI_API_KEY=`, `export TOKEN="` and `api_key: ` style prefixes.
fn assigned_name(prefix: &str) -> Option<&str> {
    let line = prefix.rsplit('\n').next()?;
    let line = line.trim_end().trim_end_matches(['"', '\'']).trim_end();
    let line = line
        .strip_suffix('=')
        .or_else(|| line.strip_suffix(':'))?
        .trim_end();
    let line = line.trim_start();
    let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
    let name = line.trim_matches(['"', '\'']);
    (!name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'))
    .then_some(name)
}

fn has_hint(s: Option<&str>) -> bool {
    s.is_some_and(|s| {
        let s = s.to_ascii_lowercase();
        CONTEXT_HINTS.iter().any(|hint| s.contains(hint))
    })
}

pub fn host(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    (!host.is_empty()).then_some(host)
}

fn looks_random(token: &str) -> bool {
    token.chars().any(|c| c.is_ascii_digit()) && token.chars().any(|c| c.is_ascii_alphabetic())
}

fn entropy(s: &str) -> f64 {
    let mut counts = [0u32; 256];
    for b in s.bytes() {
        counts[b as usize] += 1;
    }
    let len = s.len() as f64;
    counts
        .iter()
        .filter(|&&n| n > 0)
        .map(|&n| {
            let p = f64::from(n) / len;
            -p * p.log2()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AppInfo;

    fn annotated(text: &str, url: Option<&str>, page: Option<PageContext>) -> Option<Secret> {
        let mut cap = Capture::from_text(text.to_owned());
        cap.source_url = url.map(str::to_owned);
        cap.app = Some(AppInfo {
            bundle_id: "com.google.Chrome".into(),
            name: Some("Chrome".into()),
            path: None,
        });
        annotate(&mut cap, || page);
        cap.secret
    }

    #[test]
    fn known_keys_are_named_from_assignment_or_site() {
        let key = "sk-proj-Ab3dEf6hIj9kLm2nOp5qRs8tUv1wXy4zAb7c";
        let s = annotated(key, Some("https://platform.openai.com/api-keys"), None).unwrap();
        assert_eq!(s.provider, "OpenAI API key");
        assert_eq!(s.name, "OpenAI API key · platform.openai.com");
        assert_eq!(s.masked, "sk-pro…Ab7c");

        let s = annotated(&format!("export OPENAI_API_KEY=\"{key}\""), None, None).unwrap();
        assert_eq!(s.name, "OPENAI_API_KEY");
        assert_eq!(s.masked, "export OPENAI_API_KEY=\"sk-pro…Ab7c\"");

        let anthropic = annotated(
            "sk-ant-api03-Zx9Yw8Vu7Ts6Rq5Po4Nm3Lk2Ji1Hg0FeDcBa",
            None,
            None,
        )
        .unwrap();
        assert_eq!(anthropic.provider, "Anthropic API key");
        assert_eq!(anthropic.name, "Anthropic API key · Chrome");
    }

    #[test]
    fn random_tokens_need_a_page_that_talks_about_keys() {
        let token = "q8Zr2LmN0vX4tY7pW1sK9dF3gH6jB5cA";
        let page = |url: &str, title: &str| {
            Some(PageContext {
                url: url.into(),
                title: Some(title.into()),
            })
        };
        let s = annotated(
            token,
            None,
            page(
                "https://dash.cloudflare.com/profile/api-tokens",
                "API Tokens",
            ),
        )
        .unwrap();
        assert_eq!(s.provider, "API key");
        assert_eq!(s.name, "API key · dash.cloudflare.com");
        assert_eq!(
            annotated(
                token,
                None,
                page("https://github.com/x/y/commit/1", "Commit")
            ),
            None
        );
        assert_eq!(
            annotated("just a normal sentence with words", None, None),
            None
        );
        assert_eq!(
            annotated("re_compile_pattern_for_parser_module", None, None),
            None
        );
    }
}
