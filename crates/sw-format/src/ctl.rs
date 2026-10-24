//! Loading control: progress reporting, cancellation and memory limits.

use crate::{Error, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Thread-safe progress callback: overall fraction 0..1 and a stage label (French, translated by
/// the viewer), plus an optional byte counter for the current stage.
#[derive(Clone)]
pub struct Progress {
    f: Arc<dyn Fn(f32, &str) + Send + Sync>,
    bytes: Option<Arc<dyn Fn(u64, u64) + Send + Sync>>,
}

impl Progress {
    pub fn new(f: impl Fn(f32, &str) + Send + Sync + 'static) -> Self {
        Progress { f: Arc::new(f), bytes: None }
    }
    /// Also receive (done, total) byte counts while reading / decompressing.
    pub fn with_bytes(mut self, b: impl Fn(u64, u64) + Send + Sync + 'static) -> Self {
        self.bytes = Some(Arc::new(b));
        self
    }
    pub fn report(&self, fraction: f32, stage: &str) {
        (self.f)(fraction, stage)
    }
    pub fn report_bytes(&self, done: u64, total: u64) {
        if let Some(b) = &self.bytes {
            b(done, total)
        }
    }
}

impl std::fmt::Debug for Progress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Progress")
    }
}

/// Progress + cancellation for one load.
#[derive(Clone, Debug, Default)]
pub struct Ctl {
    pub progress: Option<Progress>,
    pub cancel: Option<Arc<AtomicBool>>,
}

impl Ctl {
    pub fn report(&self, fraction: f32, stage: &str) {
        if let Some(p) = &self.progress {
            p.report(fraction, stage);
        }
    }
    pub fn bytes(&self, done: u64, total: u64) {
        if let Some(p) = &self.progress {
            p.report_bytes(done, total);
        }
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed))
    }
    /// `Err(Cancelled)` if the user cancelled.
    pub fn check(&self) -> Result<()> {
        if self.cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

thread_local! {
    /// True while a fallible allocation (`reserve`) is in progress on this thread: a failure
    /// then is handled, so allocator hooks should not report it as a crash.
    pub static FALLIBLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Reserve room for `extra` more elements or fail with a readable error instead of aborting.
pub fn reserve<T>(v: &mut Vec<T>, extra: usize, what: &str) -> Result<()> {
    FALLIBLE.with(|f| f.set(true));
    let r = v.try_reserve(extra);
    FALLIBLE.with(|f| f.set(false));
    r.map_err(|_| {
        Error::OutOfMemory(format!("{what} : {} Mo demandés", (extra.saturating_mul(std::mem::size_of::<T>()) >> 20).max(1)))
    })
}

/// Total physical memory, in bytes (None if unknown).
pub fn total_memory() -> Option<u64> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut s: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        s.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        // SAFETY: plain Win32 call filling a correctly sized struct.
        if unsafe { GlobalMemoryStatusEx(&mut s) } != 0 {
            return Some(s.ullTotalPhys);
        }
        None
    }
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb = s.lines().find(|l| l.starts_with("MemTotal:"))?.split_whitespace().nth(1)?.parse::<u64>().ok()?;
        Some(kb * 1024)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

/// Physical memory currently available to us, in bytes (None if unknown).
pub fn mem_available() -> Option<u64> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut s: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        s.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        // SAFETY: plain Win32 call filling a correctly sized struct.
        if unsafe { GlobalMemoryStatusEx(&mut s) } != 0 {
            // available physical memory plus some page file headroom, never more than the address space
            return Some(s.ullAvailPhys.max(s.ullAvailPageFile.min(s.ullAvailPhys.saturating_mul(2))).min(s.ullAvailVirtual));
        }
        None
    }
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb = s.lines().find(|l| l.starts_with("MemAvailable:"))?.split_whitespace().nth(1)?.parse::<u64>().ok()?;
        Some(kb * 1024)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

/// Fail early (with a clear message) if decompressing `bytes` and building the meshes from them
/// would clearly not fit in memory. `factor` estimates the peak use relative to `bytes`.
pub fn check_memory(bytes: u64, factor: f64, what: &str) -> Result<()> {
    if let Some(avail) = mem_available() {
        let need = (bytes as f64 * factor) as u64;
        if need > avail {
            return Err(Error::TooLarge(format!(
                "{what} : environ {:.1} Go de mémoire nécessaires, {:.1} Go disponibles",
                need as f64 / 1e9,
                avail as f64 / 1e9
            )));
        }
    }
    Ok(())
}

/// Largest decompressed stream accepted (protects against corrupt size fields).
pub const MAX_STREAM: u64 = if cfg!(target_pointer_width = "64") { 8 << 30 } else { 1 << 30 };
