//! File and folder search, the launcher's second mode (type the file prefix, "f" by default,
//! then a space). Searches run on their own thread: through Everything when it's running, else
//! through lattice's own [`index`](super::index). Either way results are ranked by the same
//! fuzzy matcher as apps, plus your history of opening them.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use rune::AppHandle;

use super::everything::{self, Client};
use super::index::{self, Index};
use super::rank::{self, History};
use super::ui;

/// What Enter (or Ctrl+Enter) does with a file or folder.
#[derive(Clone, Debug, PartialEq)]
pub enum FileAction {
    /// Open with its default app (a folder: in Explorer).
    Open,
    /// Open the containing folder in Explorer, with the item selected.
    Reveal,
    /// Copy the full path to the clipboard.
    CopyPath,
    /// Run a command line; `{path}`, `{dir}` (the item's folder, or the folder itself) and
    /// `{name}` are filled in.
    Exec(String),
}

impl FileAction {
    pub fn parse(s: &str) -> Result<FileAction, String> {
        let s = s.trim();
        let (head, rest) = s.split_once(char::is_whitespace).unwrap_or((s, ""));
        Ok(match head.to_ascii_lowercase().as_str() {
            "open" => FileAction::Open,
            "reveal" | "open-location" | "show-in-folder" => FileAction::Reveal,
            "copy-path" | "copy" => FileAction::CopyPath,
            "exec" if !rest.trim().is_empty() => FileAction::Exec(rest.trim().to_string()),
            "exec" => return Err("exec needs a command line".into()),
            other => return Err(format!("unknown file action '{other}' (open, reveal, copy-path, exec <command>)")),
        })
    }

    /// The footer's name for it.
    pub fn label(&self) -> &'static str {
        match self {
            FileAction::Open => "Open",
            FileAction::Reveal => "Show in folder",
            FileAction::CopyPath => "Copy path",
            FileAction::Exec(_) => "Run",
        }
    }
}

/// Enter and Ctrl+Enter, for files and for folders.
#[derive(Clone, Debug)]
pub struct FileActions {
    pub file: FileAction,
    pub file_ctrl: FileAction,
    pub dir: FileAction,
    pub dir_ctrl: FileAction,
}

impl Default for FileActions {
    fn default() -> Self {
        FileActions {
            file: FileAction::Open,
            file_ctrl: FileAction::Reveal,
            dir: FileAction::Open,
            dir_ctrl: FileAction::CopyPath,
        }
    }
}

impl FileActions {
    pub fn pick(&self, dir: bool, ctrl: bool) -> &FileAction {
        match (dir, ctrl) {
            (false, false) => &self.file,
            (false, true) => &self.file_ctrl,
            (true, false) => &self.dir,
            (true, true) => &self.dir_ctrl,
        }
    }
}

/// File search settings.
#[derive(Clone, Debug)]
pub struct FileOptions {
    /// Typed before a space to switch to file search ("f").
    pub prefix: String,
    pub actions: FileActions,
    /// Folders lattice's own index walks when Everything isn't running.
    pub index_roots: Vec<PathBuf>,
    /// Folder names that index never walks into ("node_modules").
    pub index_exclude: Vec<String>,
}

/// One result.
#[derive(Clone, Debug)]
pub struct FileHit {
    pub path: Arc<str>,
    /// Byte offset of the file name in `path`.
    pub name_at: usize,
    pub dir: bool,
    /// Matched byte offsets in the name, for highlighting.
    pub positions: Vec<usize>,
}

impl FileHit {
    pub fn name(&self) -> &str {
        &self.path[self.name_at..]
    }
}

/// Where results came from, for the launcher's status line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    /// Nothing asked yet.
    Idle,
    Everything,
    Index(index::Status),
}

pub struct Results {
    pub generation: u64,
    pub hits: Vec<FileHit>,
    pub source: Source,
}

pub enum Req {
    Query { generation: u64, text: String },
    /// File mode was entered: make sure some backend is ready.
    Warm,
    /// The index has more entries: rerun the last query if the index answered it.
    Progress,
}

/// The UI's end of the search thread.
#[derive(Clone)]
pub struct Searcher {
    tx: Sender<Req>,
    /// The newest query's generation: a search running for an older one gives up.
    latest: Arc<AtomicU64>,
}

impl Searcher {
    pub fn query(&self, generation: u64, text: String) {
        self.latest.store(generation, Ordering::SeqCst);
        let _ = self.tx.send(Req::Query { generation, text });
    }

    pub fn warm(&self) {
        let _ = self.tx.send(Req::Warm);
    }
}

/// Makes the searcher; [`start`] starts its thread once the UI's handle exists.
pub fn searcher() -> (Searcher, Receiver<Req>) {
    let (tx, rx) = mpsc::channel();
    (Searcher { tx, latest: Arc::new(AtomicU64::new(0)) }, rx)
}

/// Results asked of each backend: the more, the better the fuzzy re-rank, the slower.
const EVERYTHING_MAX: u32 = 100;

pub fn start(
    searcher: &Searcher,
    rx: Receiver<Req>,
    app: AppHandle<ui::State>,
    opts: &FileOptions,
    data_dir: PathBuf,
    history: Arc<Mutex<History>>,
    want: usize,
) {
    let index = Index::new(opts.index_roots.clone(), &opts.index_exclude, data_dir.join("file-index.txt"));
    let progress_tx = searcher.tx.clone();
    let latest = searcher.latest.clone();
    let spawned = std::thread::Builder::new().name("lattice-file-search".into()).spawn(move || {
        let mut client = Client::new();
        if client.is_none() {
            log::warn!("launcher: no Everything reply window; using lattice's own file index");
        }
        // The last query, and whether the index answered it.
        let mut last: Option<(u64, String, bool)> = None;
        // Whether our index has been loaded and walked this session (Everything may come and go).
        let mut index_started = false;
        // Why Everything last failed, so each failure is logged once rather than per keystroke.
        let mut everything_down: Option<everything::Error> = None;
        let warm = |index: &Arc<Index>| {
            let tx = progress_tx.clone();
            index.refresh(move || {
                let _ = tx.send(Req::Progress);
            });
        };
        while let Ok(first) = rx.recv() {
            // Only the newest query matters; fold the backlog.
            let mut req = first;
            let mut wants_warm = false;
            while let Ok(next) = rx.try_recv() {
                match (&req, next) {
                    (_, Req::Warm) => wants_warm = true,
                    (Req::Query { .. }, Req::Progress) => {}
                    (_, next) => req = next,
                }
            }
            if wants_warm || matches!(req, Req::Warm) {
                // Everything keeps itself current; our index only matters when it can't answer.
                if !everything::running() || everything_down.is_some() {
                    warm(&index);
                    index_started = true;
                }
            }
            let (generation, text) = match req {
                Req::Query { generation, text } => (generation, text),
                Req::Progress => match &last {
                    Some((g, text, true)) if latest.load(Ordering::SeqCst) == *g => (*g, text.clone()),
                    _ => continue,
                },
                Req::Warm => continue,
            };
            let history = history.lock().unwrap().snapshot();
            let answer = client.as_mut().map(|c| search_everything(c, &text, &history, want));
            let mut source = Source::Everything;
            let mut hits = match answer {
                Some(Ok(hits)) => {
                    everything_down = None;
                    hits
                }
                // Not running, or not answering (Everything Lite has no IPC): use our index.
                failed => {
                    let why = match failed {
                        Some(Err(e)) => e,
                        _ => everything::Error::NotRunning,
                    };
                    if everything_down.as_ref() != Some(&why) {
                        match why {
                            everything::Error::NotRunning => log::info!("launcher: Everything isn't running; using lattice's file index"),
                            everything::Error::Refused => log::warn!(
                                "launcher: Everything refused the query (Everything Lite has no IPC; the full version \
                                 refuses while loading); using lattice's file index"
                            ),
                            everything::Error::NoReply => log::warn!("launcher: Everything didn't answer; using lattice's file index"),
                        }
                        everything_down = Some(why);
                    }
                    if !index_started {
                        warm(&index);
                        index_started = true;
                    }
                    source = Source::Index(index.status());
                    match search_index(&index, &text, &history, want, || latest.load(Ordering::SeqCst) != generation)
                    {
                        Some(hits) => hits,
                        None => continue, // overtaken by a newer query
                    }
                }
            };
            hits.truncate(want);
            let from_index = matches!(source, Source::Index(_));
            last = Some((generation, text, from_index));
            app.update(move |s, _| s.files_loaded(Results { generation, hits, source }));
        }
    });
    if let Err(e) = spawned {
        log::error!("launcher: can't start file search: {e}");
    }
}

/// Query text with Everything's `name:value` filters ("ext:pdf") left out, for fuzzy matching.
fn fuzzy_text(text: &str) -> String {
    text.split_whitespace().filter(|w| !w.contains(':')).collect::<Vec<_>>().join(" ")
}

/// Everything's literal match first; when that finds few, a wildcard pass that matches the
/// letters in order anywhere in the name ("vsc" as `*v*s*c*`), as the fuzzy matcher would.
fn search_everything(c: &mut Client, text: &str, history: &History, want: usize) -> Result<Vec<FileHit>, everything::Error> {
    let fuzzy = fuzzy_text(text);
    let mut items = c.query(text.trim(), EVERYTHING_MAX, everything::SORT_DATE_MODIFIED_DESC)?;
    if items.len() < want.max(20)
        && let Some(wild) = wildcard_query(text)
    {
        for item in c.query(&wild, EVERYTHING_MAX, everything::SORT_DATE_MODIFIED_DESC)? {
            if !items.iter().any(|i| i.path == item.path) {
                items.push(item);
            }
        }
    }
    let mut scored: Vec<(i32, FileHit)> = items
        .into_iter()
        .map(|item| {
            // Everything matched it, perhaps on syntax the fuzzy matcher doesn't know: keep it.
            let (score, positions) = score(&fuzzy, &item.path, item.name_at, history).unwrap_or((-200, Vec::new()));
            let hit = FileHit { path: item.path.into(), name_at: item.name_at, dir: item.dir, positions };
            (score, hit)
        })
        .collect();
    scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
    Ok(scored.into_iter().map(|(_, h)| h).collect())
}

/// `*v*s*c*` per word, or `None` when the text has syntax a wildcard would mangle.
fn wildcard_query(text: &str) -> Option<String> {
    const SPECIAL: [char; 9] = [':', '*', '?', '"', '|', '<', '>', '!', '\\'];
    if text.contains(SPECIAL) || text.contains('/') {
        return None;
    }
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| std::iter::once('*'.to_string()).chain(w.chars().map(|c| format!("{c}*"))).collect())
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// The best `want` index entries for `text`, or `None` if `stale()` turns true first.
fn search_index(
    index: &Index,
    text: &str,
    history: &History,
    want: usize,
    stale: impl Fn() -> bool,
) -> Option<Vec<FileHit>> {
    let fuzzy = fuzzy_text(text);
    let path_query = fuzzy.contains(['\\', '/']);
    let q: Vec<char> = fuzzy.chars().filter(|c| !c.is_whitespace() && *c != '\\' && *c != '/').flat_map(char::to_lowercase).collect();
    if q.is_empty() {
        return Some(Vec::new());
    }
    let mut best: Vec<(i32, FileHit)> = Vec::new();
    let mut seen = 0usize;
    let mut overtaken = false;
    index.scan(|e| {
        seen += 1;
        if seen.is_multiple_of(16_384) && stale() {
            overtaken = true;
            return false;
        }
        let hay = if path_query { &e.path[..] } else { e.name() };
        if !rank::in_order(&q, hay) {
            return true;
        }
        if let Some((s, positions)) = score(&fuzzy, &e.path, e.name_at as usize, history) {
            best.push((s, FileHit { path: e.path.as_ref().into(), name_at: e.name_at as usize, dir: e.dir, positions }));
            if best.len() > want * 8 {
                best.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
                best.truncate(want);
            }
        }
        true
    });
    if overtaken {
        return None;
    }
    best.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
    best.truncate(want);
    Some(best.into_iter().map(|(_, h)| h).collect())
}

/// Folders whose contents are rarely what you're looking for.
const NOISE: [&str; 6] = ["\\appdata\\", "\\windows\\", "$recycle.bin", "\\node_modules\\", "\\.git\\", "\\winsxs\\"];

/// How well `query` matches the item at `path`: fuzzy on the name, or on the whole path when the
/// query has a slash in it. Plus history; minus depth and noisy folders.
fn score(query: &str, path: &str, name_at: usize, history: &History) -> Option<(i32, Vec<usize>)> {
    let name = &path[name_at..];
    let (base, positions) = if query.contains(['\\', '/']) {
        let m = rank::fuzzy(&query.replace('/', "\\"), path)?;
        // Highlight only what falls in the name.
        (m.score, m.positions.into_iter().filter(|&p| p >= name_at).map(|p| p - name_at).collect())
    } else {
        let m = rank::fuzzy(query, name)?;
        (m.score, m.positions)
    };
    let lower = path.to_lowercase();
    let depth = path.matches('\\').count() as i32;
    let noise = if NOISE.iter().any(|n| lower.contains(n)) { 60 } else { 0 };
    Some((base + history.boost(path) - depth.min(15) * 2 - noise, positions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_parse() {
        assert_eq!(FileAction::parse("open"), Ok(FileAction::Open));
        assert_eq!(FileAction::parse("Reveal"), Ok(FileAction::Reveal));
        assert_eq!(FileAction::parse("copy-path"), Ok(FileAction::CopyPath));
        assert_eq!(FileAction::parse("exec code \"{path}\""), Ok(FileAction::Exec("code \"{path}\"".into())));
        assert!(FileAction::parse("exec").is_err());
        assert!(FileAction::parse("delete").is_err());
    }

    #[test]
    fn defaults_match_the_spec() {
        let a = FileActions::default();
        assert_eq!(a.pick(false, false), &FileAction::Open);
        assert_eq!(a.pick(false, true), &FileAction::Reveal);
        assert_eq!(a.pick(true, false), &FileAction::Open);
        assert_eq!(a.pick(true, true), &FileAction::CopyPath);
    }

    #[test]
    fn wildcards() {
        assert_eq!(wildcard_query("vsc"), Some("*v*s*c*".into()));
        assert_eq!(wildcard_query("ab cd"), Some("*a*b* *c*d*".into()));
        assert_eq!(wildcard_query("ext:pdf tax"), None);
        assert_eq!(wildcard_query("src\\main"), None);
        assert_eq!(fuzzy_text("ext:pdf tax  return"), "tax return");
    }

    #[test]
    fn scoring_prefers_names_shallow_and_clean() {
        let h = History::default();
        let s = |path: &str| {
            let at = path.rfind('\\').unwrap() + 1;
            score("report", path, at, &h).map(|(s, _)| s).unwrap_or(i32::MIN)
        };
        assert!(s("C:\\Users\\me\\report.pdf") > s("C:\\Users\\me\\a\\b\\c\\d\\report.pdf"));
        assert!(s("C:\\Users\\me\\report.pdf") > s("C:\\Users\\me\\AppData\\Local\\report.pdf"));
        assert_eq!(s("C:\\report\\other.txt"), i32::MIN, "matches the name, not the folder");
        let (_, pos) = score("src\\main", "C:\\dev\\src\\main.rs", 11, &h).unwrap();
        assert_eq!(pos, vec![0, 1, 2, 3], "path query highlights within the name");
    }
}
