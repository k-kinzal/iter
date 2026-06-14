//! On-disk identity carried in `<dir>/pid`.
//!
//! Wire format:
//!
//! - Linux: `<pid>:linux:<clock_ticks>:<boot_id>\n`
//! - macOS: `<pid>:macos:<sec>:<usec>\n`
//!
//! The identity is the canonical equality predicate for "is this still the
//! same kernel process?": pid + start-time fingerprint + (Linux) boot id.

use std::fmt;
use std::str::FromStr;

use crate::process::id::Pid;
use crate::process::proc_info::ProcessStartTime;

/// Combined fingerprint that goes into `<dir>/pid`.
///
/// On Linux `linux_boot_id` is **mandatory** (per `linux_boot_id()` failing
/// fast at registration time); on macOS it is `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProcessIdentity {
    /// The kernel pid.
    pub(crate) pid: Pid,
    /// OS-tagged start-time.
    pub(crate) start_time: ProcessStartTime,
    /// Linux boot id (`/proc/sys/kernel/random/boot_id`); `None` on macOS.
    pub(crate) linux_boot_id: Option<String>,
}

impl ProcessIdentity {
    /// Render in the canonical pid-file form: `<pid>:linux:<ticks>:<boot>`
    /// or `<pid>:macos:<sec>:<usec>`.
    #[must_use]
    pub(crate) fn to_pid_line(&self) -> String {
        match &self.start_time {
            ProcessStartTime::LinuxClockTicks(ticks) => {
                let boot = self.linux_boot_id.as_deref().unwrap_or("");
                format!("{}:linux:{}:{}\n", self.pid.as_raw(), ticks, boot)
            }
            ProcessStartTime::MacosEpochMicros(micros) => {
                let sec = micros / 1_000_000;
                let usec = micros % 1_000_000;
                format!("{}:macos:{}:{}\n", self.pid.as_raw(), sec, usec)
            }
        }
    }
}

impl fmt::Display for ProcessIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let line = self.to_pid_line();
        // Strip the trailing `\n` for Display so callers can compose it
        // freely (file write keeps the newline by going through
        // `to_pid_line`).
        let trimmed = line.trim_end_matches('\n');
        f.write_str(trimmed)
    }
}

impl FromStr for ProcessIdentity {
    type Err = ParseIdentityError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let line = s.trim_end_matches('\n');
        let mut parts = line.splitn(3, ':');
        let pid_s = parts.next().ok_or(ParseIdentityError::Empty)?;
        let os = parts.next().ok_or(ParseIdentityError::MissingOs)?;
        let rest = parts.next().ok_or(ParseIdentityError::MissingStartTime)?;
        let pid: u32 = pid_s
            .parse()
            .map_err(|_| ParseIdentityError::InvalidPid(pid_s.to_owned()))?;
        match os {
            "linux" => {
                let mut bits = rest.splitn(2, ':');
                let ticks_s = bits.next().ok_or(ParseIdentityError::MissingStartTime)?;
                let boot = bits.next().ok_or(ParseIdentityError::MissingBootId)?;
                let ticks: u64 = ticks_s
                    .parse()
                    .map_err(|_| ParseIdentityError::InvalidStartTime(ticks_s.to_owned()))?;
                if !is_lower_hex(boot) || boot.is_empty() {
                    return Err(ParseIdentityError::InvalidBootId(boot.to_owned()));
                }
                Ok(ProcessIdentity {
                    pid: Pid::new(pid),
                    start_time: ProcessStartTime::LinuxClockTicks(ticks),
                    linux_boot_id: Some(boot.to_owned()),
                })
            }
            "macos" => {
                let mut bits = rest.splitn(2, ':');
                let sec_s = bits.next().ok_or(ParseIdentityError::MissingStartTime)?;
                let usec_s = bits.next().ok_or(ParseIdentityError::MissingStartTime)?;
                let sec: u64 = sec_s
                    .parse()
                    .map_err(|_| ParseIdentityError::InvalidStartTime(sec_s.to_owned()))?;
                let usec: u64 = usec_s
                    .parse()
                    .map_err(|_| ParseIdentityError::InvalidStartTime(usec_s.to_owned()))?;
                let micros = sec.saturating_mul(1_000_000).saturating_add(usec);
                Ok(ProcessIdentity {
                    pid: Pid::new(pid),
                    start_time: ProcessStartTime::MacosEpochMicros(micros),
                    linux_boot_id: None,
                })
            }
            other => Err(ParseIdentityError::UnknownOs(other.to_owned())),
        }
    }
}

fn is_lower_hex(s: &str) -> bool {
    s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f' | '-'))
}

/// Surface for parsing failures of [`ProcessIdentity::from_str`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParseIdentityError {
    /// String was empty.
    Empty,
    /// No OS tag (`linux` / `macos`) was present.
    MissingOs,
    /// Start-time field was missing.
    MissingStartTime,
    /// Linux variant was missing the boot-id field.
    MissingBootId,
    /// pid component did not parse.
    InvalidPid(String),
    /// start-time component did not parse.
    InvalidStartTime(String),
    /// Linux boot id was empty or contained non-hex characters.
    InvalidBootId(String),
    /// OS tag was neither `linux` nor `macos`.
    UnknownOs(String),
}

impl fmt::Display for ParseIdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseIdentityError::Empty => f.write_str("pid identity is empty"),
            ParseIdentityError::MissingOs => f.write_str("pid identity is missing OS tag"),
            ParseIdentityError::MissingStartTime => {
                f.write_str("pid identity is missing start-time")
            }
            ParseIdentityError::MissingBootId => {
                f.write_str("linux pid identity is missing boot_id")
            }
            ParseIdentityError::InvalidPid(s) => write!(f, "pid component is not a u32: {s:?}"),
            ParseIdentityError::InvalidStartTime(s) => {
                write!(f, "start-time component is not a u64: {s:?}")
            }
            ParseIdentityError::InvalidBootId(s) => write!(f, "boot_id is not lower-hex: {s:?}"),
            ParseIdentityError::UnknownOs(s) => write!(f, "unknown OS tag: {s:?}"),
        }
    }
}

impl std::error::Error for ParseIdentityError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux_id() -> ProcessIdentity {
        ProcessIdentity {
            pid: Pid::new(1234),
            start_time: ProcessStartTime::LinuxClockTicks(98765),
            linux_boot_id: Some("abcdef0123456789-abcd".into()),
        }
    }

    fn macos_id() -> ProcessIdentity {
        ProcessIdentity {
            pid: Pid::new(4321),
            start_time: ProcessStartTime::MacosEpochMicros(1_700_000_000_123_456),
            linux_boot_id: None,
        }
    }

    #[test]
    fn linux_round_trip_through_pid_line() {
        let id = linux_id();
        let line = id.to_pid_line();
        assert_eq!(line, "1234:linux:98765:abcdef0123456789-abcd\n");
        let parsed: ProcessIdentity = line.parse().expect("round-trip");
        assert_eq!(parsed, id);
    }

    #[test]
    fn macos_round_trip_through_pid_line() {
        let id = macos_id();
        let line = id.to_pid_line();
        assert_eq!(line, "4321:macos:1700000000:123456\n");
        let parsed: ProcessIdentity = line.parse().expect("round-trip");
        assert_eq!(parsed, id);
    }

    #[test]
    fn unknown_os_rejected() {
        let res: Result<ProcessIdentity, _> = "1234:windows:1:2".parse();
        assert!(matches!(res, Err(ParseIdentityError::UnknownOs(_))));
    }

    #[test]
    fn invalid_pid_rejected() {
        let res: Result<ProcessIdentity, _> = "abc:linux:1:deadbeef".parse();
        assert!(matches!(res, Err(ParseIdentityError::InvalidPid(_))));
    }
}
