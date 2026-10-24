//! Crash reporting: panics and (on Windows) native faults are appended to `crash.log` in the
//! settings folder (`%APPDATA%\ModelCraft\crash.log`), with the file being loaded at the time.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

static CURRENT: Mutex<Option<PathBuf>> = Mutex::new(None);
/// Pre-rendered "current file" line for the native fault handler (no allocation needed there).
static CURRENT_LINE: Mutex<String> = Mutex::new(String::new());
static GPU_ERRORS: AtomicU64 = AtomicU64::new(0);
static GPU_LAST: Mutex<String> = Mutex::new(String::new());

pub fn log_path() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("crash.log"))
}

/// Remember which file is being loaded (shown in crash reports).
pub fn set_current_file(p: Option<&Path>) {
    if let Ok(mut c) = CURRENT.lock() {
        *c = p.map(Path::to_path_buf);
    }
    if let Ok(mut l) = CURRENT_LINE.lock() {
        *l = p.map(|p| format!("fichier en cours : {}\r\n", p.display())).unwrap_or_default();
    }
}

pub fn current_file() -> Option<PathBuf> {
    CURRENT.lock().ok().and_then(|c| c.clone())
}

pub fn panic_text(e: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = e.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = e.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".into()
    }
}

/// UTC timestamp "YYYY-MM-DD HH:MM:SS UTC" without external crates.
fn now_utc() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", rem / 3600, rem / 60 % 60, rem % 60)
}

/// crash.log opened at start-up, for the allocation-failure path (which must not allocate).
static RAW_LOG: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();

/// Global allocator that notes an unrecoverable allocation failure in crash.log before Rust
/// aborts the process (an abort cannot be caught by the panic hook or the exception filter).
pub struct LoggingAlloc;

unsafe impl std::alloc::GlobalAlloc for LoggingAlloc {
    unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
        let p = unsafe { std::alloc::System.alloc(l) };
        if p.is_null() {
            alloc_failed(l.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: std::alloc::Layout) -> *mut u8 {
        let p = unsafe { std::alloc::System.alloc_zeroed(l) };
        if p.is_null() {
            alloc_failed(l.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
        unsafe { std::alloc::System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: std::alloc::Layout, n: usize) -> *mut u8 {
        let q = unsafe { std::alloc::System.realloc(p, l, n) };
        if q.is_null() {
            alloc_failed(n);
        }
        q
    }
}

#[cold]
fn alloc_failed(size: usize) {
    if sw_format::ctl::FALLIBLE.try_with(|f| f.get()).unwrap_or(false) {
        return; // handled by the caller (try_reserve)
    }
    let Some(mut f) = RAW_LOG.get() else { return };
    // no allocation from here on: fixed buffers only
    fn num(buf: &mut [u8; 24], mut v: u64) -> &[u8] {
        let mut i = buf.len();
        loop {
            i -= 1;
            buf[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        &buf[i..]
    }
    let mut b = [0u8; 24];
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let _ = f.write_all("==== mémoire insuffisante (arrêt du programme) - horodatage unix ".as_bytes());
    let _ = f.write_all(num(&mut b, secs));
    let _ = f.write_all("\r\nallocation refusée : ".as_bytes());
    let _ = f.write_all(num(&mut b, size as u64));
    let _ = f.write_all(b" octets\r\n");
    if let Ok(l) = CURRENT_LINE.try_lock() {
        let _ = f.write_all(l.as_bytes());
    }
    let _ = f.write_all(b"\r\n");
    let _ = f.flush();
}

pub fn append(text: &str) {
    let Some(p) = log_path() else { return };
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = f.write_all(text.as_bytes());
        let _ = f.flush();
    }
}

/// Install the panic hook (and the native fault filter on Windows).
pub fn install() {
    if let Some(p) = log_path() {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
            let _ = RAW_LOG.set(f);
        }
    }
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = panic_text(info.payload());
        let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("?").to_string();
        let bt = std::backtrace::Backtrace::force_capture();
        let file = current_file().map(|p| p.display().to_string()).unwrap_or_else(|| "-".into());
        append(&format!(
            "==== {} — {} {} — panic\r\nthread: {thread}\r\nmessage: {msg}\r\nlieu: {loc}\r\nfichier en cours : {file}\r\n{bt}\r\n\r\n",
            now_utc(),
            crate::branding::NAME,
            crate::branding::VERSION
        ));
        default(info);
    }));
    #[cfg(windows)]
    win::install();
}

/// wgpu errors are logged instead of panicking (the default handler aborts the app).
pub fn gpu_error(msg: String) {
    log::error!("GPU: {msg}");
    append(&format!("==== {} — erreur GPU\r\n{msg}\r\nfichier en cours : {}\r\n\r\n", now_utc(), current_file().map(|p| p.display().to_string()).unwrap_or_else(|| "-".into())));
    if let Ok(mut l) = GPU_LAST.lock() {
        *l = msg;
    }
    GPU_ERRORS.fetch_add(1, Ordering::SeqCst);
}

/// Number of GPU errors so far, and the last message.
pub fn gpu_errors() -> (u64, String) {
    (GPU_ERRORS.load(Ordering::SeqCst), GPU_LAST.lock().map(|l| l.clone()).unwrap_or_default())
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, EXCEPTION_STACK_OVERFLOW};
    use windows_sys::Win32::System::Diagnostics::Debug::{SetUnhandledExceptionFilter, EXCEPTION_POINTERS};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

    unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
        // Keep this short: the process is in a bad state. We still use the std file API because
        // the alternative (no report at all) is worse; a second fault here just ends the process.
        let (code, addr) = unsafe {
            match info.as_ref().and_then(|i| i.ExceptionRecord.as_ref()) {
                Some(r) => (r.ExceptionCode as u32, r.ExceptionAddress as usize),
                None => (0, 0),
            }
        };
        let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
        let off = addr.wrapping_sub(base);
        let what = match code as i32 {
            EXCEPTION_ACCESS_VIOLATION => "violation d'accès",
            EXCEPTION_STACK_OVERFLOW => "dépassement de pile",
            _ => "exception native",
        };
        let file = super::CURRENT_LINE.try_lock().map(|l| l.clone()).unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("?").to_string();
        super::append(&format!(
            "==== {} — {} {} — plantage natif\r\ncode: 0x{code:08X} ({what})\r\nadresse: 0x{addr:X} (exe+0x{off:X})\r\nthread: {thread}\r\n{file}\r\n",
            super::now_utc(),
            crate::branding::NAME,
            crate::branding::VERSION
        ));
        0 // EXCEPTION_CONTINUE_SEARCH: let Windows report / terminate as usual
    }

    pub fn install() {
        unsafe {
            SetUnhandledExceptionFilter(Some(filter));
        }
    }
}
