//! Other lattice processes talking to the running lattice: not written yet on macOS (it will
//! go over the same socket as `ipc`). Until then a running lattice is never found, so the tour
//! runs on its own and settings changes apply on the next start.

pub struct Lattice;

pub fn find() -> Option<Lattice> {
    super::unsupported("talking to the running lattice (tour, settings)");
    None
}

impl Lattice {
    pub fn tour_status(&self, _running: bool) -> Option<u64> {
        None
    }

    pub fn tour_rect(&self, _anchor: usize) -> Option<u64> {
        None
    }

    pub fn reload(&self) -> bool {
        false
    }

    pub fn quit(&self) {}
}
