//! OS layer: process VM regions + ptrace attach/read/write.
//!
//! macOS: `vmmap <pid>` for regions; legacy BSD `ptrace` (4-arg, 32-bit words).
//! Linux: `/proc/<pid>/maps` for regions; `ptrace(PTRACE_*)` (5-arg, `long` words).

use std::io;
use std::os::raw::c_int;

#[allow(non_camel_case_types)]
type pid_t = libc::pid_t;

extern "C" {
    #[link_name = "errno"]
    static mut errno: c_int;
}

/// One VM region of the target process.
#[derive(Debug, Clone)]
pub struct Region {
    pub name: String,
    pub start: u64,
    pub end: u64,
    pub perms: String,
}

/// Regions we treat as "heap" for scanning: writable, non-executable, named
/// `[heap]`/`[anon]` (older macOS / Linux), or `MALLOC_*` zones (modern macOS).
pub fn is_heap_region(r: &Region) -> bool {
    let writable = r.perms.contains('w') && !r.perms.contains('x');
    if !writable {
        return false;
    }
    match r.name.as_str() {
        "[heap]" | "[anon]" | "anon" | "" => true,
        n => n.starts_with("MALLOC_TINY")
            || n.starts_with("MALLOC_SMALL")
            || n.starts_with("MALLOC_MEDIUM")
            || n.starts_with("MALLOC_HUGE"),
    }
}

// ---------------------------------------------------------------------------
// ptrace request numbers (per-OS headers)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
const PTRACE_ATTACH: c_int = 16;
#[cfg(target_os = "linux")]
const PTRACE_DETACH: c_int = 17;
#[cfg(target_os = "linux")]
const PTRACE_PEEKDATA: c_int = 2;
#[cfg(target_os = "linux")]
const PTRACE_POKEDATA: c_int = 5;

#[cfg(target_os = "macos")]
const PT_ATTACHEXC: c_int = 14;
#[cfg(target_os = "macos")]
const PT_DETACH: c_int = 11;
#[cfg(target_os = "macos")]
const PT_READ_D: c_int = 2;
#[cfg(target_os = "macos")]
const PT_WRITE_D: c_int = 5;

fn last_os_error() -> String {
    io::Error::last_os_error().to_string()
}

/// Clear errno, run ptrace, and only treat `-1` + set errno as an error.
///
/// Important: read requests return the *word value* in the same register, so a
/// legitimately read word like `0xFFFFFFFF` comes back as `-1` and must not be
/// mistaken for failure.
#[cfg(target_os = "linux")]
fn raw_read(pid: pid_t, addr: u64) -> Result<u64, String> {
    unsafe {
        libc::errno = 0;
        let r = libc::ptrace(
            PTRACE_PEEKDATA,
            pid,
            addr as *mut _,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        if r == -1 && libc::errno != 0 {
            return Err(format!("read at {:#x}: {}", addr, last_os_error()));
        }
        Ok(r as u64)
    }
}

#[cfg(target_os = "linux")]
fn raw_write(pid: pid_t, addr: u64, value: u64) -> Result<(), String> {
    unsafe {
        let r = libc::ptrace(
            PTRACE_POKEDATA,
            pid,
            addr as *mut _,
            &value as *const u64 as *mut _,
            std::ptr::null_mut(),
        );
        if r != 0 {
            return Err(format!("write at {:#x}: {}", addr, last_os_error()));
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn raw_read(pid: pid_t, addr: u64) -> Result<u32, String> {
    unsafe {
        errno = 0;
        let r = libc::ptrace(PT_READ_D, pid, addr as *mut _, 0);
        if r == -1 && errno != 0 {
            return Err(format!("read at {:#x}: {}", addr, last_os_error()));
        }
        Ok(r as u32)
    }
}

#[cfg(target_os = "macos")]
fn raw_write(pid: pid_t, addr: u64, value: u32) -> Result<(), String> {
    unsafe {
        let r = libc::ptrace(PT_WRITE_D, pid, addr as *mut _, value as c_int);
        if r != 0 {
            return Err(format!("write at {:#x}: {}", addr, last_os_error()));
        }
        Ok(())
    }
}

/// Read 8 bytes from tracee memory. Linux: one `long` word. macOS: two `int` words.
fn read_u64(pid: pid_t, addr: u64) -> Result<u64, String> {
    #[cfg(target_os = "linux")]
    {
        raw_read(pid, addr)
    }
    #[cfg(target_os = "macos")]
    {
        let lo = raw_read(pid, addr)? as u64;
        let hi = raw_read(pid, addr + 4)? as u64;
        Ok(lo | (hi << 32))
    }
}

/// Write 8 bytes to tracee memory.
fn write_u64(pid: pid_t, addr: u64, value: u64) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        raw_write(pid, addr, value)
    }
    #[cfg(target_os = "macos")]
    {
        raw_write(pid, addr, value as u32)?;
        raw_write(pid, addr + 4, (value >> 32) as u32)
    }
}

// ---------------------------------------------------------------------------
// Tracee: attached, stopped target
// ---------------------------------------------------------------------------

/// An attached tracee. The target is stopped for the duration of the attach;
/// `Drop` detaches (resuming the target).
pub struct Tracee {
    pid: pid_t,
    attached: bool,
}

impl Tracee {
    pub fn attach(pid: pid_t) -> Result<Tracee, String> {
        let r = unsafe {
            #[cfg(target_os = "linux")]
            {
                libc::ptrace(PTRACE_ATTACH, pid, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
            }
            #[cfg(target_os = "macos")]
            {
                libc::ptrace(PT_ATTACHEXC, pid, std::ptr::null_mut(), 0)
            }
        };
        if r != 0 {
            return Err(attach_error(pid));
        }
        // Linux: PTRACE_ATTACH stops the tracee; reap the pending stop event.
        // macOS: PT_ATTACHEXC leaves the tracee running — no stop event exists,
        // a blocking waitpid would hang forever.
        #[cfg(target_os = "linux")]
        unsafe {
            let mut status: c_int = 0;
            let _ = libc::waitpid(pid, &mut status, 0);
        }
        Ok(Tracee { pid, attached: true })
    }

    pub fn read_u64(&self, addr: u64) -> Result<u64, String> {
        read_u64(self.pid, addr)
    }

    pub fn write_u64(&self, addr: u64, value: u64) -> Result<(), String> {
        write_u64(self.pid, addr, value)
    }

    pub fn detach(&mut self) -> Result<(), String> {
        if self.attached {
            let r = unsafe {
                #[cfg(target_os = "linux")]
                {
                    libc::ptrace(PTRACE_DETACH, self.pid, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
                }
                #[cfg(target_os = "macos")]
                {
                    libc::ptrace(PT_DETACH, self.pid, std::ptr::null_mut(), 0)
                }
            };
            if r != 0 {
                return Err(format!("detach from pid {} failed: {}", self.pid, last_os_error()));
            }
            self.attached = false;
        }
        Ok(())
    }
}

impl Drop for Tracee {
    fn drop(&mut self) {
        let _ = self.detach();
    }
}

fn attach_error(pid: pid_t) -> String {
    let base = format!("attach to pid {pid} failed: {}", last_os_error());
    #[cfg(target_os = "linux")]
    {
        format!(
            "{base}\nhint: target must be same-user or root; check /proc/sys/kernel/yama/ptrace_scope \
             (1 blocks tracing non-children); targets that called ptrace(PT_DENY_ATTACH) refuse attach"
        )
    }
    #[cfg(target_os = "macos")]
    {
        format!(
            "{base}\nhint: target must be same-user or root; targets that called ptrace(PT_DENY_ATTACH) refuse attach"
        )
    }
}

/// Attach, write `payload` (byte-exact, last partial word zero-padded), detach.
pub fn write_region(pid: pid_t, addr: u64, payload: &[u8]) -> Result<(), String> {
    let mut tracee = Tracee::attach(pid)?;
    let mut off = 0usize;
    for chunk in payload.chunks(8) {
        let mut w = [0u8; 8];
        w[..chunk.len()].copy_from_slice(chunk);
        tracee.write_u64(addr + off as u64, u64::from_le_bytes(w))?;
        off += chunk.len();
    }
    tracee.detach()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// VM regions
// ---------------------------------------------------------------------------

pub fn list_regions(pid: i32) -> Result<Vec<Region>, String> {
    #[cfg(target_os = "linux")]
    {
        list_regions_linux(pid)
    }
    #[cfg(target_os = "macos")]
    {
        list_regions_macos(pid)
    }
}

#[cfg(target_os = "linux")]
fn list_regions_linux(pid: i32) -> Result<Vec<Region>, String> {
    use std::fs;
    let path = format!("/proc/{pid}/maps");
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("failed to read {path}: {e} (process gone or not readable?)"))?;
    let mut regions = Vec::new();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() < 5 {
            continue;
        }
        let mut range = t[0].split('-');
        let start = match range.next().and_then(|s| u64::from_str_radix(s, 16).ok()) {
            Some(v) => v,
            None => continue,
        };
        let end = match range.next().and_then(|s| u64::from_str_radix(s, 16).ok()) {
            Some(v) => v,
            None => continue,
        };
        let perms = t[1];
        if perms.len() != 4
            || !perms
                .chars()
                .all(|c| matches!(c, 'r' | 'w' | 'x' | 'p' | 's' | '-'))
        {
            continue;
        }
        // Field 5+ is the pathname (empty for anonymous mappings).
        let name = if t.len() > 5 { t[5..].join(" ") } else { String::new() };
        regions.push(Region {
            name,
            start,
            end,
            perms: perms.to_string(),
        });
    }
    Ok(regions)
}

#[cfg(target_os = "macos")]
fn list_regions_macos(pid: i32) -> Result<Vec<Region>, String> {
    use std::process::Command;
    let out = Command::new("vmmap")
        .arg("-interleaved")
        .arg(pid.to_string())
        .output()
        .map_err(|e| format!("failed to run vmmap: {e}"))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!("vmmap for pid {pid} failed: {msg}"));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut regions = Vec::new();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        // Find the "104c44000-104c8c000" start-end token.
        let Some(i) = t.iter().position(|tok| {
            let parts: Vec<&str> = tok.split('-').collect();
            parts.len() == 2
                && !parts[0].is_empty()
                && !parts[1].is_empty()
                && parts[0].bytes().all(|b| b.is_ascii_hexdigit())
                && parts[1].bytes().all(|b| b.is_ascii_hexdigit())
        }) else {
            continue;
        };
        let (start_s, end_s) = t[i].split_once('-').unwrap();
        let (start, end) = match (
            u64::from_str_radix(start_s, 16),
            u64::from_str_radix(end_s, 16),
        ) {
            (Ok(s), Ok(e)) if e > s => (s, e),
            _ => continue,
        };
        let name = t[..i].join(" ");
        // Perms: first "rwx/rwx" token after the address range.
        let perms = t[i + 1..]
            .iter()
            .find(|tok| {
                tok.len() == 7
                    && tok.as_bytes()[3] == b'/'
                    && is_perms(&tok[..3])
                    && is_perms(&tok[4..])
            })
            .map(|tok| tok[..3].to_string());
        let Some(perms) = perms else {
            continue;
        };
        regions.push(Region {
            name,
            start,
            end,
            perms,
        });
    }
    Ok(regions)
}

fn is_perms(s: &str) -> bool {
    s.chars().all(|c| matches!(c, 'r' | 'w' | 'x' | '-'))
}

fn parse_size(s: &str) -> u64 {
    let split = s
        .as_bytes()
        .iter()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(s.len());
    let num: u64 = s[..split].parse().unwrap_or(0);
    match &s[split..] {
        "K" => num * 1_024,
        "M" => num * 1_024 * 1_024,
        "G" => num * 1_024 * 1_024 * 1_024,
        _ => num,
    }
}
