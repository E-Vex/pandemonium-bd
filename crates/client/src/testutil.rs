//! A collision-free temp directory for the client's unit tests.
//!
//! The timestamp-only names the pack and texture suites used ("nanos since
//! the epoch") collide on Windows, where the wall clock's granularity is
//! coarse enough to put two parallel tests in the same "nanosecond" — the
//! pack manifest race CI caught on B-026's push run (two tests drew one
//! directory and each loaded the other's manifest). The process id plus a
//! process-local atomic counter are unique by construction; the timestamp
//! stays for across-run tidiness.
//!
//! Test-only code — the module is compiled behind `cfg(test)`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// A fresh, uniquely-named directory under the system temp dir (created).
pub(crate) fn temp_dir(prefix: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "{prefix}-{}-{nanos}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
    ));
    std::fs::create_dir_all(&dir).expect("creating temp dir");
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapid_calls_never_collide() {
        // The race this module exists to kill: two back-to-back calls (the
        // coarser the clock, the closer they land) must draw two distinct
        // directories — the counter guarantees it even when the timestamp
        // repeats.
        let a = temp_dir("pandemonium-testutil-probe");
        let b = temp_dir("pandemonium-testutil-probe");
        assert_ne!(a, b);
    }
}
