//! What the file index needs on macOS.

/// Dotfiles and dot-folders, which the index skips.
pub fn hidden(entry: &std::fs::DirEntry) -> bool {
    entry.file_name().to_string_lossy().starts_with('.')
}

/// Not written yet (`setpriority`/QoS): the walk runs at normal priority.
pub fn background_priority() {}

pub fn home() -> Option<String> {
    std::env::var("HOME").ok().filter(|h| !h.is_empty())
}
