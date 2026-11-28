//! What the file index needs from Windows.

/// Hidden or system files and folders, which the index skips. Cheap on Windows: the attributes
/// come with the directory listing.
pub fn hidden(entry: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    entry.metadata().is_ok_and(|m| m.file_attributes() & (HIDDEN | SYSTEM) != 0)
}

/// Low CPU and disk priority for the calling thread, so a cold walk doesn't slow the desktop.
pub fn background_priority() {
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
}

/// The user's home folder (%USERPROFILE%), as shown shortened in results.
pub fn home() -> Option<String> {
    std::env::var("USERPROFILE").ok().filter(|h| !h.is_empty())
}
