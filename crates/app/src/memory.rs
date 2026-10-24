//! How much RAM this process holds, for the top bar.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How often the number is read again.
const EVERY: Duration = Duration::from_secs(2);

/// The resident memory of this process, read on a thread every few
/// seconds, so the frame never waits for it.
pub struct Memory {
    bytes: Arc<AtomicU64>,
    reading: Arc<AtomicBool>,
    last: Option<Instant>,
}

impl Memory {
    pub fn new() -> Memory {
        Memory { bytes: Arc::new(AtomicU64::new(0)), reading: Arc::new(AtomicBool::new(false)), last: None }
    }

    /// The last number read, or None before the first read. Starts a new
    /// read when the last one is old.
    pub fn bytes(&mut self) -> Option<u64> {
        if self.last.is_none_or(|t| t.elapsed() >= EVERY) && !self.reading.swap(true, Ordering::SeqCst) {
            self.last = Some(Instant::now());
            let (bytes, reading) = (self.bytes.clone(), self.reading.clone());
            std::thread::spawn(move || {
                if let Some(b) = resident_bytes() {
                    bytes.store(b, Ordering::SeqCst);
                }
                reading.store(false, Ordering::SeqCst);
            });
        }
        Some(self.bytes.load(Ordering::SeqCst)).filter(|&b| b > 0)
    }
}

/// `312 MB`, or `1.4 GB` from 1000 MB up.
pub fn label(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1000.0 { format!("{:.1} GB", mb / 1024.0) } else { format!("{mb:.0} MB") }
}

#[cfg(target_os = "linux")]
fn resident_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

#[cfg(not(target_os = "linux"))]
fn resident_bytes() -> Option<u64> {
    let pid = std::process::id().to_string();
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &pid]).output().ok()?;
    let kb: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(kb * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_megabytes_and_gigabytes() {
        assert_eq!(label(312 * 1024 * 1024), "312 MB");
        assert_eq!(label(1434 * 1024 * 1024), "1.4 GB");
    }

    #[test]
    fn reads_the_memory_of_this_process() {
        assert!(resident_bytes().unwrap() > 1024 * 1024);
    }
}
