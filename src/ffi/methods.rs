//! The `cr_call` methods: one variant per method, with its params.

#[cfg(feature = "pastenow-import")]
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Code, CrHandle, Error};
use crate::model::{ItemSummary, Kind, SecretKind, Settings, TEXT_FORMATS};
use crate::query::{Search, contact_name, contact_named};
use crate::rules::{Rule, SortType};
use crate::store::{Find, PasteList, SecretFilter, Store};
use crate::{capture, now_ms, platform};

type R<T> = Result<T, Error>;

const DETAIL_TEXT_LIMIT: usize = 256 * 1024;

/// `{"method": "...", "params": {...}}`; params may be omitted when a method takes none.
#[derive(Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
enum Request {
    Items(ItemsQuery),
    ParseQuery {
        query: String,
        /// The app the caller already narrowed to; `from:` words then stay text.
        app: Option<String>,
    },
    Item {
        id: i64,
    },
    PastePrep {
        id: i64,
        #[serde(default)]
        plain: bool,
    },
    Counts {},
    DataVersion {},
    Checkpoint {},
    RevealKey {
        id: i64,
    },
    Secrets(SecretsQuery),
    SecretFacets {
        kind: Option<SecretKind>,
    },
    SetNote {
        id: i64,
        note: Option<String>,
    },
    SetTitle {
        id: i64,
        title: Option<String>,
    },
    Delete {
        id: i64,
    },
    Apps {
        query: Option<String>,
    },
    Lists {},
    ListIds {
        name: String,
    },
    CreateList {
        name: String,
        rule: Option<Rule>,
    },
    RenameList {
        name: String,
        new_name: String,
    },
    DeleteList {
        name: String,
    },
    AddToList {
        name: String,
        id: i64,
    },
    RemoveFromList {
        name: String,
        id: i64,
    },
    Settings {},
    SaveSettings(Settings),
    Excludes {},
    AddExclude {
        bundle_id: String,
    },
    RemoveExclude {
        bundle_id: String,
    },
    Stats {},
    Clear {},
    EncryptionStatus {},
    SetDbEncryption {
        on: bool,
    },
    RotateKey {},
    RecoveryCode {},
    RestoreKey {
        code: String,
    },
    ForgetUnreadableKeys {},
    #[cfg(feature = "pastenow-import")]
    ImportPastenow {
        path: Option<PathBuf>,
    },
}

#[derive(Deserialize)]
struct ItemsQuery {
    /// Search text in the grammar of [`crate::query`].
    query: Option<String>,
    kind: Option<Kind>,
    list: Option<String>,
    /// A bundle id; overrides a `from:` word.
    app: Option<String>,
    /// `phone`, `email`, `address` or `any`; an `is:` word overrides it.
    contact: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Deserialize)]
struct SecretsQuery {
    #[serde(flatten)]
    filter: SecretFilter,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    200
}

pub(super) fn call(h: &CrHandle, request: &str) -> R<Value> {
    #[derive(Deserialize)]
    struct Raw {
        method: String,
        #[serde(default)]
        params: Value,
    }
    // Methods without params are `{}` variants, which need an object.
    let raw: Raw = serde_json::from_str(request)?;
    let params = if raw.params.is_null() {
        json!({})
    } else {
        raw.params
    };
    let req = serde_json::from_value(json!({ "method": raw.method, "params": params }))?;
    handle(h, req)
}

fn handle(h: &CrHandle, req: Request) -> R<Value> {
    let now = now_ms();
    let mut store = h.store();
    store.revalidate()?;
    Ok(match req {
        Request::Items(q) => Value::from_iter(items(&store, q, now)?.iter().map(summary)),
        Request::ParseQuery { query, app } => {
            let s = Search::resolve(&store, &query, app.is_some())?;
            json!({
                "text": s.text,
                "app": s.app,
                "app_term": s.app_term,
                "contact": s.contact.map(contact_name),
                "contact_terms": s.contact_terms,
            })
        }
        Request::Item { id } => {
            let s = store.summary(id)?.ok_or_else(no_item)?;
            let formats = store.formats(id)?;
            let text = match formats.iter().find(|f| TEXT_FORMATS.contains(&f.as_str())) {
                _ if is_masked(&s) => s.preview.clone(),
                Some(f) => store
                    .rep(id, f)?
                    .and_then(|d| String::from_utf8(d).ok())
                    .map(|t| truncate(&t, DETAIL_TEXT_LIMIT).to_owned()),
                None => None,
            };
            let mut v = summary(&s);
            v["text"] = json!(text);
            v["formats"] = json!(formats);
            v
        }
        Request::PastePrep { id, plain } => {
            // Keys go through reveal_key so the app can gate and conceal them.
            if is_masked(&store.summary(id)?.ok_or_else(no_item)?) {
                return Err(Error::new(Code::Invalid, "item is a key"));
            }
            let item = store.get(id)?.ok_or_else(no_item)?;
            platform::with_autorelease_pool(|| -> R<()> {
                let ctx = clipboard_rs::ClipboardContext::new()
                    .map_err(|e| Error::new(Code::Io, e.to_string()))?;
                capture::write(&ctx, &item.reps, plain)?;
                #[cfg(target_os = "macos")]
                crate::monitor::own_change(platform::pasteboard_state().0);
                Ok(())
            })?;
            store.touch(id, now)?;
            Value::Null
        }
        Request::Counts {} => Value::from_iter(
            store
                .contact_counts()?
                .into_iter()
                .map(|(name, n)| (name.to_owned(), json!(n))),
        ),
        Request::DataVersion {} => json!(store.data_version()?),
        Request::Checkpoint {} => {
            store.checkpoint();
            Value::Null
        }
        Request::RevealKey { id } => json!(store.get(id)?.ok_or_else(no_item)?.text()),
        Request::Secrets(SecretsQuery { filter, limit }) => {
            Value::from_iter(store.secrets(&filter, limit)?.iter().map(summary))
        }
        Request::SecretFacets { kind } => json!(store.secret_facets(kind)?),
        Request::SetNote { id, note } => json!(store.set_note(id, note.as_deref())?),
        Request::SetTitle { id, title } => {
            let title = title.filter(|t| !t.trim().is_empty());
            json!(store.set_title(id, title.as_deref())?)
        }
        Request::Delete { id } => json!(store.delete(id)?),
        Request::Apps { query } => json!(store.apps(query.as_deref())?),
        Request::Lists {} => Value::from_iter(store.lists()?.iter().map(list_json)),
        Request::ListIds { name } => json!(store.list_ids(&name)?),
        Request::CreateList { name, rule } => {
            json!(store.create_list(&name, rule.as_ref(), SortType::Recent)?)
        }
        Request::RenameList { name, new_name } => json!(store.rename_list(&name, &new_name)?),
        Request::DeleteList { name } => json!(store.delete_list(&name)?),
        Request::AddToList { name, id } => {
            store.add_to_list(&name, id, now)?;
            json!(true)
        }
        Request::RemoveFromList { name, id } => json!(store.remove_from_list(&name, id)?),
        Request::Settings {} => json!(store.settings()?),
        Request::SaveSettings(settings) => {
            store.save_settings(&settings)?;
            store.apply_retention(&settings, now)?;
            // Keys found by a rescan are sealed now if the keychain allows.
            let _ = store.encrypt_pending();
            json!(settings)
        }
        Request::Excludes {} => json!(store.excludes()?),
        Request::AddExclude { bundle_id } => {
            store.add_exclude(&bundle_id)?;
            json!(true)
        }
        Request::RemoveExclude { bundle_id } => json!(store.remove_exclude(&bundle_id)?),
        Request::Stats {} => {
            let s = store.stats()?;
            json!({
                "items": s.items,
                "raw_bytes": s.raw_bytes,
                "stored_bytes": s.stored_bytes,
                "file_bytes": s.file_bytes,
                "path": h.path.display().to_string(),
            })
        }
        Request::Clear {} => json!(store.clear()?),
        Request::EncryptionStatus {} => json!(store.encryption_status()?),
        Request::SetDbEncryption { on } => {
            store.set_db_encryption(on)?;
            json!(store.encryption_status()?)
        }
        Request::RotateKey {} => json!(store.rotate_key()?),
        Request::RecoveryCode {} => json!(store.recovery_code()?.as_str()),
        Request::RestoreKey { code } => {
            let generation = store.restore_key(&code)?;
            let _ = store.encrypt_pending();
            json!(generation)
        }
        Request::ForgetUnreadableKeys {} => json!(store.forget_unreadable_keys()?),
        #[cfg(feature = "pastenow-import")]
        Request::ImportPastenow { path } => {
            let r = crate::import_pastenow::import_live(&mut store, path)?;
            json!({
                "imported": r.imported,
                "duplicates": r.duplicates,
                "skipped": r.skipped.len(),
                "lists_created": r.lists_created,
                "excludes": r.excludes,
            })
        }
    })
}

fn items(store: &Store, q: ItemsQuery, now: i64) -> R<Vec<ItemSummary>> {
    let list = match &q.list {
        Some(name) => Some(
            store
                .list_by_name(name)?
                .ok_or_else(|| Error::new(Code::NotFound, format!("no list named {name:?}")))?,
        ),
        None => None,
    };
    let contact = match q.contact.as_deref() {
        Some(c) => Some(
            contact_named(c)
                .ok_or_else(|| Error::new(Code::Invalid, format!("unknown contact {c:?}")))?,
        ),
        None => None,
    };
    let search = Search::resolve(
        store,
        q.query.as_deref().unwrap_or_default(),
        q.app.is_some(),
    )?;
    let parsed = search.find();
    let find = Find {
        kind: q.kind,
        list: list.as_ref(),
        app: q.app.as_deref().or(parsed.app),
        contact: parsed.contact.or(contact),
        ..parsed
    };
    Ok(store.find(&find, q.limit, now)?)
}

fn no_item() -> Error {
    Error::new(Code::NotFound, "no such item")
}

fn summary(s: &ItemSummary) -> Value {
    json!({
        "id": s.id,
        "kind": s.kind.as_str(),
        "title": s.title,
        "preview": s.preview,
        "app": s.app_bundle_id,
        "source_url": s.source_url,
        "source_domain": s.source_domain,
        "secret_kind": s.secret_kind.map(SecretKind::as_str),
        "secret_provider": s.secret_provider,
        "note": s.note,
        "contact": s.contact,
        "created_at": s.created_at,
        "last_used_at": s.last_used_at,
        "byte_size": s.byte_size,
    })
}

fn list_json(l: &PasteList) -> Value {
    json!({
        "name": l.name,
        "rule": l.rule,
        "sort": l.sort.as_str(),
        "manual": l.rule.is_none(),
    })
}

/// Keys stay masked; IDs are shown in full.
pub(super) fn is_masked(s: &ItemSummary) -> bool {
    s.secret_kind == Some(SecretKind::Key)
        || (s.secret_provider.is_some() && s.secret_kind != Some(SecretKind::Id))
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::ffi::cr_item_data;
    use crate::model::{Capture, Secret};

    #[test]
    fn keys_filter_by_kind_and_notes_round_trip_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let h = CrHandle {
            store: std::sync::Arc::new(Mutex::new(Store::open(&path).unwrap())),
            path,
            watching: AtomicBool::new(false),
        };
        let mut key = Capture::from_text("sk-live-secret-value".into());
        key.secret = Some(Secret {
            kind: SecretKind::Key,
            provider: "Test key".into(),
            name: "TEST_KEY".into(),
            masked: "sk-liv…alue".into(),
        });
        key.app = Some(crate::AppInfo {
            bundle_id: "com.openai.codex".into(),
            name: Some("ChatGPT".into()),
            path: None,
        });
        let id = h.store().ingest(&key, 1).unwrap().id();
        let send = |m: &str, p: Value| call(&h, &json!({"method": m, "params": p}).to_string());
        let req = |m: &str, p: Value| send(m, p).unwrap();
        let code = |m: &str, p: Value| send(m, p).unwrap_err().code;

        assert_eq!(
            req("set_note", json!({"id": id, "note": "billing"})),
            json!(true)
        );
        let keys = req("secrets", json!({"kind": "key", "provider": "test KEY"}));
        assert_eq!(keys[0]["note"], "billing");
        assert_eq!(keys[0]["secret_kind"], "key");
        assert_eq!(req("secrets", json!({"kind": "id"})), json!([]));
        assert_eq!(
            req("secret_facets", Value::Null)["providers"][0]["count"],
            1
        );
        assert_ne!(
            req("item", json!({"id": id}))["text"],
            "sk-live-secret-value"
        );
        assert_eq!(
            req("apps", json!({"query": "codex"})),
            json!([{"bundle_id": "com.openai.codex", "name": "ChatGPT", "count": 1}])
        );
        let from = |app: &str| {
            req(
                "items",
                json!({"app": app, "query": "TEST", "kind": "text"}),
            )
        };
        assert_eq!(from("com.openai.codex")[0]["id"], id);
        assert_eq!(req("items", json!({"query": "@codex TEST"}))[0]["id"], id);
        let phone = h
            .store()
            .ingest(&Capture::from_text("+41 79 123 45 67".into()), 2)
            .unwrap()
            .id();
        let tagged = req("items", json!({"contact": "any"}));
        assert_eq!(
            (&tagged[0]["id"], &tagged[0]["contact"]),
            (&json!(phone), &json!(["phone"]))
        );
        assert_eq!(req("items", json!({"contact": "email"})), json!([]));
        assert_eq!(
            req("items", json!({"query": "is:phone", "contact": "email"}))[0]["id"],
            phone
        );
        assert_eq!(
            req("parse_query", json!({"query": "from:chat is:contact 79"})),
            json!({
                "text": "79",
                "app": {"bundle_id": "com.openai.codex", "name": "ChatGPT", "count": 1},
                "app_term": "from:chat",
                "contact": "any",
                "contact_terms": ["is:contact"],
            })
        );
        assert_eq!(
            call(&h, r#"{"method": "counts"}"#).unwrap(),
            json!({"phone": 1, "email": 0, "address": 0, "any": 1})
        );
        assert_eq!(req("item", json!({"id": id}))["contact"], json!([]));
        assert_eq!(from("com.apple.mail"), json!([]));
        req("add_to_list", json!({"name": "Pinned", "id": id}));
        assert_eq!(req("list_ids", json!({"name": "Pinned"})), json!([id]));
        req("create_list", json!({"name": "Work"}));
        assert_eq!(
            req("rename_list", json!({"name": "Work", "new_name": "Job"})),
            json!(true)
        );
        assert!(
            req("lists", json!({}))
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l["name"] == "Job")
        );
        assert_eq!(code("item", json!({"id": 999})), Code::NotFound);
        assert_eq!(code("items", json!({"list": "Nope"})), Code::NotFound);
        assert_eq!(code("frobnicate", json!({})), Code::Invalid);
        assert_eq!(code("add_to_list", json!({"name": "Job"})), Code::Invalid);
        assert_eq!(code("create_list", json!({"name": "Job"})), Code::Invalid);
        assert_eq!(code("paste_prep", json!({"id": id})), Code::Invalid);
        let format = CString::new(h.store().formats(id).unwrap().remove(0)).unwrap();
        let (mut out, mut len) = (std::ptr::null_mut(), 0);
        assert_eq!(
            unsafe { cr_item_data(&h, id, format.as_ptr(), &mut out, &mut len) },
            2
        );
    }
}
