use std::error::Error;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use cliprust::monitor::{self, Event};
use cliprust::{
    Capture, Facet, Ingest, ItemSummary, Kind, Rule, SecretFilter, SecretKind, SortType, Store,
    capture, now_ms, secrets,
};

type CliResult<T = ()> = Result<T, Box<dyn Error>>;

#[derive(Parser)]
#[command(
    version,
    about = "Clipboard history with deduplication and full-text search"
)]
struct Cli {
    /// Database file [default: the platform data directory]
    #[arg(long, env = "CLIPRUST_DB", global = true)]
    db: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Record every clipboard change until interrupted
    Watch {
        #[arg(long, default_value_t = 300)]
        interval_ms: u64,
        /// Skip clips larger than this many MiB
        #[arg(long, default_value_t = 32)]
        max_mb: usize,
    },
    /// Show recent items, or the items of a list
    List {
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        list: Option<String>,
    },
    /// Find items containing every term as a substring
    Search {
        #[arg(required = true)]
        terms: Vec<String>,
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// Print an item's full text
    Show {
        id: i64,
    },
    /// Put an item back on the clipboard
    Copy {
        id: i64,
        /// Drop rich text, HTML and other formats
        #[arg(long)]
        plain: bool,
    },
    /// Store text from the argument or stdin without using the clipboard
    Add {
        text: Option<String>,
    },
    /// Set or remove an item's title
    Title {
        id: i64,
        title: Option<String>,
    },
    Delete {
        id: i64,
    },
    /// Delete the history except items in manual lists
    Clear,
    /// Delete unlisted items beyond the newest KEEP
    Prune {
        keep: u32,
    },
    /// Detected API keys and tokens, and IDs copied from web pages
    #[command(args_conflicts_with_subcommands = true)]
    Keys {
        #[command(subcommand)]
        cmd: Option<KeysCmd>,
        #[arg(short = 'n', long, default_value_t = 50)]
        limit: usize,
        /// Only this provider, e.g. "Cloudflare API token"
        #[arg(long)]
        provider: Option<String>,
        /// Only keys copied from this site, e.g. cloudflare.com
        #[arg(long)]
        site: Option<String>,
        /// Show IDs instead of keys
        #[arg(long, conflicts_with = "all")]
        ids: bool,
        /// Show keys and IDs
        #[arg(long)]
        all: bool,
        /// Group by provider
        #[arg(long)]
        group: bool,
    },
    /// Attach a searchable note to an item; without text, remove it
    Note {
        id: i64,
        text: Vec<String>,
    },
    #[command(subcommand)]
    Lists(ListsCmd),
    #[command(subcommand)]
    Exclude(ExcludeCmd),
    /// Stop recording until `resume`
    Pause,
    Resume,
    /// Show settings, or change one
    Settings {
        #[command(subcommand)]
        change: Option<SettingsCmd>,
    },
    Stats,
    /// Copy history, lists and exclusions from PasteNow (quit PasteNow first)
    #[cfg(feature = "pastenow-import")]
    ImportPastenow {
        /// PasteNow.sqlite [default: PasteNow's store in its group container]
        path: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum KeysCmd {
    /// Providers and sites with how many keys and IDs each has
    Facets,
}

#[derive(Subcommand)]
enum ListsCmd {
    Ls,
    /// Create a list; with --rule it is a smart list
    Add {
        name: String,
        /// Smart-list rule as JSON, e.g. {"type":"kind","kind":"url"}
        #[arg(long)]
        rule: Option<String>,
        #[arg(long, default_value = "recent", value_parser = ["recent", "oldest"])]
        sort: String,
    },
    Rm {
        name: String,
    },
    /// Add an item to a manual list
    Put {
        name: String,
        id: i64,
    },
    /// Remove an item from a manual list
    Take {
        name: String,
        id: i64,
    },
}

#[derive(Subcommand)]
enum ExcludeCmd {
    Ls,
    /// Never record copies made while this app is in front
    Add {
        bundle_id: String,
    },
    Rm {
        bundle_id: String,
    },
}

#[derive(Subcommand)]
enum SettingsCmd {
    /// Recognise API keys and file them in the Keys list
    DetectKeys {
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
    /// Keep at most N unlisted items ("off" for no limit)
    MaxItems { value: String },
    /// Delete unlisted items unused for N days ("off" for no limit)
    MaxAgeDays { value: String },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cliprust: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> CliResult {
    let path = match cli.db {
        Some(path) => path,
        None => default_db_path()?,
    };
    let mut store = Store::open(&path)?;
    match cli.cmd {
        Cmd::Watch {
            interval_ms,
            max_mb,
        } => {
            let config = monitor::Config {
                interval: Duration::from_millis(interval_ms),
                max_item_bytes: max_mb << 20,
            };
            return watch(store, config, &path);
        }
        Cmd::List { limit, list: None } => print_items(&store.recent(limit)?),
        Cmd::List {
            limit,
            list: Some(name),
        } => {
            let list = store
                .list_by_name(&name)?
                .ok_or_else(|| format!("no list named {name:?}"))?;
            print_items(&store.list_items(&list, limit, now_ms())?);
        }
        Cmd::Search { terms, limit } => print_items(&store.search(&terms.join(" "), limit)?),
        Cmd::Show { id } => {
            let item = store.get(id)?.ok_or_else(|| format!("no item #{id}"))?;
            match item.text() {
                Some(text) => println!("{text}"),
                None => {
                    let formats: Vec<&str> = item.reps.iter().map(|r| r.format.as_str()).collect();
                    println!(
                        "{} item, {} bytes, formats: {}",
                        item.summary.kind,
                        item.summary.byte_size,
                        formats.join(", ")
                    );
                }
            }
        }
        Cmd::Copy { id, plain } => {
            let item = store.get(id)?.ok_or_else(|| format!("no item #{id}"))?;
            let ctx = clipboard_rs::ClipboardContext::new().map_err(|e| e.to_string())?;
            capture::write(&ctx, &item.reps, plain)?;
            store.touch(id, now_ms())?;
        }
        Cmd::Add { text } => {
            let text = match text {
                Some(text) => text,
                None => {
                    let mut text = String::new();
                    std::io::stdin().read_to_string(&mut text)?;
                    text
                }
            };
            let mut cap = Capture::from_text(text);
            if store.settings()?.detect_keys {
                secrets::annotate(&mut cap, || None);
            }
            match store.ingest(&cap, now_ms())? {
                Ingest::Inserted(id) => println!("stored #{id}"),
                Ingest::Duplicate(id) => println!("already stored as #{id}, moved to top"),
            }
        }
        Cmd::Title { id, title } => require(store.set_title(id, title.as_deref())?, id)?,
        Cmd::Delete { id } => require(store.delete(id)?, id)?,
        Cmd::Clear => println!("deleted {} items", store.clear()?),
        Cmd::Prune { keep } => println!("deleted {} items", store.prune(Some(keep), None)?),
        Cmd::Keys {
            cmd: Some(KeysCmd::Facets),
            ..
        } => {
            let facets = store.secret_facets(None)?;
            print_facets("providers", &facets.providers);
            println!();
            print_facets("sites", &facets.domains);
        }
        Cmd::Keys {
            cmd: None,
            limit,
            provider,
            site,
            ids,
            all,
            group,
        } => {
            let kind = match (ids, all) {
                (_, true) => None,
                (true, _) => Some(SecretKind::Id),
                _ => Some(SecretKind::Key),
            };
            let filter = SecretFilter {
                kind,
                provider,
                domain: site,
            };
            let items = store.secrets(&filter, limit)?;
            if items.is_empty() && !store.settings()?.detect_keys {
                println!("no keys yet (enable with `cliprust settings detect-keys on`)");
            } else if group {
                print_grouped(&items);
            } else {
                for it in &items {
                    println!("{}", secret_line(it, true));
                }
            }
        }
        Cmd::Note { id, text } => require(store.set_note(id, Some(&text.join(" ")))?, id)?,
        Cmd::Lists(cmd) => lists(&store, cmd)?,
        Cmd::Exclude(ExcludeCmd::Ls) => store.excludes()?.iter().for_each(|b| println!("{b}")),
        Cmd::Exclude(ExcludeCmd::Add { bundle_id }) => store.add_exclude(&bundle_id)?,
        Cmd::Exclude(ExcludeCmd::Rm { bundle_id }) => {
            if !store.remove_exclude(&bundle_id)? {
                return Err(format!("{bundle_id} was not excluded").into());
            }
        }
        Cmd::Pause | Cmd::Resume => {
            let mut settings = store.settings()?;
            settings.paused = matches!(cli.cmd, Cmd::Pause);
            store.save_settings(&settings)?;
        }
        Cmd::Settings { change } => {
            let mut settings = store.settings()?;
            match change {
                None => {}
                Some(SettingsCmd::DetectKeys { state }) => settings.detect_keys = state == "on",
                Some(SettingsCmd::MaxItems { value }) => settings.max_items = limit_value(&value)?,
                Some(SettingsCmd::MaxAgeDays { value }) => {
                    settings.max_age_days = limit_value(&value)?
                }
            }
            store.save_settings(&settings)?;
            println!("{}", serde_json::to_string_pretty(&settings)?);
        }
        Cmd::Stats => {
            let s = store.stats()?;
            println!("items          {}", s.items);
            println!("content        {}", human_bytes(s.raw_bytes));
            println!("compressed     {}", human_bytes(s.stored_bytes));
            println!(
                "database file  {}  ({})",
                human_bytes(s.file_bytes),
                path.display()
            );
        }
        #[cfg(feature = "pastenow-import")]
        Cmd::ImportPastenow { path } => import_pastenow(&mut store, path)?,
    }
    Ok(())
}

#[cfg(feature = "pastenow-import")]
fn import_pastenow(store: &mut Store, path: Option<PathBuf>) -> CliResult {
    use cliprust::import_pastenow;

    let src = match path {
        Some(path) => path,
        None => home()?.join(
            "Library/Group Containers/KAQ3M6FSM7.app.pastenow.PasteNow/Documents/PasteNow.sqlite",
        ),
    };
    if !src.is_file() {
        return Err(format!("no PasteNow store at {}", src.display()).into());
    }
    // A fresh 0700 directory with a random name, removed when dropped.
    let tmp = tempfile::Builder::new()
        .prefix("cliprust-pastenow-")
        .tempdir()?;
    let copy = snapshot_pastenow(&src, tmp.path())?;
    let report = import_pastenow::import(store, &copy)?;

    println!(
        "imported {} items, {} already present",
        report.imported, report.duplicates
    );
    for (pk, reason) in &report.skipped {
        println!("skipped item {pk}: {reason}");
    }
    println!("created {} lists", report.lists_created);
    for (name, reason) in &report.lists_skipped {
        println!("skipped list {name:?}: {reason}");
    }
    println!("excluded {} apps", report.excludes);
    Ok(())
}

/// Copies the store with its WAL and external blobs, so the live files are
/// never opened.
#[cfg(feature = "pastenow-import")]
fn snapshot_pastenow(src: &Path, dir: &Path) -> CliResult<PathBuf> {
    let name = src.file_name().ok_or("store path has no file name")?;
    for suffix in ["", "-wal", "-shm"] {
        let mut file = name.to_owned();
        file.push(suffix);
        let from = src.with_file_name(&file);
        if suffix.is_empty() || from.exists() {
            std::fs::copy(&from, dir.join(&file))?;
        }
    }
    let external_dir = cliprust::import_pastenow::EXTERNAL_DATA_DIR;
    let external = src.with_file_name(external_dir);
    if external.is_dir() {
        let to = dir.join(external_dir);
        std::fs::create_dir_all(&to)?;
        for entry in std::fs::read_dir(&external)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                std::fs::copy(entry.path(), to.join(entry.file_name()))?;
            }
        }
    }
    Ok(dir.join(name))
}

fn lists(store: &Store, cmd: ListsCmd) -> CliResult {
    match cmd {
        ListsCmd::Ls => {
            for list in store.lists()? {
                let rule = match &list.rule {
                    Some(rule) => serde_json::to_string(rule)?,
                    None => "manual".to_owned(),
                };
                println!("{:<20}  {:<6}  {rule}", list.name, list.sort.as_str());
            }
        }
        ListsCmd::Add { name, rule, sort } => {
            let rule: Option<Rule> = rule.as_deref().map(serde_json::from_str).transpose()?;
            let sort = SortType::parse(&sort).unwrap_or_default();
            store.create_list(&name, rule.as_ref(), sort)?;
        }
        ListsCmd::Rm { name } => {
            if !store.delete_list(&name)? {
                return Err(format!("no list named {name:?}").into());
            }
        }
        ListsCmd::Put { name, id } => {
            if store.list_by_name(&name)?.is_some_and(|l| l.rule.is_some()) {
                return Err(
                    format!("{name:?} is a smart list; its items come from its rule").into(),
                );
            }
            store.add_to_list(&name, id, now_ms())?;
        }
        ListsCmd::Take { name, id } => {
            if !store.remove_from_list(&name, id)? {
                return Err(format!("item #{id} is not in {name:?}").into());
            }
        }
    }
    Ok(())
}

fn watch(store: Store, config: monitor::Config, path: &Path) -> CliResult {
    eprintln!(
        "cliprust: watching the clipboard, saving to {}",
        path.display()
    );
    let on_event = |event: Event| match event {
        Event::Stored {
            outcome: Ingest::Inserted(id),
            kind,
            secret,
        } => match secret {
            Some(secret) => eprintln!("stored #{id} ({kind}, {})", secret.as_str()),
            None => eprintln!("stored #{id} ({kind})"),
        },
        Event::Stored {
            outcome: Ingest::Duplicate(id),
            ..
        } => eprintln!("seen again #{id}"),
        Event::Skipped(reason) => eprintln!("skipped: {reason}"),
        Event::Failed(e) => eprintln!("error: {e}"),
    };
    #[cfg(target_os = "macos")]
    {
        std::thread::spawn(move || {
            if let Err(e) = monitor::run(store, config, on_event) {
                eprintln!("cliprust: {e}");
                std::process::exit(1);
            }
        });
        cliprust::platform::run_main_loop()
    }
    #[cfg(not(target_os = "macos"))]
    Ok(monitor::run(store, config, on_event)?)
}

fn secret_line(it: &ItemSummary, with_provider: bool) -> String {
    let kind = it.secret_kind.map_or("-", SecretKind::as_str);
    let mut line = format!("{:>6}  {kind:<3}  ", it.id);
    if with_provider {
        line.push_str(&format!(
            "{:<24}  ",
            it.secret_provider.as_deref().unwrap_or("-")
        ));
    }
    line.push_str(&format!(
        "{:<36}  {:<24}  {}",
        it.title.as_deref().unwrap_or("-"),
        it.preview.as_deref().map(one_line).unwrap_or_default(),
        it.source_domain.as_deref().unwrap_or("-")
    ));
    if let Some(note) = &it.note {
        line.push_str(&format!("  # {}", one_line(note)));
    }
    line
}

fn print_grouped(items: &[ItemSummary]) {
    let mut groups: Vec<(&str, Vec<&ItemSummary>)> = Vec::new();
    for it in items {
        let provider = it.secret_provider.as_deref().unwrap_or("-");
        match groups.iter_mut().find(|(p, _)| *p == provider) {
            Some((_, members)) => members.push(it),
            None => groups.push((provider, vec![it])),
        }
    }
    for (i, (provider, members)) in groups.iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("{provider} ({})", members.len());
        for it in members {
            println!("{}", secret_line(it, false));
        }
    }
}

fn print_facets(heading: &str, facets: &[Facet]) {
    println!("{heading}");
    for f in facets {
        println!("{:>6}  {}", f.count, f.name);
    }
}

fn print_items(items: &[ItemSummary]) {
    let now = now_ms();
    for it in items {
        let label = match (it.kind, it.title.as_deref(), it.preview.as_deref()) {
            (Kind::Image, _, _) => format!("[image, {}]", human_bytes(it.byte_size)),
            (_, Some(title), _) => title.to_owned(),
            (_, None, preview) => one_line(preview.unwrap_or_default()),
        };
        println!(
            "{:>6}  {:<5}  {:>4}  {:<28}  {label}",
            it.id,
            it.kind,
            age(now - it.last_used_at),
            it.app_bundle_id.as_deref().unwrap_or("-")
        );
    }
}

fn one_line(s: &str) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match s.char_indices().nth(80) {
        Some((cut, _)) => format!("{}…", &s[..cut]),
        None => s,
    }
}

fn age(ms: i64) -> String {
    match ms / 1000 {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

fn human_bytes(n: i64) -> String {
    match n {
        n if n < 1024 => format!("{n} B"),
        n if n < 1 << 20 => format!("{:.1} KiB", n as f64 / 1024.0),
        n => format!("{:.1} MiB", n as f64 / f64::from(1 << 20)),
    }
}

fn limit_value(value: &str) -> CliResult<Option<u32>> {
    if value == "off" {
        return Ok(None);
    }
    Ok(Some(value.parse().map_err(|_| {
        format!("expected a number or \"off\", got {value:?}")
    })?))
}

fn require(found: bool, id: i64) -> CliResult {
    if found {
        Ok(())
    } else {
        Err(format!("no item #{id}").into())
    }
}

fn home() -> CliResult<PathBuf> {
    Ok(PathBuf::from(
        std::env::var_os("HOME").ok_or("HOME is not set")?,
    ))
}

fn default_db_path() -> CliResult<PathBuf> {
    let home = home()?;
    let base = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        std::env::var_os("XDG_DATA_HOME").map_or_else(|| home.join(".local/share"), PathBuf::from)
    };
    Ok(base.join("cliprust/cliprust.db"))
}
