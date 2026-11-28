//! Window search: the host's open windows, ranked by fuzzy match on title and app, most
//! recently used first. Pure Rust; the host lists the windows and does the switching.

use std::sync::Arc;

use super::rank;

/// An open window, as the host lists it for window search.
#[derive(Clone, Debug)]
pub struct WindowEntry {
    /// Handed back to the host's focus callback.
    pub hwnd: isize,
    pub title: String,
    /// The app's name ("firefox", "WindowsTerminal").
    pub app: String,
    /// 1-based workspace and monitor numbers.
    pub workspace: usize,
    pub monitor: usize,
    pub floating: bool,
    pub minimized: bool,
    /// The window you're in now: listed last, so Enter on an empty search flips back to the
    /// one before it, as Alt+Tab does.
    pub current: bool,
    pub icon: Option<WindowIcon>,
}

/// A window's icon: square straight-alpha RGBA. Windows with the same pixels share a `key`, so
/// ten terminals decode one image.
#[derive(Clone, Debug)]
pub struct WindowIcon {
    pub key: u64,
    pub size: u32,
    pub rgba: Arc<[u8]>,
}

impl WindowIcon {
    /// Its id in the launcher's icon cache (beside app ids and file paths).
    pub(super) fn id(&self) -> Arc<str> {
        format!("window-icon:{:x}", self.key).into()
    }
}

/// One result: an index into the window list, and which field matched where.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct WindowHit {
    pub window: usize,
    /// Byte offsets of matched characters in the title, or in the app name; only the better
    /// matching field is highlighted, so a strong app match doesn't paint stray title letters.
    pub title: Vec<usize>,
    pub app: Vec<usize>,
}

/// The best `max` windows for `query`, best first. `windows` comes most recently used first;
/// equal scores keep that order, with the current window last.
pub(super) fn rank(query: &str, windows: &[WindowEntry], max: usize) -> Vec<WindowHit> {
    let mut scored: Vec<(i32, WindowHit)> = windows
        .iter()
        .enumerate()
        .filter_map(|(i, w)| {
            let title = rank::fuzzy(query, &w.title);
            let app = rank::fuzzy(query, &w.app);
            let hit = |title: Vec<usize>, app: Vec<usize>| WindowHit { window: i, title, app };
            match (title, app) {
                (Some(t), Some(a)) if a.score > t.score => Some((a.score, hit(Vec::new(), a.positions))),
                (Some(t), _) => Some((t.score, hit(t.positions, Vec::new()))),
                (None, Some(a)) => Some((a.score, hit(Vec::new(), a.positions))),
                (None, None) => None,
            }
        })
        .collect();
    // Stable sorts: by recency (the list's order) with the current window sunk, then by score.
    scored.sort_by_key(|(_, h)| windows[h.window].current);
    scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
    scored.truncate(max);
    scored.into_iter().map(|(_, h)| h).collect()
}

/// The line under a window's title: its app, and where it is when that's worth saying.
pub(super) fn detail(w: &WindowEntry, monitors: bool) -> String {
    let mut parts = vec![w.app.clone()];
    if monitors {
        parts.push(format!("Monitor {}", w.monitor));
    }
    if w.minimized {
        parts.push("Minimized".into());
    } else if w.floating {
        parts.push("Floating".into());
    }
    parts.join("  ·  ")
}

/// The windows sit on more than one monitor, so rows say which.
pub(super) fn many_monitors(windows: &[WindowEntry]) -> bool {
    windows.first().is_some_and(|f| windows.iter().any(|w| w.monitor != f.monitor))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(hwnd: isize, title: &str, app: &str) -> WindowEntry {
        WindowEntry {
            hwnd,
            title: title.into(),
            app: app.into(),
            workspace: 1,
            monitor: 1,
            floating: false,
            minimized: false,
            current: false,
            icon: None,
        }
    }

    fn order(hits: &[WindowHit], windows: &[WindowEntry]) -> Vec<isize> {
        hits.iter().map(|h| windows[h.window].hwnd).collect()
    }

    #[test]
    fn empty_query_lists_by_recency_current_last() {
        let mut list = vec![win(1, "README.md - lattice", "Code"), win(2, "Inbox", "firefox"), win(3, "~", "WindowsTerminal")];
        list[0].current = true;
        assert_eq!(order(&rank("", &list, 10), &list), vec![2, 3, 1]);
        assert_eq!(order(&rank("  ", &list, 2), &list), vec![2, 3]);
    }

    #[test]
    fn matches_title_or_app_highlighting_the_better() {
        let list = vec![win(1, "Inbox - Mail", "firefox"), win(2, "Firefox Privacy Notice", "notepad")];
        let hits = rank("firefox", &list, 10);
        // Both match exactly-ish; the app name is an exact match, so window 1 wins.
        assert_eq!(order(&hits, &list), vec![1, 2]);
        assert!(hits[0].title.is_empty() && hits[0].app.len() == 7);
        assert!(hits[1].app.is_empty() && hits[1].title.len() == 7);
        assert!(rank("zzz", &list, 10).is_empty());
    }

    #[test]
    fn ties_keep_recency() {
        let list = vec![win(1, "a", "term"), win(2, "b", "term"), win(3, "c", "term")];
        assert_eq!(order(&rank("term", &list, 10), &list), vec![1, 2, 3]);
    }

    #[test]
    fn details() {
        let mut w = win(1, "t", "firefox");
        assert_eq!(detail(&w, false), "firefox");
        w.floating = true;
        w.monitor = 2;
        assert_eq!(detail(&w, true), "firefox  ·  Monitor 2  ·  Floating");
        w.minimized = true;
        assert_eq!(detail(&w, false), "firefox  ·  Minimized");
        let mut list = vec![win(1, "a", "x"), win(2, "b", "x")];
        assert!(!many_monitors(&list));
        list[1].monitor = 2;
        assert!(many_monitors(&list));
    }
}
