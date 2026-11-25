//! lattice's own file index, for when Everything isn't running: every file and folder under a
//! few roots (your user folder by default), walked on a low-priority background thread, kept in
//! memory and saved to disk so the next start has it straight away.
//!
//! A search never waits for a walk. The first walk ever publishes what it has found as it goes;
//! later walks build a fresh list and swap it in when done, searches using the old one meanwhile.

use std::io::{BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// One file or folder.
pub struct Entry {
    pub path: Box<str>,
    /// Byte offset of the file name in `path`.
    pub name_at: u32,
    pub dir: bool,
}

impl Entry {
    pub fn name(&self) -> &str {
        &self.path[self.name_at as usize..]
    }
}

/// A walk this old is redone when file search is next used.
const STALE_AFTER: Duration = Duration::from_secs(10 * 60);
/// Enough for any reasonable set of roots; past it the walk stops rather than eat memory.
const MAX_ENTRIES: usize = 2_000_000;
/// How many entries the first walk gathers between publishing them.
const BATCH: usize = 8192;
const CACHE_HEADER: &str = "lattice-file-index 1";

pub struct Index {
    entries: RwLock<Vec<Entry>>,
    roots: Vec<PathBuf>,
    /// Folder names never walked into, lowercased.
    exclude: Vec<String>,
    cache: PathBuf,
    walking: AtomicBool,
    /// Entries found by the walk in progress.
    found: AtomicUsize,
    last_walk: Mutex<Option<Instant>>,
    loaded_cache: AtomicBool,
}

/// What the index can say about itself, for the launcher's status line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Status {
    pub walking: bool,
    /// Entries searchable now.
    pub entries: usize,
    /// Entries found by the walk in progress.
    pub found: usize,
}

impl Index {
    pub fn new(roots: Vec<PathBuf>, exclude: &[String], cache: PathBuf) -> Arc<Index> {
        Arc::new(Index {
            entries: RwLock::new(Vec::new()),
            roots,
            exclude: exclude.iter().map(|e| e.to_lowercase()).collect(),
            cache,
            walking: AtomicBool::new(false),
            found: AtomicUsize::new(0),
            last_walk: Mutex::new(None),
            loaded_cache: AtomicBool::new(false),
        })
    }

    pub fn status(&self) -> Status {
        Status {
            walking: self.walking.load(Ordering::SeqCst),
            entries: self.entries.read().unwrap().len(),
            found: self.found.load(Ordering::SeqCst),
        }
    }

    /// Loads the saved index if this is the first call, then starts a walk unless one is
    /// running or the last finished recently. `progress` is called (from the walk's thread) as
    /// entries become searchable and when the walk ends.
    pub fn refresh(self: &Arc<Self>, progress: impl Fn() + Send + 'static) {
        if !self.loaded_cache.swap(true, Ordering::SeqCst) {
            let started = Instant::now();
            if let Some(entries) = load_cache(&self.cache) {
                log::info!("launcher: {} indexed files from cache in {:?}", entries.len(), started.elapsed());
                *self.entries.write().unwrap() = entries;
            }
        }
        if self.last_walk.lock().unwrap().is_some_and(|t| t.elapsed() < STALE_AFTER) {
            return;
        }
        if self.walking.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("lattice-file-index".into()).spawn(move || {
            background_priority();
            me.walk(&progress);
            *me.last_walk.lock().unwrap() = Some(Instant::now());
            me.walking.store(false, Ordering::SeqCst);
            progress();
        });
        if spawned.is_err() {
            self.walking.store(false, Ordering::SeqCst);
        }
    }

    fn walk(&self, progress: &dyn Fn()) {
        let started = Instant::now();
        self.found.store(0, Ordering::SeqCst);
        // With nothing searchable yet, publish as we go; otherwise swap in at the end.
        let incremental = self.entries.read().unwrap().is_empty();
        let mut fresh: Vec<Entry> = Vec::new();
        let mut published = 0;
        let mut stack: Vec<PathBuf> = self.roots.iter().filter(|r| r.is_dir()).cloned().collect();
        while let Some(dir) = stack.pop() {
            let Ok(read) = std::fs::read_dir(&dir) else { continue };
            for entry in read.flatten() {
                let Ok(kind) = entry.file_type() else { continue };
                // Junctions and symlinks: their targets are walked (or excluded) on their own.
                if kind.is_symlink() || hidden(&entry) {
                    continue;
                }
                let path = entry.path();
                let is_dir = kind.is_dir();
                if is_dir {
                    let name = entry.file_name().to_string_lossy().to_lowercase();
                    if self.exclude.contains(&name) {
                        continue;
                    }
                    stack.push(path.clone());
                }
                if let Some(e) = make_entry(&path, is_dir) {
                    fresh.push(e);
                }
            }
            self.found.store(published + fresh.len(), Ordering::SeqCst);
            if incremental && fresh.len() >= BATCH {
                published += fresh.len();
                self.entries.write().unwrap().append(&mut fresh);
                progress();
            }
            if published + fresh.len() >= MAX_ENTRIES {
                log::warn!("launcher: file index stopped at {MAX_ENTRIES} entries; narrow index_roots");
                break;
            }
        }
        if incremental {
            self.entries.write().unwrap().append(&mut fresh);
        } else {
            *self.entries.write().unwrap() = fresh;
        }
        let entries = self.entries.read().unwrap();
        log::info!("launcher: indexed {} files in {:?}", entries.len(), started.elapsed());
        if let Err(e) = save_cache(&self.cache, &entries) {
            log::warn!("launcher: saving file index {}: {e}", self.cache.display());
        }
    }

    /// Calls `f` with every entry, until it returns false. Holds the read lock throughout.
    pub fn scan(&self, mut f: impl FnMut(&Entry) -> bool) {
        for e in self.entries.read().unwrap().iter() {
            if !f(e) {
                break;
            }
        }
    }
}

fn make_entry(path: &Path, dir: bool) -> Option<Entry> {
    let path = path.to_str()?;
    let name_at = path.rfind('\\').map_or(0, |i| i + 1);
    Some(Entry { path: path.into(), name_at: name_at as u32, dir })
}

#[cfg(windows)]
fn hidden(entry: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    // Cheap on Windows: the attributes come with the directory listing.
    entry.metadata().is_ok_and(|m| m.file_attributes() & (HIDDEN | SYSTEM) != 0)
}

#[cfg(not(windows))]
fn hidden(entry: &std::fs::DirEntry) -> bool {
    entry.file_name().to_string_lossy().starts_with('.')
}

/// Low CPU and disk priority for the walking thread, so a cold walk doesn't slow the desktop.
fn background_priority() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
}

/// The saved index: a header line, then "d<TAB>path" or "f<TAB>path" per entry.
fn load_cache(path: &Path) -> Option<Vec<Entry>> {
    let file = std::fs::File::open(path).ok()?;
    let mut lines = std::io::BufReader::new(file).lines();
    if lines.next()?.ok()? != CACHE_HEADER {
        return None;
    }
    let mut entries = Vec::new();
    for line in lines {
        let line = line.ok()?;
        let Some((kind, p)) = line.split_once('\t') else { continue };
        if let Some(e) = make_entry(Path::new(p), kind == "d") {
            entries.push(e);
        }
    }
    Some(entries)
}

fn save_cache(path: &Path, entries: &[Entry]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Write beside it and rename, so a crash mid-write never leaves half an index.
    let tmp = path.with_extension("tmp");
    let mut out = BufWriter::new(std::fs::File::create(&tmp)?);
    writeln!(out, "{CACHE_HEADER}")?;
    for e in entries {
        writeln!(out, "{}\t{}", if e.dir { 'd' } else { 'f' }, e.path)?;
    }
    out.flush()?;
    drop(out);
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_saves_and_reloads() {
        let root = std::env::temp_dir().join(format!("lattice-index-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("docs").join("deep")).unwrap();
        std::fs::create_dir_all(root.join("node_modules").join("pkg")).unwrap();
        std::fs::write(root.join("docs").join("deep").join("report.pdf"), b"").unwrap();
        std::fs::write(root.join("node_modules").join("pkg").join("index.js"), b"").unwrap();
        let cache = root.join("cache").join("index.txt");

        let index = Index::new(vec![root.clone()], &["Node_Modules".to_string()], cache.clone());
        index.walk(&|| {});
        let mut names = Vec::new();
        index.scan(|e| {
            names.push((e.name().to_string(), e.dir));
            true
        });
        names.sort();
        assert!(names.contains(&("report.pdf".to_string(), false)));
        assert!(names.contains(&("deep".to_string(), true)));
        assert!(!names.iter().any(|(n, _)| n == "index.js" || n == "node_modules"), "excluded: {names:?}");

        let reloaded = load_cache(&cache).unwrap();
        assert_eq!(reloaded.len(), names.len());
        assert!(reloaded.iter().any(|e| e.name() == "report.pdf" && !e.dir));
        let _ = std::fs::remove_dir_all(&root);
    }
}
