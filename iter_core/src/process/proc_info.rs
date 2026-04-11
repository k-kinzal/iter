//! OS-level introspection: read a process's start-time fingerprint and check
//! whether a given (pid, `start_time`, `boot_id`?) is still alive.
//!
//! # Why a fingerprint at all
//!
//! A bare pid is reusable. A previously-killed process's pid can be assigned
//! to an unrelated program before `iter ps` reads the proc directory; without
//! a fingerprint, `kill(pid, 0) == 0` would incorrectly report the old runner
//! as alive. We therefore record the kernel start-time of the runner and
//! cross-check it during liveness queries.
//!
//! # Linux
//!
//! `/proc/<pid>/stat` field 22 (`starttime`) is the count of clock-ticks
//! since boot at which the process started. Combined with the system
//! `boot_id` from `/proc/sys/kernel/random/boot_id`, this is reuse-proof
//! across reboots: a fresh boot resets the tick counter, so without the
//! `boot_id` we would falsely match.
//!
//! `/proc/<pid>/stat` is whitespace-separated, but field 2 (`comm`) is
//! enclosed in parentheses and may itself contain whitespace. The robust
//! parser splits on the *last* `") "` boundary, then on whitespace.
//!
//! # macOS
//!
//! `proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &out, sizeof(struct proc_bsdinfo))`
//! returns a `proc_bsdinfo` whose `pbi_start_tvsec` * `1_000_000` +
//! `pbi_start_tvusec` is the wall-clock start time in microseconds.
//! `proc_bsdshortinfo` does **not** carry a start-time field — earlier plan
//! revisions referenced it incorrectly.

use std::fmt;
use std::io;
#[cfg(target_os = "linux")]
use std::sync::OnceLock;

use crate::process::error::ProcessError;
use crate::process::id::Pid;
use crate::process::pid_file::ProcessIdentity;

/// OS-tagged start-time fingerprint stored in the pid file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessStartTime {
    /// Linux: clock-ticks since boot (`/proc/<pid>/stat` field 22). Use with
    /// the accompanying `linux_boot_id`.
    LinuxClockTicks(u64),
    /// macOS: epoch microseconds (`pbi_start_tvsec` * 1e6 + `pbi_start_tvusec`).
    MacosEpochMicros(u64),
}

impl fmt::Display for ProcessStartTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProcessStartTime::LinuxClockTicks(n) => write!(f, "{n}"),
            ProcessStartTime::MacosEpochMicros(n) => {
                let sec = n / 1_000_000;
                let usec = n % 1_000_000;
                write!(f, "{sec}:{usec}")
            }
        }
    }
}

/// Error returned by [`ProcessStartTime::from_label_string`].
#[derive(Debug, thiserror::Error)]
pub enum ProcessStartTimeParseError {
    /// The input is missing the `linux:` / `macos:` tag prefix.
    #[error("missing tag prefix in {raw:?} (expected linux: or macos:)")]
    MissingTag {
        /// Original input that failed to parse.
        raw: String,
    },
    /// The tag is recognised but the body is malformed.
    #[error("malformed body for {tag} fingerprint: {raw:?}")]
    MalformedBody {
        /// The recognised tag (`linux` or `macos`).
        tag: &'static str,
        /// Original input that failed to parse.
        raw: String,
    },
}

impl ProcessStartTime {
    /// Round-trip serialise the fingerprint with an explicit OS tag.
    ///
    /// Format: `linux:<ticks>` / `macos:<sec>:<usec>`. Distinct from
    /// [`Display`], which prints the human-readable form without a tag.
    /// Used by orchestrator labels (e.g. `iter.compose.orchestrator_start_time`)
    /// where the consumer may run on a different host than the producer.
    #[must_use]
    pub fn to_label_string(&self) -> String {
        match self {
            ProcessStartTime::LinuxClockTicks(n) => format!("linux:{n}"),
            ProcessStartTime::MacosEpochMicros(n) => {
                let sec = n / 1_000_000;
                let usec = n % 1_000_000;
                format!("macos:{sec}:{usec}")
            }
        }
    }

    /// Inverse of [`to_label_string`].
    ///
    /// # Errors
    ///
    /// Returns [`ProcessStartTimeParseError`] if the prefix is unknown or
    /// the body is malformed.
    ///
    /// [`to_label_string`]: Self::to_label_string
    pub fn from_label_string(raw: &str) -> Result<Self, ProcessStartTimeParseError> {
        if let Some(body) = raw.strip_prefix("linux:") {
            let ticks =
                body.parse::<u64>()
                    .map_err(|_| ProcessStartTimeParseError::MalformedBody {
                        tag: "linux",
                        raw: raw.to_owned(),
                    })?;
            return Ok(ProcessStartTime::LinuxClockTicks(ticks));
        }
        if let Some(body) = raw.strip_prefix("macos:") {
            let (sec_str, usec_str) =
                body.split_once(':')
                    .ok_or_else(|| ProcessStartTimeParseError::MalformedBody {
                        tag: "macos",
                        raw: raw.to_owned(),
                    })?;
            let sec =
                sec_str
                    .parse::<u64>()
                    .map_err(|_| ProcessStartTimeParseError::MalformedBody {
                        tag: "macos",
                        raw: raw.to_owned(),
                    })?;
            let usec =
                usec_str
                    .parse::<u64>()
                    .map_err(|_| ProcessStartTimeParseError::MalformedBody {
                        tag: "macos",
                        raw: raw.to_owned(),
                    })?;
            let total = sec
                .checked_mul(1_000_000)
                .and_then(|s| s.checked_add(usec))
                .ok_or_else(|| ProcessStartTimeParseError::MalformedBody {
                    tag: "macos",
                    raw: raw.to_owned(),
                })?;
            return Ok(ProcessStartTime::MacosEpochMicros(total));
        }
        Err(ProcessStartTimeParseError::MissingTag {
            raw: raw.to_owned(),
        })
    }
}

/// Collect the current-process identity (used by foreground startup).
/// # Errors
///
/// Returns an error if the operation fails.
pub fn current_identity() -> Result<ProcessIdentity, ProcessError> {
    let pid = std::process::id();
    identity_for(Pid::new(pid))
}

/// Collect a fingerprint for an arbitrary live pid (used by detached
/// # Errors
///
/// Returns an error if the operation fails.
/// adoption from inside the child).
pub fn identity_for(pid: Pid) -> Result<ProcessIdentity, ProcessError> {
    let start_time = process_start_time(pid)?;
    #[cfg(target_os = "linux")]
    let linux_boot_id = Some(linux_boot_id().map(|s| s.to_owned()).map_err(|e| {
        ProcessError::UnsupportedProcIdentity {
            reason: format!("linux boot_id unavailable: {e}"),
        }
    })?);
    #[cfg(not(target_os = "linux"))]
    let linux_boot_id = None;

    Ok(ProcessIdentity {
        pid,
        start_time,
        linux_boot_id,
    })
}

/// Read the start-time of `pid` from the OS.
#[cfg(target_os = "linux")]
pub fn process_start_time(pid: Pid) -> Result<ProcessStartTime, ProcessError> {
    let path = format!("/proc/{}/stat", pid.as_raw());
    let raw = std::fs::read_to_string(&path).map_err(ProcessError::Io)?;
    let ticks =
        parse_linux_starttime_field22(&raw).ok_or_else(|| ProcessError::CorruptPidFile {
            raw_bytes: raw.as_bytes().to_vec(),
            reason: "could not extract /proc/<pid>/stat field 22".into(),
        })?;
    Ok(ProcessStartTime::LinuxClockTicks(ticks))
}

/// Robust parser for `/proc/<pid>/stat` that respects the `") "` boundary
/// of the `comm` field, which may itself contain whitespace.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_linux_starttime_field22(line: &str) -> Option<u64> {
    let line = line.trim_end_matches('\n');
    let close_paren = line.rfind(") ")?;
    let after = &line[close_paren + 2..];
    // After `comm`, the remaining fields are 3..N. Field 22 (1-indexed)
    // sits at offset 22 - 3 = 19 in the post-`)` slice.
    let mut iter = after.split_whitespace();
    iter.nth(19).and_then(|s| s.parse::<u64>().ok())
}

/// Read the start-time of `pid` via `proc_pidinfo PROC_PIDTBSDINFO`.
/// # Errors
///
/// Returns an error if the operation fails.
#[cfg(target_os = "macos")]
pub fn process_start_time(pid: Pid) -> Result<ProcessStartTime, ProcessError> {
    macos::start_time(pid)
}

/// Stub for non-Linux/macOS targets — always returns `UnsupportedPlatform`.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn process_start_time(_pid: Pid) -> Result<ProcessStartTime, ProcessError> {
    Err(ProcessError::UnsupportedPlatform)
}

/// Linux boot id (`/proc/sys/kernel/random/boot_id`), cached after first read
/// because the value is invariant during a boot.
#[cfg(target_os = "linux")]
pub fn linux_boot_id() -> io::Result<&'static str> {
    static BOOT_ID: OnceLock<io::Result<String>> = OnceLock::new();
    let cached = BOOT_ID.get_or_init(|| {
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id").map(|s| s.trim().to_owned())
    });
    match cached {
        Ok(s) => Ok(s.as_str()),
        Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
    }
}

/// `kill(pid, 0)` + (`start_time`, `boot_id`) cross-check.
/// # Errors
///
/// Returns an error if the operation fails.
pub fn process_is_alive_with_start_time(identity: &ProcessIdentity) -> Result<bool, ProcessError> {
    if !raw_kill_alive(identity.pid)? {
        return Ok(false);
    }
    // `kill -0` says alive — confirm fingerprint.
    let observed = match process_start_time(identity.pid) {
        Ok(s) => s,
        // The process died between `kill -0` and `process_start_time`.
        Err(ProcessError::Io(ref e)) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if observed != identity.start_time {
        return Ok(false);
    }
    #[cfg(target_os = "linux")]
    {
        let recorded =
            identity
                .linux_boot_id
                .as_deref()
                .ok_or_else(|| ProcessError::CorruptPidFile {
                    raw_bytes: vec![],
                    reason: "linux pid file missing boot_id".into(),
                })?;
        let live = linux_boot_id().map_err(ProcessError::Io)?;
        if recorded != live {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Bare `kill(pid, 0)` — does the kernel still know about this pid?
/// Returns true on `Ok` or `EPERM` (alive, possibly different UID),
/// false on `ESRCH` (gone). Treats other errno values as
/// [`ProcessError::Io`].
///
/// Unlike [`process_is_alive_with_start_time`] this does **not** verify
/// any fingerprint; the caller takes responsibility for handling the
/// pid-reuse window. Use it only when you own the pid you just spawned
/// and the window is too short for reuse to matter (e.g. the readiness
/// poll right after `spawn_unmanaged_detached`).
///
/// # Errors
///
/// Returns [`ProcessError::Io`] for unexpected errno values, or
/// [`ProcessError::UnsupportedPlatform`] on non-unix.
pub fn pid_in_process_table(pid: u32) -> Result<bool, ProcessError> {
    raw_kill_alive(Pid::new(pid))
}

#[cfg(unix)]
fn raw_kill_alive(pid: Pid) -> Result<bool, ProcessError> {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid as NixPid;

    let raw = i32::try_from(pid.as_raw()).map_err(|_| ProcessError::CorruptPidFile {
        raw_bytes: vec![],
        reason: format!("pid {pid} out of range"),
    })?;
    match kill(NixPid::from_raw(raw), None) {
        Ok(()) | Err(Errno::EPERM) => Ok(true),
        Err(Errno::ESRCH) => Ok(false),
        Err(err) => Err(ProcessError::Io(io::Error::from_raw_os_error(err as i32))),
    }
}

#[cfg(not(unix))]
fn raw_kill_alive(_pid: Pid) -> Result<bool, ProcessError> {
    Err(ProcessError::UnsupportedPlatform)
}

#[cfg(target_os = "macos")]
mod macos {
    use std::io;

    use super::{ProcessError, ProcessStartTime};
    use crate::process::id::Pid;

    pub(super) fn start_time(pid: Pid) -> Result<ProcessStartTime, ProcessError> {
        // `proc_bsdinfo` is exposed by libc on Apple targets.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size =
            i32::try_from(size_of::<libc::proc_bsdinfo>()).expect("proc_bsdinfo size fits in i32");
        let raw_pid = i32::try_from(pid.as_raw()).map_err(|_| ProcessError::CorruptPidFile {
            raw_bytes: vec![],
            reason: format!("pid {pid} out of range"),
        })?;

        // SAFETY: We pass a properly-sized buffer; the FFI signature is
        // `proc_pidinfo(pid: c_int, flavor: c_int, arg: u64, buffer: *mut c_void, buffersize: c_int) -> c_int`.
        let ret = unsafe {
            libc::proc_pidinfo(
                raw_pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&raw mut info).cast(),
                size,
            )
        };
        if ret <= 0 {
            return Err(ProcessError::Io(io::Error::last_os_error()));
        }
        if ret < size {
            return Err(ProcessError::CorruptPidFile {
                raw_bytes: vec![],
                reason: format!("proc_pidinfo returned {ret} bytes (expected {size})"),
            });
        }
        let total_micros = info
            .pbi_start_tvsec
            .saturating_mul(1_000_000)
            .saturating_add(info.pbi_start_tvusec);
        Ok(ProcessStartTime::MacosEpochMicros(total_micros))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_linux_starttime_handles_simple_comm() {
        let line = "1234 (cat) S 1 1234 1234 0 -1 4194304 1 0 0 0 0 0 0 0 20 0 1 0 12345 0 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";
        let ticks = parse_linux_starttime_field22(line).expect("parsed");
        assert_eq!(ticks, 12345);
    }

    #[test]
    fn parse_linux_starttime_handles_comm_with_spaces() {
        // comm = "weird (name)" — the close paren in comm must NOT be the
        // boundary; the parser uses the *last* ") ".
        let line = "1234 (weird (name)) S 1 1234 1234 0 -1 4194304 1 0 0 0 0 0 0 0 20 0 1 0 99999 0 0 0 0 0 0 0 0 0 0 0 17 0 0 0";
        let ticks = parse_linux_starttime_field22(line).expect("parsed");
        assert_eq!(ticks, 99999);
    }

    #[test]
    fn parse_linux_starttime_returns_none_on_truncation() {
        let line = "1234 (cat) S 1";
        assert!(parse_linux_starttime_field22(line).is_none());
    }

    #[test]
    fn macos_start_time_format_is_sec_colon_usec() {
        let st = ProcessStartTime::MacosEpochMicros(1_234_567_890_001_002);
        assert_eq!(st.to_string(), "1234567890:1002");
    }

    #[test]
    fn linux_start_time_format_is_just_ticks() {
        let st = ProcessStartTime::LinuxClockTicks(12345);
        assert_eq!(st.to_string(), "12345");
    }

    #[cfg(unix)]
    #[test]
    fn process_is_alive_for_self() {
        let me = current_identity().expect("collect identity for self");
        let alive = process_is_alive_with_start_time(&me).expect("alive check");
        assert!(alive, "self should appear alive");
    }

    #[test]
    fn label_string_round_trips_linux() {
        let original = ProcessStartTime::LinuxClockTicks(987_654);
        let s = original.to_label_string();
        assert_eq!(s, "linux:987654");
        let parsed = ProcessStartTime::from_label_string(&s).expect("parse");
        assert_eq!(parsed, original);
    }

    #[test]
    fn label_string_round_trips_macos() {
        let original = ProcessStartTime::MacosEpochMicros(1_700_000_000_123_456);
        let s = original.to_label_string();
        assert_eq!(s, "macos:1700000000:123456");
        let parsed = ProcessStartTime::from_label_string(&s).expect("parse");
        assert_eq!(parsed, original);
    }

    #[test]
    fn label_string_round_trips_macos_zero_usec() {
        let original = ProcessStartTime::MacosEpochMicros(1_700_000_000_000_000);
        let s = original.to_label_string();
        assert_eq!(s, "macos:1700000000:0");
        let parsed = ProcessStartTime::from_label_string(&s).expect("parse");
        assert_eq!(parsed, original);
    }

    #[test]
    fn from_label_string_rejects_missing_tag() {
        assert!(matches!(
            ProcessStartTime::from_label_string("12345"),
            Err(ProcessStartTimeParseError::MissingTag { .. })
        ));
    }

    #[test]
    fn from_label_string_rejects_malformed_macos() {
        assert!(matches!(
            ProcessStartTime::from_label_string("macos:abc"),
            Err(ProcessStartTimeParseError::MalformedBody { .. })
        ));
        assert!(matches!(
            ProcessStartTime::from_label_string("macos:1:2:3"),
            // sec parses, usec parse fails on "2:3"
            Err(ProcessStartTimeParseError::MalformedBody { .. })
        ));
    }

    #[test]
    fn from_label_string_rejects_malformed_linux() {
        assert!(matches!(
            ProcessStartTime::from_label_string("linux:not-a-number"),
            Err(ProcessStartTimeParseError::MalformedBody { .. })
        ));
    }
}
