//! Minimal logger: stderr plus %APPDATA%\lattice\lattice.log (truncated on each start).
//! `LATTICE_LOG=debug` turns on debug output.

use std::fs::File;
use std::io::Write;
use std::sync::Mutex;

struct Logger {
    file: Option<Mutex<File>>,
    level: log::LevelFilter,
}

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= self.level && m.target().starts_with("lattice")
    }

    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        let line = format!(
            "{:02}:{:02}:{:02}.{:03} {:5} {}\n",
            t.wHour,
            t.wMinute,
            t.wSecond,
            t.wMilliseconds,
            r.level(),
            r.args()
        );
        eprint!("{line}");
        if let Some(f) = &self.file
            && let Ok(mut f) = f.lock()
        {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {}
}

pub fn init() {
    let dir = crate::config::config_dir();
    let _ = std::fs::create_dir_all(&dir);
    let file = File::create(dir.join("lattice.log")).ok().map(Mutex::new);
    let level = match std::env::var("LATTICE_LOG").as_deref() {
        Ok("debug") | Ok("trace") => log::LevelFilter::Debug,
        _ => log::LevelFilter::Info,
    };
    let logger = Box::leak(Box::new(Logger { file, level }));
    let _ = log::set_logger(logger);
    log::set_max_level(level);
}
