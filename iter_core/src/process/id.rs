//! Identifier newtypes for the process subsystem.
//!
//! - [`ProcessId`] — opaque identifier for a single iter process directory under
//!   `~/.iter/proc/<id>/`. Encoded as a [`Ulid`] so the lexicographic order
//!   matches creation order, and so the value is short enough for human use on
//!   the command line.
//! - [`Pid`] — POSIX process id. Stored as `u32` (kernel pids fit), separate
//!   from `i32` because we only ever observe non-negative pids here.
//! - [`BootstrapToken`] — 16-byte CSPRNG-generated value used to verify that
//!   the child process adopting a `proc/<id>/` directory is the one the parent
//!   spawned. Anti-accidental-adoption only (single-user assumption); not a
//!   cryptographic security boundary.

use std::fmt;
use std::str::FromStr;

use rand::RngCore;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// Opaque identifier for a single iter process.
///
/// Wraps a [`Ulid`] so callers cannot accidentally pass a free-form `String`.
/// The `Display` impl renders the canonical 26-char Crockford-base32 ULID
/// encoding **lower-cased** for readability; `FromStr` accepts either case
/// because the Crockford alphabet is case-insensitive, so on-disk directories
/// created in older (upper-case) form continue to parse correctly.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProcessId(Ulid);

impl ProcessId {
    /// Generate a fresh `ProcessId` based on the current monotonic time.
    #[must_use]
    pub fn generate() -> Self {
        Self(Ulid::new())
    }

    /// Wrap an existing [`Ulid`].
    #[must_use]
    pub fn from_ulid(ulid: Ulid) -> Self {
        Self(ulid)
    }

    /// Inner [`Ulid`].
    #[must_use]
    pub fn as_ulid(&self) -> Ulid {
        self.0
    }
}

impl fmt::Display for ProcessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Render lower-case for readability. Crockford base32 is case-insensitive,
        // so `FromStr` round-trips this form back to the same `Ulid`.
        let mut buf = [0u8; ulid::ULID_LEN];
        let upper = self.0.array_to_str(&mut buf);
        upper.make_ascii_lowercase();
        f.write_str(upper)
    }
}

impl FromStr for ProcessId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_str(s).map(Self)
    }
}

/// POSIX process id, as observed by `kill(pid, 0)` / `/proc/<pid>/stat` /
/// `proc_pidinfo`.
///
/// Stored as `u32`: real kernel pids are non-negative and fit in 32 bits on
/// every supported target. Using a newtype prevents accidental swaps with
/// other numeric quantities (signal numbers, exit codes).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Pid(u32);

impl Pid {
    /// Wrap a raw kernel pid.
    #[must_use]
    pub fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// Raw value, suitable for passing to `libc::kill` / `getpid`-style APIs.
    #[must_use]
    pub fn as_raw(self) -> u32 {
        self.0
    }
}

impl fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// 128-bit anti-accidental-adoption token written to `<dir>/bootstrap_token`
/// by the parent and re-read by the child.
///
/// **Threat model**: same OS user accidentally adopting the wrong proc dir
/// (e.g. due to a stale `--process-id` flag). Not a security boundary against
/// a hostile co-tenant on the same machine — `~/.iter` already assumes single
/// user.
///
/// On disk the value is encoded as 32 lower-case hex characters. Comparison
/// is constant-time-ish (16-byte slice equality); we do not strictly require
/// timing safety for the threat model above, but using a fixed-length buffer
/// keeps the comparison shape uniform.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct BootstrapToken([u8; 16]);

impl BootstrapToken {
    /// Generate a fresh token from the OS CSPRNG.
    #[must_use]
    pub fn generate() -> Self {
        let mut bytes = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// Wrap an existing 16-byte buffer (e.g. from an in-memory test fixture).
    #[must_use]
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Borrow the 16-byte payload.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Render as 32 lower-case hex characters (the on-disk form).
    #[must_use]
    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Debug for BootstrapToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Avoid leaking the token in logs; show length only.
        f.debug_struct("BootstrapToken")
            .field("len", &self.0.len())
            .finish()
    }
}

impl fmt::Display for BootstrapToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_id_roundtrip_via_string() {
        let id = ProcessId::generate();
        let s = id.to_string();
        let parsed: ProcessId = s.parse().expect("ULID round-trip");
        assert_eq!(id, parsed);
    }

    #[test]
    fn process_id_display_is_lowercase() {
        let id = ProcessId::generate();
        let s = id.to_string();
        assert_eq!(s.len(), 26);
        assert!(
            s.chars().all(|c| !c.is_ascii_uppercase()),
            "expected lowercase ULID, got: {s}"
        );
    }

    #[test]
    fn process_id_from_str_accepts_uppercase() {
        // Crockford base32 is case-insensitive: parsing the canonical
        // upper-case form must succeed and re-render as lower-case.
        let id = ProcessId::generate();
        let upper = id.to_string().to_ascii_uppercase();
        let parsed: ProcessId = upper.parse().expect("upper-case ULID parses");
        assert_eq!(parsed, id);
        assert_eq!(parsed.to_string(), id.to_string());
    }

    #[test]
    fn pid_display_matches_raw() {
        assert_eq!(Pid::new(1234).to_string(), "1234");
    }

    #[test]
    fn bootstrap_token_hex_is_lowercase_and_32_chars() {
        let token = BootstrapToken::from_bytes([0xab; 16]);
        let hex = token.to_hex();
        assert_eq!(hex.len(), 32);
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_eq!(hex, "abababababababababababababababab");
    }

    #[test]
    fn bootstrap_token_debug_does_not_leak_payload() {
        let token = BootstrapToken::from_bytes([0xff; 16]);
        let dbg = format!("{token:?}");
        assert!(!dbg.contains("ff"), "Debug leaked payload: {dbg}");
    }
}
