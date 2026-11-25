//! Update checks against the GitHub releases of this repo, and installing a newer release.
//!
//! The WM process checks once a day (if `general.check_for_updates` is on and this is the
//! installed copy) and a toast at the bottom right offers it when a newer release is out
//! (`update_toast`). The settings window has "Check now". Nothing is downloaded or run until the user clicks.
//!
//! Trust: everything goes over HTTPS (Windows' own TLS and certificate store, plain HTTP is
//! refused, redirects included). The installer has to be the release's
//! `lattice-setup-<version>.exe` at the exact github.com download URL for that tag, and its
//! SHA-256 has to match the digest GitHub reports for the asset. Only strictly newer
//! versions are offered, so an old release can't be pushed as an "update".

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use sha2::{Digest, Sha256};

const REPO: &str = "gbart0198/lattice";
/// Inno Setup's uninstall key for the installer's AppId (installer/lattice.iss).
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{6F3A9C1E-2B7D-4E8A-9C5F-1D2E3B4A5C6D}_is1";
/// Bigger than any installer we ship (about 10 MB today); stops an endless download.
const MAX_INSTALLER: u64 = 64 * 1024 * 1024;
const MAX_API_RESPONSE: u64 = 1024 * 1024;
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// Temp folders the downloads go in, cleared on the next start.
const TEMP_PREFIX: &str = "lattice-update-";

pub type Version = (u64, u64, u64);

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: Version,
    url: String,
    sha256: [u8; 32],
    size: u64,
}

impl Release {
    /// A made-up release for UI previews; it can't be installed.
    pub fn preview(version: Version) -> Self {
        Release { version, url: String::new(), sha256: [0; 32], size: 0 }
    }

    pub fn version_str(&self) -> String {
        let (a, b, c) = self.version;
        format!("{a}.{b}.{c}")
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum Status {
    #[default]
    Unknown,
    Checking,
    UpToDate,
    CheckFailed(String),
    Available(Release),
    Downloading(Release),
    /// The installer is running; it closes lattice and starts the new one.
    Installing(Release),
    InstallFailed(Release, String),
}

static STATUS: Mutex<Status> = Mutex::new(Status::Unknown);
/// Bumped on every status change, for the UI's poll.
pub static GEN: AtomicU64 = AtomicU64::new(0);
static WAKER: OnceLock<rune::Waker> = OnceLock::new();

/// Shows `s` in the UI, for previews. Nothing is checked or installed.
pub fn preview(s: Status) {
    PREVIEW.store(true, Ordering::SeqCst);
    set_status(s);
}

pub fn is_preview() -> bool {
    PREVIEW.load(Ordering::SeqCst)
}

static PREVIEW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Calls `w` whenever the status changes (one listener per process: the update toast in the
/// WM, the settings window in its own process).
pub fn set_waker(w: rune::Waker) {
    let _ = WAKER.set(w);
}

pub fn status() -> Status {
    STATUS.lock().unwrap().clone()
}

fn set_status(s: Status) {
    *STATUS.lock().unwrap() = s;
    GEN.fetch_add(1, Ordering::SeqCst);
    if let Some(w) = WAKER.get() {
        w();
    }
}

pub fn current() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).expect("Cargo version is x.y.z")
}

/// `"1.2.3"` or `"v1.2.3"`; anything else (pre-releases included) is None.
pub fn parse_version(s: &str) -> Option<Version> {
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = s.split('.').map(|p| {
        if p.is_empty() || p.len() > 9 || !p.bytes().all(|b| b.is_ascii_digit()) {
            None
        } else {
            p.parse().ok()
        }
    });
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

// ---------------------------------------------------------------------------------------------
// GitHub

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(5)
        .timeout_global(Some(Duration::from_secs(120)))
        // GitHub's API wants a User-Agent; keep it generic.
        .user_agent(concat!("lattice-updater/", env!("CARGO_PKG_VERSION")))
        .tls_config(TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build())
        .build()
        .into()
}

/// The newest release, if it's newer than this build.
fn fetch_latest() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut resp = agent()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("couldn't reach GitHub: {e}"))?;
    let rel: ApiRelease = resp
        .body_mut()
        .with_config()
        .limit(MAX_API_RESPONSE)
        .read_json()
        .map_err(|e| format!("unexpected reply from GitHub: {e}"))?;
    validate(rel, current())
}

/// Checks a release against what we're willing to install: a newer x.y.z tag, with exactly
/// our installer asset at its expected URL and a SHA-256 digest.
fn validate(rel: ApiRelease, current: Version) -> Result<Option<Release>, String> {
    if rel.draft || rel.prerelease {
        return Ok(None);
    }
    let Some(version) = parse_version(&rel.tag_name) else {
        return Err(format!("latest release tag \"{}\" isn't a version", rel.tag_name));
    };
    if version <= current {
        return Ok(None);
    }
    let (a, b, c) = version;
    let name = format!("lattice-setup-{a}.{b}.{c}.exe");
    let asset = rel
        .assets
        .iter()
        .find(|x| x.name == name)
        .ok_or_else(|| format!("release {} has no {name}", rel.tag_name))?;
    let expected_url = format!("https://github.com/{REPO}/releases/download/{}/{name}", rel.tag_name);
    if asset.browser_download_url != expected_url {
        return Err(format!("{name} has an unexpected download URL: {}", asset.browser_download_url));
    }
    if asset.size == 0 || asset.size > MAX_INSTALLER {
        return Err(format!("{name} has an unexpected size ({} bytes)", asset.size));
    }
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .and_then(parse_hex32)
        .ok_or_else(|| format!("{name} has no SHA-256 digest on GitHub"))?;
    Ok(Some(Release { version, url: expected_url, sha256, size: asset.size }))
}

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Downloads the installer into a new temp folder and checks its hash.
fn download(rel: &Release) -> Result<PathBuf, String> {
    let mut resp = agent().get(&rel.url).call().map_err(|e| format!("download failed: {e}"))?;
    let bytes = resp
        .body_mut()
        .with_config()
        .limit(MAX_INSTALLER)
        .read_to_vec()
        .map_err(|e| format!("download failed: {e}"))?;
    verify(&bytes, rel)?;
    // A folder of our own (create_dir fails if it exists), and the file is created new in it.
    let nonce = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("{TEMP_PREFIX}{}-{nonce}", std::process::id()));
    std::fs::create_dir(&dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    let path = dir.join(format!("lattice-setup-{}.exe", rel.version_str()));
    use std::io::Write;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|mut f| f.write_all(&bytes))
        .map_err(|e| format!("couldn't save {}: {e}", path.display()))?;
    Ok(path)
}

fn verify(bytes: &[u8], rel: &Release) -> Result<(), String> {
    if bytes.len() as u64 != rel.size {
        return Err(format!("download is {} bytes, expected {}", bytes.len(), rel.size));
    }
    if Sha256::digest(bytes).as_slice() != rel.sha256 {
        return Err("download doesn't match GitHub's SHA-256 digest".into());
    }
    Ok(())
}

/// Starts the installer: no wizard, a progress window, and lattice restarted at the end
/// (`/UPDATE`, see installer/lattice.iss). The installer closes this lattice cleanly itself.
fn launch(installer: &Path) -> Result<(), String> {
    let log = installer.with_file_name("install.log");
    std::process::Command::new(installer)
        .args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/NOCANCEL", "/UPDATE"])
        .arg(format!("/LOG={}", log.display()))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("couldn't start the installer: {e}"))
}

// ---------------------------------------------------------------------------------------------
// Installed copy

/// Whether this exe is the one the installer put in place (so updating it makes sense).
/// A `cargo run` build or a copy elsewhere isn't.
pub fn installed() -> bool {
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) else {
        return false;
    };
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
    let here = norm(&dir);
    if install_location().is_some_and(|l| norm(&l) == here) {
        return true;
    }
    // install.ps1's location (the installer upgrades it in place).
    std::env::var_os("LOCALAPPDATA").is_some_and(|l| norm(&Path::new(&l).join(r"Programs\lattice")) == here)
}

fn install_location() -> Option<PathBuf> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::{HSTRING, w};
    let key = HSTRING::from(UNINSTALL_KEY);
    let mut buf = [0u16; 1024];
    let mut len = std::mem::size_of_val(&buf) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            w!("InstallLocation"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if rc.is_err() {
        return None;
    }
    let chars = (len as usize / 2).min(buf.len());
    let s = String::from_utf16_lossy(&buf[..chars]);
    let s = s.trim_end_matches('\0');
    (!s.is_empty()).then(|| PathBuf::from(s))
}

// ---------------------------------------------------------------------------------------------
// Actions

/// Checks GitHub on a background thread (no-op if a check or install is under way).
pub fn check_async() {
    {
        let mut s = STATUS.lock().unwrap();
        if matches!(*s, Status::Checking | Status::Downloading(_) | Status::Installing(_)) {
            return;
        }
        *s = Status::Checking;
    }
    GEN.fetch_add(1, Ordering::SeqCst);
    if let Some(w) = WAKER.get() {
        w();
    }
    let _ = std::thread::Builder::new().name("lattice-update-check".into()).spawn(check_now);
}

fn check_now() {
    let s = match fetch_latest() {
        Ok(Some(rel)) => {
            log::info!("update available: {} (running {})", rel.version_str(), env!("CARGO_PKG_VERSION"));
            Status::Available(rel)
        }
        Ok(None) => Status::UpToDate,
        Err(e) => {
            log::warn!("update check: {e}");
            Status::CheckFailed(e)
        }
    };
    set_status(s);
}

/// Downloads, verifies and runs the installer for the release on offer (after a failed
/// attempt, tries again). Does nothing otherwise, or if this isn't the installed copy.
pub fn install_async() {
    let rel = {
        let mut s = STATUS.lock().unwrap();
        let rel = match &*s {
            Status::Available(r) | Status::InstallFailed(r, _) => r.clone(),
            _ => return,
        };
        if !installed() || is_preview() {
            return;
        }
        *s = Status::Downloading(rel.clone());
        rel
    };
    GEN.fetch_add(1, Ordering::SeqCst);
    if let Some(w) = WAKER.get() {
        w();
    }
    let _ = std::thread::Builder::new().name("lattice-update-install".into()).spawn(move || {
        log::info!("updating to {}", rel.version_str());
        match download(&rel).and_then(|p| launch(&p)) {
            Ok(()) => set_status(Status::Installing(rel)),
            Err(e) => {
                log::error!("update to {}: {e}", rel.version_str());
                set_status(Status::InstallFailed(rel, e));
            }
        }
    });
}

/// The WM's daily check: first a little after start, then every 24 hours of wall-clock time
/// (so a machine that slept through a day checks when it wakes). Reads the config each time,
/// so turning checks off in settings takes effect without a restart.
pub fn start_background() {
    if !installed() {
        log::info!("not the installed copy; automatic update checks are off");
        return;
    }
    let _ = std::thread::Builder::new().name("lattice-update".into()).spawn(|| {
        std::thread::sleep(Duration::from_secs(30));
        clean_old_downloads();
        let mut last: Option<SystemTime> = None;
        loop {
            let due = last.is_none_or(|t| t.elapsed().is_ok_and(|e| e >= CHECK_EVERY));
            if due && crate::config::load().general.check_for_updates {
                last = Some(SystemTime::now());
                check_async();
            }
            std::thread::sleep(Duration::from_secs(60 * 60));
        }
    });
}

/// Removes installers earlier updates downloaded.
fn clean_old_downloads() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for e in entries.flatten() {
        if e.file_name().to_string_lossy().starts_with(TEMP_PREFIX) && e.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "sha256:1e20c4236abef89e354b0a17376b87f82698786175cc3b941a3e828cb9a8ff5d";

    fn release(tag: &str, name: &str, url: &str, digest: Option<&str>) -> ApiRelease {
        ApiRelease {
            tag_name: tag.into(),
            draft: false,
            prerelease: false,
            assets: vec![ApiAsset { name: name.into(), browser_download_url: url.into(), size: 10_000_000, digest: digest.map(Into::into) }],
        }
    }

    fn good(v: &str) -> ApiRelease {
        let url = format!("https://github.com/{REPO}/releases/download/v{v}/lattice-setup-{v}.exe");
        release(&format!("v{v}"), &format!("lattice-setup-{v}.exe"), &url, Some(DIGEST))
    }

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.6.1"), Some((0, 6, 1)));
        assert_eq!(parse_version("1.10.0"), Some((1, 10, 0)));
        assert_eq!(parse_version("v1.0.0-rc1"), None);
        assert_eq!(parse_version("1.0"), None);
        assert_eq!(parse_version("1.0.0.0"), None);
        assert_eq!(parse_version("v+1.0.0"), None);
        assert_eq!(parse_version(""), None);
        assert!(parse_version("1.10.0") > parse_version("1.9.9"));
        assert_eq!(parse_version(env!("CARGO_PKG_VERSION")), Some(current()));
    }

    #[test]
    fn newer_release_is_offered() {
        let r = validate(good("0.7.0"), (0, 6, 1)).unwrap().unwrap();
        assert_eq!(r.version, (0, 7, 0));
        assert_eq!(r.url, format!("https://github.com/{REPO}/releases/download/v0.7.0/lattice-setup-0.7.0.exe"));
        assert_eq!(r.sha256[0], 0x1e);
    }

    #[test]
    fn same_or_older_is_not() {
        assert_eq!(validate(good("0.6.1"), (0, 6, 1)), Ok(None));
        assert_eq!(validate(good("0.5.0"), (0, 6, 1)), Ok(None));
        let mut pre = good("0.9.0");
        pre.prerelease = true;
        assert_eq!(validate(pre, (0, 6, 1)), Ok(None));
    }

    #[test]
    fn suspicious_assets_are_refused() {
        let cur = (0, 6, 1);
        let v = "0.7.0";
        let name = format!("lattice-setup-{v}.exe");
        let url = format!("https://github.com/{REPO}/releases/download/v{v}/{name}");
        // Elsewhere, over HTTP, or from another repo.
        for bad in [
            format!("https://evil.example/{name}"),
            url.replace("https://", "http://"),
            url.replace(REPO, "someone/lattice"),
        ] {
            assert!(validate(release("v0.7.0", &name, &bad, Some(DIGEST)), cur).is_err(), "{bad}");
        }
        // No digest, or not a SHA-256 one.
        assert!(validate(release("v0.7.0", &name, &url, None), cur).is_err());
        assert!(validate(release("v0.7.0", &name, &url, Some("md5:abc")), cur).is_err());
        // Not our installer's name.
        assert!(validate(release("v0.7.0", "setup.exe", &url, Some(DIGEST)), cur).is_err());
        // Oversized.
        let mut big = good(v);
        big.assets[0].size = MAX_INSTALLER + 1;
        assert!(validate(big, cur).is_err());
    }

    /// Talks to GitHub: `cargo test --bin lattice live_ -- --ignored`. Treats the latest
    /// release as new and downloads and verifies its installer (without running it).
    #[test]
    #[ignore]
    fn live_latest_release_downloads_and_verifies() {
        let mut resp = agent().get(&format!("https://api.github.com/repos/{REPO}/releases/latest")).call().unwrap();
        let rel: ApiRelease = resp.body_mut().with_config().limit(MAX_API_RESPONSE).read_json().unwrap();
        let rel = validate(rel, (0, 0, 0)).unwrap().expect("a release");
        let path = download(&rel).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), rel.size);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        // Plain HTTP is refused outright.
        assert!(agent().get("http://github.com/").call().is_err());
    }

    #[test]
    fn verify_checks_size_and_hash() {
        let bytes = b"installer".to_vec();
        let mut sha256 = [0u8; 32];
        sha256.copy_from_slice(&Sha256::digest(&bytes));
        let rel = Release { version: (9, 9, 9), url: String::new(), sha256, size: bytes.len() as u64 };
        assert!(verify(&bytes, &rel).is_ok());
        assert!(verify(b"installes", &rel).is_err());
        assert!(verify(b"installer!", &rel).is_err());
    }
}
