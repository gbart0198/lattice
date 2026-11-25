//! Ranking: how well a typed query matches an app name (fuzzy, with the matched characters for
//! highlighting), and how much your launch history lifts an app. Pure Rust, no platform code.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A query matched against a name. Higher `score` is better; `positions` are the byte offsets of
/// the matched characters in the name.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub score: i32,
    pub positions: Vec<usize>,
}

const MATCH: i32 = 16;
/// The character starts a word ("Studio" in "Visual Studio", "Shell" in "PowerShell").
const BOUNDARY: i32 = 24;
/// On top of `BOUNDARY`, for the name's very first character.
const FIRST: i32 = 12;
/// The character directly follows the previous match.
const CONSECUTIVE: i32 = 18;
/// Per character skipped between two matches.
const GAP: i32 = 2;
/// Per character before the first match (counted up to `LEADING_CAP`).
const LEADING_GAP: i32 = 1;
const LEADING_CAP: i32 = 15;
const EXACT: i32 = 400;
const PREFIX: i32 = 150;

/// Fuzzy-matches `query` against `name`: every query character must appear in the name, in
/// order, case-insensitively; spaces in the query are ignored. Word starts, runs of consecutive
/// characters, and an exact or prefix match score higher, so "vsc" finds "Visual Studio Code" and
/// "term" puts "Terminal" above "Windows Terminal". An empty query matches everything with 0.
pub fn fuzzy(query: &str, name: &str) -> Option<Match> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).map(lower).collect();
    if q.is_empty() {
        return Some(Match { score: 0, positions: Vec::new() });
    }
    // (byte offset, lowercased char, starts a word) for each character of the name.
    let mut n: Vec<(usize, char, bool)> = Vec::with_capacity(name.len());
    let mut prev: Option<char> = None;
    for (at, c) in name.char_indices() {
        let boundary = match prev {
            None => true,
            Some(p) => {
                !p.is_alphanumeric()
                    || (p.is_lowercase() && c.is_uppercase())
                    || (!p.is_ascii_digit() && c.is_ascii_digit())
            }
        };
        n.push((at, lower(c), boundary));
        prev = Some(c);
    }
    let (m, len) = (q.len(), n.len());
    if m > len || !is_subsequence(&q, &n) {
        return None;
    }

    // score[i * len + j]: the best score with query char i matched at name char j.
    // from[i * len + j]: where query char i - 1 was matched on that best path.
    const NONE: i32 = i32::MIN / 2;
    let mut score = vec![NONE; m * len];
    let mut from = vec![usize::MAX; m * len];
    let bonus = |j: usize| MATCH + if n[j].2 { BOUNDARY } else { 0 } + if j == 0 { FIRST } else { 0 };
    for j in 0..len {
        if n[j].1 == q[0] {
            score[j] = bonus(j) - (j as i32).min(LEADING_CAP) * LEADING_GAP;
        }
    }
    for (i, &qc) in q.iter().enumerate().skip(1) {
        let (prev_row, row) = ((i - 1) * len, i * len);
        // Best of `score[i-1][k] + k * GAP` over k <= j - 2, so a gapped jump from k to j costs
        // `(j - k - 1) * GAP` without a scan over every k.
        let mut run = (NONE, usize::MAX);
        for j in i..len {
            if j >= 2 {
                let k = j - 2;
                let v = score[prev_row + k];
                if v > NONE && v + k as i32 * GAP > run.0 {
                    run = (v + k as i32 * GAP, k);
                }
            }
            if n[j].1 != qc {
                continue;
            }
            let mut best = (NONE, usize::MAX);
            let adjacent = score[prev_row + j - 1];
            if adjacent > NONE {
                best = (adjacent + CONSECUTIVE, j - 1);
            }
            if run.0 > NONE {
                let gapped = run.0 - (j as i32 - 1) * GAP;
                if gapped > best.0 {
                    best = (gapped, run.1);
                }
            }
            if best.0 > NONE {
                score[row + j] = best.0 + bonus(j);
                from[row + j] = best.1;
            }
        }
    }

    let last = (m - 1) * len;
    let (mut j, mut total) = (0, NONE);
    for k in 0..len {
        if score[last + k] > total {
            (j, total) = (k, score[last + k]);
        }
    }
    if total <= NONE {
        return None;
    }
    let mut positions = vec![0; m];
    for i in (0..m).rev() {
        positions[i] = n[j].0;
        if i > 0 {
            j = from[i * len + j];
        }
    }

    let name_lower: String = name.chars().map(lower).collect();
    let query_lower: String = query.trim().chars().map(lower).collect();
    if name_lower == query_lower {
        total += EXACT;
    } else if name_lower.starts_with(&query_lower) {
        total += PREFIX;
    }
    // Between equal matches, the shorter name is the more likely meant.
    total -= (len as i32 - m as i32).min(40) / 4;
    Some(Match { score: total, positions })
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn is_subsequence(q: &[char], n: &[(usize, char, bool)]) -> bool {
    let mut it = n.iter();
    q.iter().all(|c| it.any(|(_, nc, _)| nc == c))
}

// ---------------------------------------------------------------------------------------------
// Launch history

/// How often, and how recently, each app was launched from the launcher, kept in a small text
/// file ("count<TAB>unix-seconds<TAB>app id" per line).
#[derive(Default)]
pub struct History {
    path: Option<PathBuf>,
    used: HashMap<String, (u32, u64)>,
}

impl History {
    /// Reads the history file; a missing or unreadable file is an empty history.
    pub fn load(path: PathBuf) -> Self {
        let mut used = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                let mut parts = line.splitn(3, '\t');
                let (Some(count), Some(last), Some(id)) = (parts.next(), parts.next(), parts.next()) else { continue };
                if let (Ok(count), Ok(last)) = (count.parse(), last.parse()) {
                    used.insert(id.to_string(), (count, last));
                }
            }
        }
        History { path: Some(path), used }
    }

    /// Records a launch of `id` and saves the file.
    pub fn record(&mut self, id: &str) {
        let entry = self.used.entry(id.to_string()).or_insert((0, 0));
        entry.0 = entry.0.saturating_add(1);
        entry.1 = now();
        if let Some(path) = &self.path {
            save(path, &self.used);
        }
    }

    /// Score added to an app's match: grows with use (slowly), plus a lift for recent use.
    pub fn boost(&self, id: &str) -> i32 {
        let Some(&(count, last)) = self.used.get(id) else { return 0 };
        let age = now().saturating_sub(last);
        let recency = if age < 3600 {
            30
        } else if age < 86_400 {
            20
        } else if age < 7 * 86_400 {
            10
        } else {
            0
        };
        ((count.min(100) as f32).sqrt() * 14.0) as i32 + recency
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn save(path: &Path, used: &HashMap<String, (u32, u64)>) {
    let text: String = used.iter().map(|(id, (count, last))| format!("{count}\t{last}\t{id}\n")).collect();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(path, text) {
        log::warn!("launcher history {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(q: &str, name: &str) -> i32 {
        fuzzy(q, name).map(|m| m.score).unwrap_or(i32::MIN)
    }

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(fuzzy("", "Notepad"), Some(Match { score: 0, positions: vec![] }));
    }

    #[test]
    fn missing_characters_do_not_match() {
        assert!(fuzzy("xyz", "Notepad").is_none());
        assert!(fuzzy("notepadd", "Notepad").is_none());
        assert!(fuzzy("dap", "Notepad").is_none(), "order matters");
    }

    #[test]
    fn acronyms_hit_word_starts() {
        let m = fuzzy("vsc", "Visual Studio Code").unwrap();
        assert_eq!(m.positions, vec![0, 7, 14]);
        let m = fuzzy("ps", "PowerShell").unwrap();
        assert_eq!(m.positions, vec![0, 5], "camelCase counts as a word start");
    }

    #[test]
    fn exact_beats_prefix_beats_inner_word() {
        assert!(score("edge", "Edge") > score("edge", "Edge Dev Tools"));
        assert!(score("term", "Terminal") > score("term", "Windows Terminal"));
        assert!(score("code", "Visual Studio Code") > score("code", "Barcode Reader"));
    }

    #[test]
    fn consecutive_beats_scattered() {
        assert!(score("note", "Notepad") > score("note", "Nvidia Overlay Tool Editor"));
    }

    #[test]
    fn case_and_spaces_are_ignored() {
        assert_eq!(fuzzy("VS CODE", "Visual Studio Code").map(|m| m.positions.len()), Some(6));
        assert!(fuzzy("FIREFOX", "Firefox").unwrap().score > 400);
    }

    #[test]
    fn positions_are_byte_offsets() {
        let m = fuzzy("café", "Café Menu").unwrap();
        assert_eq!(m.positions, vec![0, 1, 2, 3]);
        let m = fuzzy("menu", "Café Menu").unwrap();
        assert_eq!(m.positions, vec![6, 7, 8, 9], "é is two bytes");
    }

    #[test]
    fn history_round_trips_and_boosts() {
        let dir = std::env::temp_dir().join(format!("lattice-rank-test-{}", std::process::id()));
        let path = dir.join("history.tsv");
        let mut h = History::load(path.clone());
        assert_eq!(h.boost("a!App"), 0);
        h.record("a!App");
        h.record("a!App");
        let again = History::load(path);
        assert!(again.boost("a!App") > 0);
        assert_eq!(again.boost("other"), 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
