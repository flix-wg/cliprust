//! Recognises API keys and tokens on the clipboard, and IDs copied from web
//! pages, and names them from context.

use std::ops::Range;
use std::sync::OnceLock;

use regex::Regex;

use crate::model::{Capture, Kind, Secret, SecretKind, text_of};

/// Clips longer than this are documents or logs, not a copied key.
const MAX_CLIP_BYTES: usize = 8 * 1024;

const PRIVATE_KEY: &str = "Private key";

/// Earlier patterns name the clip when several match. ASCII word boundaries
/// keep the regexes on the small, fast engine.
const PATTERNS: &[(&str, &str)] = &[
    (
        PRIVATE_KEY,
        r"-----BEGIN (?:[A-Z]+ )*PRIVATE KEY-----(?s:.*?)(?:-----END (?:[A-Z]+ )*PRIVATE KEY-----|\z)",
    ),
    (
        "Anthropic admin key",
        r"(?-u:\b)sk-ant-admin[0-9]*-[A-Za-z0-9_-]{32,}",
    ),
    ("Anthropic API key", r"(?-u:\b)sk-ant-[A-Za-z0-9_-]{32,}"),
    ("OpenRouter API key", r"(?-u:\b)sk-or-v1-[A-Za-z0-9]{32,}"),
    (
        "OpenAI API key",
        r"(?-u:\b)sk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_-]{32,}",
    ),
    (
        "GitHub token",
        r"(?-u:\b)(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{60,})",
    ),
    ("GitLab token", r"(?-u:\b)glpat-[A-Za-z0-9_-]{20,}"),
    (
        "AWS access key",
        r"(?-u:\b)(?:AKIA|ASIA)[0-9A-Z]{16}(?-u:\b)",
    ),
    (
        "Stripe key",
        r"(?-u:\b)(?:sk|rk|pk)_(?:live|test)_[A-Za-z0-9]{20,}",
    ),
    ("Slack token", r"(?-u:\b)xox[abposr]-[A-Za-z0-9-]{10,}"),
    ("Google API key", r"(?-u:\b)AIza[0-9A-Za-z_-]{35}"),
    ("Hugging Face token", r"(?-u:\b)hf_[A-Za-z0-9]{30,}"),
    ("Groq API key", r"(?-u:\b)gsk_[A-Za-z0-9]{40,}"),
    ("npm token", r"(?-u:\b)npm_[A-Za-z0-9]{36}"),
    (
        "SendGrid API key",
        r"(?-u:\b)SG\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43}",
    ),
    ("Resend API key", r"(?-u:\b)re_[A-Za-z0-9_]{24,}"),
    (
        "JSON Web Token",
        r"(?-u:\b)eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
    ),
    (
        "Cloudflare API token",
        r"(?-u:\b)cf(?:ut|at)_[A-Za-z0-9]{40,}",
    ),
    ("Cloudflare Global API key", r"(?-u:\b)cfk_[A-Za-z0-9]{40,}"),
    (
        "Cloudflare Origin CA key",
        r"(?-u:\b)v1\.0-[0-9A-Fa-f-]{40,}",
    ),
    (
        "Supabase key",
        r"(?-u:\b)sb_(?:secret|publishable)_[A-Za-z0-9_-]{20,}",
    ),
    ("DigitalOcean token", r"(?-u:\b)do[por]_v1_[A-Za-z0-9]{32,}"),
    (
        "Sentry token",
        r"(?-u:\b)(?:sntryu_[a-f0-9]{64}|sntrys_[A-Za-z0-9+/=]+_[A-Za-z0-9+/]{43})",
    ),
    ("Notion token", r"(?-u:\b)ntn_[A-Za-z0-9]{40,}"),
    ("Shopify token", r"(?-u:\b)shp(?:at|ca|pa)_[A-Fa-f0-9]{32,}"),
    ("PyPI token", r"(?-u:\b)pypi-[A-Za-z0-9_-]{85,}"),
    (
        "Tailscale key",
        r"(?-u:\b)tskey-(?:api|auth|client|scim|webhook)-[A-Za-z0-9-]{16,}",
    ),
    ("Vercel token", r"(?-u:\b)vc[piark]_[A-Za-z0-9]{24,}"),
    ("Replicate API token", r"(?-u:\b)r8_[A-Za-z0-9]{37}(?-u:\b)"),
    (
        "Doppler token",
        r"(?-u:\b)dp\.(?:ct|pt|st|sa|said|scim|audit)\.(?:[a-z0-9_-]{2,35}\.)?[A-Za-z0-9]{40,44}",
    ),
];

/// Public suffixes with two labels, so `bbc.co.uk` is a site and `co.uk` is not.
const TWO_PART_SUFFIXES: &[&str] = &[
    "co.uk", "org.uk", "ac.uk", "gov.uk", "com.au", "net.au", "org.au", "co.nz", "co.jp", "co.kr",
    "co.in", "co.za", "com.br", "com.cn", "com.hk", "com.mx", "com.sg", "com.tr", "com.tw",
];

/// Site names that capitalising the domain label would get wrong.
const SITE_NAMES: &[(&str, &str)] = &[
    ("digitalocean", "DigitalOcean"),
    ("github", "GitHub"),
    ("gitlab", "GitLab"),
    ("huggingface", "Hugging Face"),
    ("npmjs", "npm"),
    ("openai", "OpenAI"),
    ("openrouter", "OpenRouter"),
    ("pypi", "PyPI"),
    ("sendgrid", "SendGrid"),
];

/// Words in a page URL or title that make an unrecognised random token a key.
const CONTEXT_HINTS: &[&str] = &["key", "token", "secret", "credential", "api", "auth"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    /// `None` for a random-looking token that needs a context hint to count.
    pub provider: Option<&'static str>,
    /// Every secret in the clip, sorted and non-overlapping.
    pub ranges: Vec<Range<usize>>,
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
    let mut provider = None;
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (name, re) in patterns() {
        for m in re.find_iter(text) {
            let overlaps = ranges
                .iter()
                .any(|r| r.start < m.end() && m.start() < r.end);
            if overlaps || (*name != PRIVATE_KEY && !looks_random(m.as_str())) {
                continue;
            }
            provider.get_or_insert(*name);
            ranges.push(m.range());
        }
    }
    if !ranges.is_empty() {
        ranges.sort_by_key(|r| r.start);
        return Some(Detection { provider, ranges });
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
    // One span here, but `ranges` lists every key's span in the clip.
    #[allow(clippy::single_range_in_vec_init)]
    let ranges = vec![start..start + token.len()];
    candidate.then_some(Detection {
        provider: None,
        ranges,
    })
}

/// Marks `cap` as a secret when its text holds a key, or as an ID when it is
/// a single identifier copied from a known page. `page` is only called once
/// something looks like a key or ID, because asking a browser is slow.
pub fn annotate(cap: &mut Capture, page: impl FnOnce() -> Option<PageContext>) {
    if !matches!(cap.kind, Kind::Text | Kind::Url) {
        return;
    }
    let Some(text) = text_of(&cap.reps) else {
        return;
    };
    let found = detect(text);
    let id = match &found {
        Some(Detection {
            provider: Some(_), ..
        }) => None,
        _ => id_span(text),
    };
    if found.is_none() && id.is_none() {
        return;
    }
    if cap.source_url.is_none()
        && let Some(page) = page()
    {
        cap.source_url = Some(page.url);
        cap.source_title = cap.source_title.take().or(page.title);
    }
    let url_host = cap.source_url.as_deref().and_then(host);
    let site = cap
        .source_url
        .as_deref()
        .and_then(domain)
        .map(|d| site_name(&d));
    let app = cap.app.as_ref().and_then(|a| a.name.as_deref());
    let hinted = has_hint(cap.source_url.as_deref()) || has_hint(cap.source_title.as_deref());
    let (kind, provider, label, ranges) = match found {
        Some(Detection {
            provider: Some(provider),
            ranges,
        }) => (
            SecretKind::Key,
            provider.to_owned(),
            provider.to_owned(),
            ranges,
        ),
        Some(found) if hinted => {
            let provider = site.or(app.map(str::to_owned));
            let label = provider
                .as_ref()
                .map_or("API key".to_owned(), |p| format!("{p} key"));
            let provider = provider.unwrap_or_else(|| "API key".to_owned());
            (SecretKind::Key, provider, label, found.ranges)
        }
        // An ID-shaped value on a credentials page or assigned to a
        // key-like name is a key, e.g. `API_KEY="023e10…"`.
        _ => match (id, site) {
            (Some(range), site) if hinted || has_hint(assigned_name(&text[..range.start])) => {
                let provider = site.or(app.map(str::to_owned));
                let label = provider
                    .as_ref()
                    .map_or("API key".to_owned(), |p| format!("{p} key"));
                let provider = provider.unwrap_or_else(|| "API key".to_owned());
                (SecretKind::Key, provider, label, vec![range])
            }
            (Some(range), Some(site)) => {
                let label = format!("{site} ID");
                (SecretKind::Id, site, label, vec![range])
            }
            _ => return,
        },
    };
    let place = url_host
        .or(cap.source_title.as_deref())
        .or(app)
        .filter(|p| *p != provider);
    let name = assigned_name(&text[..ranges[0].start]).map_or_else(
        || match place {
            Some(place) => format!("{label} · {place}"),
            None => label,
        },
        str::to_owned,
    );
    let masked = match kind {
        SecretKind::Id => text.to_owned(),
        SecretKind::Key => {
            let mut masked = String::with_capacity(text.len());
            let mut end = 0;
            for range in &ranges {
                masked.push_str(&text[end..range.start]);
                masked.push_str(&mask(&text[range.clone()]));
                end = range.end;
            }
            masked.push_str(&text[end..]);
            masked
        }
    };
    cap.secret = Some(Secret {
        kind,
        provider,
        name,
        masked,
    });
}

/// The registrable domain of a URL's host: `dash.cloudflare.com` →
/// `cloudflare.com`, `www.bbc.co.uk` → `bbc.co.uk`.
pub fn domain(url: &str) -> Option<String> {
    let host = host(url)?.to_ascii_lowercase();
    if host.starts_with('[') {
        return None;
    }
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Some(host);
    }
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    let n = labels.len();
    let suffix = if n >= 3 && TWO_PART_SUFFIXES.contains(&labels[n - 2..].join(".").as_str()) {
        3
    } else {
        2
    };
    Some(labels[n.saturating_sub(suffix)..].join("."))
}

/// A readable site name for a registrable domain: `cloudflare.com` → "Cloudflare".
pub fn site_name(domain: &str) -> String {
    let label = domain.split('.').next().unwrap_or(domain);
    if let Some((_, name)) = SITE_NAMES.iter().find(|(l, _)| *l == label) {
        return (*name).to_owned();
    }
    let mut chars = label.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

/// The span of a single copied identifier, alone or as `NAME=value`.
fn id_span(text: &str) -> Option<Range<usize>> {
    if text.trim().contains('\n') {
        return None;
    }
    let line = text.trim_end();
    let end = line.trim_end_matches(['"', '\'']).len();
    let start = line[..end]
        .char_indices()
        .rev()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '-' || *c == '_'))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let prefix = &text[..start];
    if !prefix.trim().is_empty() && assigned_name(prefix).is_none() {
        return None;
    }
    is_id(&text[start..end]).then_some(start..end)
}

/// UUIDs, hex IDs (not 40- or 64-digit commit hashes), `prefix_` IDs and
/// long lowercase alphanumeric IDs. Mixed-case random strings look like keys.
fn is_id(v: &str) -> bool {
    let b = v.as_bytes();
    if !(20..=128).contains(&b.len()) {
        return false;
    }
    let digits = b.iter().filter(|c| c.is_ascii_digit()).count();
    let letters = b.iter().filter(|c| c.is_ascii_alphabetic()).count();
    let uuid = b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        });
    if uuid {
        return true;
    }
    if b.iter().all(u8::is_ascii_hexdigit) {
        return b.len() != 40 && b.len() != 64 && digits > 0 && letters > 0;
    }
    let prefixed = v.split_once('_').is_some_and(|(head, rest)| {
        (2..=8).contains(&head.len())
            && head.bytes().all(|c| c.is_ascii_lowercase())
            && rest.len() >= 14
            && rest.bytes().all(|c| c.is_ascii_alphanumeric())
    });
    let lowercase = b
        .iter()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    (prefixed || lowercase) && digits >= 3 && letters > 0 && entropy(v) >= 3.5
}

/// How a password shows: nothing of it, not even its length.
pub const PASSWORD_MASK: &str = "••••••••";

/// Masks a value marked as a key by hand: a long token keeps its ends, as
/// detected keys do; anything shorter may be a password and shows nothing.
pub fn mask_marked(value: &str) -> String {
    if value.chars().count() >= 32 {
        mask(value)
    } else {
        PASSWORD_MASK.to_owned()
    }
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
            .all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c)))
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

    const OPENAI: &str = "sk-proj-Ab3dEf6hIj9kLm2nOp5qRs8tUv1wXy4zAb7c";
    const GITHUB: &str = "ghp_Zx9Yw8Vu7Ts6Rq5Po4Nm3Lk2Ji1Hg0FeDcBa99";

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
    fn known_keys_are_named_and_every_key_is_masked() {
        let s = annotated(OPENAI, Some("https://platform.openai.com/api-keys"), None).unwrap();
        assert_eq!(s.provider, "OpenAI API key");
        assert_eq!(s.name, "OpenAI API key · platform.openai.com");
        assert_eq!(s.masked, "sk-pro…Ab7c");

        let env = format!("export OPENAI_API_KEY=\"{OPENAI}\"\nGITHUB_TOKEN={GITHUB}\n");
        let s = annotated(&env, None, None).unwrap();
        assert_eq!(s.name, "OPENAI_API_KEY");
        assert_eq!(
            s.masked,
            "export OPENAI_API_KEY=\"sk-pro…Ab7c\"\nGITHUB_TOKEN=ghp_Zx…Ba99\n"
        );

        let anthropic = annotated(
            "sk-ant-api03-Zx9Yw8Vu7Ts6Rq5Po4Nm3Lk2Ji1Hg0FeDcBa",
            None,
            None,
        )
        .unwrap();
        assert_eq!(anthropic.provider, "Anthropic API key");
        assert_eq!(anthropic.name, "Anthropic API key · Chrome");

        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmU=\n-----END OPENSSH PRIVATE KEY-----\n";
        let s = annotated(pem, None, None).unwrap();
        assert_eq!(s.provider, "Private key");
        assert!(!s.masked.contains("b3Blbn"));
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
        assert_eq!(s.provider, "Cloudflare");
        assert_eq!(s.name, "Cloudflare key · dash.cloudflare.com");
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
        let quoted = annotated(
            "API_KEY=\"023e105f4ecef8ad9ca31a8372d0c353\"",
            Some("https://example.com/settings"),
            None,
        )
        .unwrap();
        assert_eq!(
            (quoted.kind, quoted.name.as_str()),
            (SecretKind::Key, "API_KEY")
        );
        assert!(!quoted.masked.contains("ecef8ad9ca31a"));
    }

    #[test]
    fn cloudflare_tokens_site_providers_and_ids() {
        let token = format!(
            "cfut_{}1a2b3c4d",
            "Ab3dEf6hIj9kLm2nOp5qRs8tUv1wXy4zAb7cDe0f"
        );
        let s = annotated(&token, Some("https://dash.cloudflare.com/profile"), None).unwrap();
        assert_eq!(
            (s.kind, s.provider.as_str()),
            (SecretKind::Key, "Cloudflare API token")
        );
        let router = annotated(&format!("sk-or-v1-{}", "a1".repeat(32)), None, None).unwrap();
        assert_eq!(router.provider, "OpenRouter API key");

        assert_eq!(
            domain("https://console.aws.amazon.com/iam"),
            Some("amazon.com".into())
        );
        assert_eq!(domain("https://www.bbc.co.uk/x"), Some("bbc.co.uk".into()));
        let aws = annotated(
            "q8Zr2LmN0vX4tY7pW1sK9dF3gH6jB5cA",
            Some("https://console.aws.amazon.com/iam/home#/security_credentials"),
            None,
        )
        .unwrap();
        assert_eq!(aws.provider, "Amazon");

        let zone = "023e105f4ecef8ad9ca31a8372d0c353";
        let url = Some("https://dash.cloudflare.com/023e105f4ecef8ad9ca31a8372d0c353/example.com");
        let id = annotated(zone, url, None).unwrap();
        assert_eq!(id.kind, SecretKind::Id);
        assert_eq!(id.name, "Cloudflare ID · dash.cloudflare.com");
        assert_eq!(id.masked, zone);
        let named = annotated(&format!("CF_ZONE_ID={zone}"), url, None).unwrap();
        assert_eq!(named.name, "CF_ZONE_ID");
        let commit = "9fceb02d0ae598e95dc970b74767f19372d61af8";
        let gh = Some("https://github.com/x/y/commit/9fceb02");
        assert_eq!(annotated(commit, gh, None), None);
        assert_eq!(annotated(zone, None, None), None);
    }
}
