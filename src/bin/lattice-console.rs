//! The terminal's `lattice`: installed as `lattice.com` next to `lattice.exe`.
//!
//! lattice.exe is a GUI program (no console flashes up when it starts at sign-in), so a shell
//! doesn't wait for it: `lattice query windows` would print after the prompt came back and
//! `$LASTEXITCODE` would mean nothing. Windows runs `.com` before `.exe` when both match a
//! bare `lattice`, so terminals and scripts land here instead: a console program that runs
//! lattice.exe with the same arguments and the same stdin/stdout/stderr, waits for it, and
//! exits with its exit code.

fn main() {
    let exe = match std::env::current_exe() {
        Ok(p) => p.with_file_name("lattice.exe"),
        Err(e) => {
            eprintln!("lattice: {e}");
            std::process::exit(1);
        }
    };
    let status = std::process::Command::new(&exe).args(std::env::args_os().skip(1)).status();
    match status {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("lattice: couldn't start {}: {e}", exe.display());
            std::process::exit(1);
        }
    }
}
