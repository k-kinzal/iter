//! `sandbox-exec` profile rendering split out of
//! [`macos`](super::macos) so the exact Scheme text can be asserted in
//! unit tests without running `sandbox-exec` itself.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::SandboxRequirements;

use super::super::policy::NetworkAccess;
use super::SandboxDescriptor;

/// Build the full sandbox-exec Scheme profile for `descriptor`.
///
/// Split out of [`SandboxBackend::prepare`](super::SandboxBackend::prepare)
/// so the exact text can be asserted on in unit tests without running
/// `sandbox-exec` itself.
#[must_use]
pub fn render_profile(descriptor: &SandboxDescriptor<'_>) -> String {
    let mut buf = String::new();

    buf.push_str("(version 1)\n");
    buf.push_str("(deny default)\n\n");

    // --- Process / signal / sysctl ops ----------------------------------
    //
    // Signal defaults to `target self` — enough for an agent that only
    // signals itself (pthread_kill, abort handlers). Agents that spawn
    // and manage subprocess trees (shell tools, `timeout`, `kill`) must
    // opt into the broader `(allow signal)` rule via
    // [`SandboxRequirements::allow_signal`]; rendered a few lines below
    // so a `true` value overrides the baseline. The kernel's UID check
    // still bounds the expanded allow to same-UID processes.
    buf.push_str("; Process management — exec/fork + self-signal.\n");
    render_process_exec(&mut buf, &descriptor.policy.allow_exec);
    buf.push_str("(allow process-fork)\n");
    if descriptor.requirements.allow_signal {
        buf.push_str("(allow signal)\n");
    } else {
        buf.push_str("(allow signal (target self))\n");
    }
    buf.push_str("(allow sysctl-read)\n\n");

    // --- File reads ------------------------------------------------------
    //
    // Metadata (stat/lstat/lookup) is blanket-allowed — it reveals file
    // existence and attributes but never content. Content reads
    // (file-read-data) and xattr reads (code-signing, quarantine) are
    // restricted to an enumerated set of platform directories, the
    // workspace, resolved $TMPDIR, agent-declared reads, and
    // policy-declared reads. This prevents the agent from reading past
    // session logs, SSH keys, or other projects' source code.
    buf.push_str("; File-read metadata (stat/lstat/lookup) everywhere — needed\n");
    buf.push_str("; for path traversal. Does not expose file contents.\n");
    buf.push_str("(allow file-read-metadata)\n\n");

    render_read_data(&mut buf, descriptor);

    // --- File writes: baseline dev nodes --------------------------------
    buf.push_str("; Baseline writable device nodes and inherited fds.\n");
    buf.push_str("(allow file-write*\n");
    buf.push_str("    (literal \"/dev/null\")\n");
    buf.push_str("    (literal \"/dev/stdout\")\n");
    buf.push_str("    (literal \"/dev/stderr\")\n");
    buf.push_str("    (literal \"/dev/dtracehelper\")\n");
    buf.push_str("    (regex #\"^/dev/tty[a-z]?\")\n");
    buf.push_str("    (regex #\"^/dev/fd/[0-9]+\"))\n\n");

    // --- Terminal ioctl (setRawMode, ink, readline) ---------------------
    buf.push_str("; Terminal ioctl — interactive agents need setRawMode etc.\n");
    buf.push_str("(allow file-ioctl (regex #\"^/dev/tty.*\"))\n\n");

    // --- Workspace tmpdir ------------------------------------------------
    //
    // Canonicalize before emitting: macOS checks rules against the
    // canonical (realpath-resolved) name, so a `subpath "/tmp/..."`
    // allowance silently misses when the workspace actually lives under
    // `/private/tmp/...` (and symmetric for `/var` → `/private/var`).
    let ws = canonical_or_self(descriptor.workspace_path);
    let ws = sb_string(&ws.display().to_string());
    writeln!(
        buf,
        "; Workspace tmpdir — read granted via enumerated block above;\n\
         ; this line is the write hole.\n\
         (allow file-write* (subpath {ws}))"
    )
    .ok();
    buf.push('\n');

    // --- Agent-declared / policy write paths -----------------------------
    let mut writes: Vec<PathBuf> = Vec::new();
    for p in &descriptor.policy.allow_write_outside {
        writes.push(canonical_or_self(p));
    }
    for p in &descriptor.requirements.file_writes {
        writes.push(canonical_or_self(p));
    }
    writes.sort();
    writes.dedup();
    if !writes.is_empty() {
        buf.push_str("; Agent-declared / policy write allowances.\n");
        buf.push_str("(allow file-write*\n");
        for p in &writes {
            writeln!(buf, "    (subpath {})", sb_string(&p.display().to_string())).ok();
        }
        buf.push_str(")\n\n");
    }

    // --- Agent-declared write regexes -----------------------------------
    //
    // Per-session random names (claude-code's `/tmp/claude-<hex4>-cwd`
    // shell wrapper file is the motivating case) cannot be expressed as
    // a literal subpath. Agents declare them as sandbox-exec SBPL
    // regexes via `requirements.file_write_regexes`; we emit each as a
    // standalone `(allow file-write* (regex #"..."))` rule.
    if !descriptor.requirements.file_write_regexes.is_empty() {
        buf.push_str("; Agent-declared write-regex allowances.\n");
        for re in &descriptor.requirements.file_write_regexes {
            writeln!(buf, "(allow file-write* (regex #{}))", sb_string(re)).ok();
        }
        buf.push('\n');
    }

    // --- Network -------------------------------------------------------
    render_network(
        &mut buf,
        &descriptor.policy.network,
        descriptor.requirements,
    );
    buf.push('\n');

    // --- Mach services -------------------------------------------------
    //
    // Narrow list, not blanket. These three are the minimum that allows
    // keychain-backed TLS trust evaluation + DNS resolution — enough for
    // any HTTPS client to reach the public internet.
    buf.push_str("; Mach services — minimum for TLS trust + DNS.\n");
    buf.push_str("(allow mach-lookup\n");
    buf.push_str("    (global-name \"com.apple.SecurityServer\")\n");
    buf.push_str("    (global-name \"com.apple.trustd\")\n");
    buf.push_str("    (global-name \"com.apple.mDNSResponder\"))\n\n");

    // --- Extra deny overrides (last — deny wins over earlier allows) ---
    if !descriptor.policy.extra_deny_paths.is_empty() {
        buf.push_str("; Explicit denies — last rule wins, so these defeat\n");
        buf.push_str("; the metadata allow, enumerated reads, and writes above.\n");
        buf.push_str("(deny file-read* file-write*\n");
        for p in &descriptor.policy.extra_deny_paths {
            let canon = canonical_or_self(p);
            writeln!(
                buf,
                "    (subpath {})",
                sb_string(&canon.display().to_string())
            )
            .ok();
        }
        buf.push_str(")\n");
    }

    buf
}

/// Stable macOS system directories that any Bun/node/native binary
/// needs to read at runtime: dynamic linker caches, system frameworks,
/// ICU/timezone data, CA certificates, DNS config, device nodes, and
/// Homebrew prefix. Entries use canonical (realpath) forms where macOS
/// has a `/private/` prefix.
const PLATFORM_READ_SUBPATHS: &[&str] = &[
    "/usr",
    "/System",
    "/Library/Preferences",
    "/private/var/db",
    "/private/etc",
    "/dev",
    "/bin",
    "/sbin",
    "/opt/homebrew",
];

/// Emit `process-exec` and `process-exec-interpreter` rules. Blanket
/// when `allow_exec` is empty; a per-binary `(literal ...)` block for
/// each operation otherwise. The interpreter variant covers shebang
/// dispatch — without it, scripts whose `#!` line names an allowed
/// binary would be denied by the kernel.
fn render_process_exec(buf: &mut String, allow_exec: &[PathBuf]) {
    if allow_exec.is_empty() {
        buf.push_str("(allow process-exec)\n");
    } else {
        let mut paths: Vec<PathBuf> = allow_exec.iter().map(|p| canonical_or_self(p)).collect();
        paths.sort();
        paths.dedup();
        buf.push_str("(allow process-exec\n");
        for path in &paths {
            writeln!(
                buf,
                "    (literal {})",
                sb_string(&path.display().to_string())
            )
            .ok();
        }
        buf.push_str(")\n");
        buf.push_str("(allow process-exec-interpreter\n");
        for path in &paths {
            writeln!(
                buf,
                "    (literal {})",
                sb_string(&path.display().to_string())
            )
            .ok();
        }
        buf.push_str(")\n");
    }
}

/// Emit the enumerated `file-read-data` + `file-read-xattr` block.
///
/// Content reads are restricted to: platform system paths, the
/// workspace tmpdir, resolved `$TMPDIR`, agent-declared reads, and
/// Iterfile `allow_read_outside`.
fn render_read_data(buf: &mut String, descriptor: &SandboxDescriptor<'_>) {
    let mut paths: Vec<PathBuf> = Vec::new();

    // Platform system directories.
    for &p in PLATFORM_READ_SUBPATHS {
        paths.push(PathBuf::from(p));
    }

    // Resolved $TMPDIR — Bun/node runtimes create transient caches
    // here; the workspace tmpdir is a child but sibling temp dirs may
    // exist.
    if let Ok(tmpdir) = std::env::var("TMPDIR") {
        paths.push(canonical_or_self(Path::new(&tmpdir)));
    }

    // Workspace tmpdir.
    paths.push(canonical_or_self(descriptor.workspace_path));

    // Agent-declared reads (credentials, keychain, binary paths, …).
    for p in &descriptor.requirements.file_reads {
        paths.push(canonical_or_self(p));
    }

    // Iterfile `allow_read_outside`.
    for p in &descriptor.policy.allow_read_outside {
        paths.push(canonical_or_self(p));
    }

    // Iterfile `allow_exec` — the kernel must read a binary image to
    // exec it, so every allowed executable needs file-read-data too.
    // These are files, not directories, so they are emitted as
    // `(literal ...)` rather than `(subpath ...)`.
    let mut exec_read_literals: Vec<PathBuf> = descriptor
        .policy
        .allow_exec
        .iter()
        .map(|p| canonical_or_self(p))
        .filter(|p| !paths.iter().any(|existing| p.starts_with(existing)))
        .collect();
    exec_read_literals.sort();
    exec_read_literals.dedup();

    paths.sort();
    paths.dedup();

    // Collect unique parent directories needed for path traversal.
    // Without file-read-data on each intermediate directory the kernel
    // cannot resolve child paths (e.g. /Users -> /Users/example -> ...).
    // Platform subpaths and the root literal already cover /usr, /System
    // etc.; this only adds parents for agent/policy/workspace paths
    // that live outside those trees (typically under /Users/$USER/…).
    let all_paths_for_parents = paths.iter().chain(exec_read_literals.iter());
    let mut parents: Vec<PathBuf> = Vec::new();
    for p in all_paths_for_parents {
        for ancestor in p.ancestors().skip(1) {
            if ancestor == Path::new("/") {
                break;
            }
            let a = ancestor.to_path_buf();
            if !paths.iter().any(|existing| a.starts_with(existing)) {
                parents.push(a);
            }
        }
    }
    parents.sort();
    parents.dedup();

    buf.push_str("; File-read data+xattr — enumerated. Content reads are\n");
    buf.push_str("; restricted to platform dirs, workspace, agent and policy paths.\n");
    buf.push_str("; The root literal and parent-directory literals are needed for\n");
    buf.push_str("; directory-component lookup (readdir to resolve child paths).\n");
    buf.push_str("(allow file-read-data file-read-xattr\n");
    buf.push_str("    (literal \"/\")\n");
    for p in &parents {
        writeln!(buf, "    (literal {})", sb_string(&p.display().to_string())).ok();
    }
    for p in &exec_read_literals {
        writeln!(buf, "    (literal {})", sb_string(&p.display().to_string())).ok();
    }
    for p in &paths {
        writeln!(buf, "    (subpath {})", sb_string(&p.display().to_string())).ok();
    }
    buf.push_str(")\n\n");
}

fn render_network(buf: &mut String, access: &NetworkAccess, reqs: &SandboxRequirements) {
    buf.push_str("; Network policy.\n");
    match access {
        NetworkAccess::Off => {
            buf.push_str("(deny network*)\n");
        }
        NetworkAccess::All => {
            // Still narrower than `(allow network*)`: inbound is opt-in,
            // not implied.
            buf.push_str("(allow network-outbound)\n");
            buf.push_str("(allow network-bind (local ip))\n");
        }
        NetworkAccess::Hosts(_hosts) => {
            // sandbox-exec cannot filter by hostname; document the
            // degradation in-profile for anyone tailing /var/log.
            if reqs.network_hosts.is_empty() {
                buf.push_str("; No hosts declared by agent and policy requested host-level\n");
                buf.push_str("; filtering — denying all network as the safe fallback.\n");
                buf.push_str("(deny network*)\n");
            } else {
                buf.push_str("; sandbox-exec cannot filter by hostname; allowing all\n");
                buf.push_str("; outbound TCP/UDP and relying on agent requirements plus any\n");
                buf.push_str("; external firewall (pf/Little Snitch) to constrain further.\n");
                buf.push_str("(allow network-outbound)\n");
            }
        }
    }
}

/// Quote a string for the Scheme profile — escape backslashes and quotes.
fn sb_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Resolve `p` to its canonical (realpath) form if possible, falling back
/// to the input path unchanged when the path does not yet exist (which is
/// normal for workspace tmpdirs that are created later in the lifecycle).
///
/// macOS's kernel sandbox checks every rule against the **canonical**
/// file name, so rules that reference `/tmp/...` silently miss when the
/// actual access goes through `/private/tmp/...` (same for `/var` →
/// `/private/var`). Canonicalising at render time keeps the profile
/// aligned with what the kernel will observe at check time.
fn canonical_or_self(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::super::super::policy::{NetworkAccess, SandboxPolicy};
    use super::super::SandboxDescriptor;
    use super::*;
    use crate::SandboxRequirements;

    /// Default-deny test policy: every field empty, network off. Tests
    /// construct this explicitly because the production code has no
    /// `deny_all_policy()` — the Iterfile is the single source
    /// of truth for project-shaped sandbox settings.
    fn deny_all_policy() -> SandboxPolicy {
        SandboxPolicy {
            network: NetworkAccess::Off,
            allow_read_outside: Vec::new(),
            allow_write_outside: Vec::new(),
            extra_deny_paths: Vec::new(),
            allow_exec: Vec::new(),
        }
    }

    fn desc<'a>(
        ws: &'a Path,
        policy: &'a SandboxPolicy,
        reqs: &'a SandboxRequirements,
    ) -> SandboxDescriptor<'a> {
        SandboxDescriptor {
            workspace_path: ws,
            policy,
            requirements: reqs,
        }
    }

    #[test]
    fn profile_denies_by_default_and_opens_workspace_write() {
        let ws = Path::new("/tmp/iter-ws-123");
        let policy = deny_all_policy();
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));

        assert!(profile.contains("(deny default)"));
        assert!(
            profile.contains("(allow file-write* (subpath \"/tmp/iter-ws-123\"))")
                || profile.contains("(allow file-write* (subpath \"/private/tmp/iter-ws-123\"))"),
            "expected workspace write subpath, got:\n{profile}"
        );
        assert!(profile.contains("(deny network*)"));
    }

    #[test]
    fn profile_defaults_signal_to_self_target() {
        let ws = Path::new("/tmp/ws");
        let policy = deny_all_policy();
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));
        assert!(
            profile.contains("(allow signal (target self))"),
            "default signal rule must be self-only:\n{profile}"
        );
        assert!(
            !profile.contains("(allow signal)\n"),
            "blanket signal rule must not appear by default:\n{profile}"
        );
    }

    #[test]
    fn profile_broadens_signal_when_agent_opts_in() {
        let ws = Path::new("/tmp/ws");
        let policy = deny_all_policy();
        let reqs = SandboxRequirements {
            allow_signal: true,
            ..SandboxRequirements::default()
        };
        let profile = render_profile(&desc(ws, &policy, &reqs));
        assert!(
            profile.contains("(allow signal)\n"),
            "agent opt-in must emit blanket signal rule:\n{profile}"
        );
        assert!(
            !profile.contains("(allow signal (target self))"),
            "self-target rule must be replaced, not appended:\n{profile}"
        );
    }

    #[test]
    fn profile_uses_enumerated_reads_not_blanket() {
        let ws = Path::new("/tmp/ws");
        let policy = deny_all_policy();
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));

        // Blanket content reads must NOT appear.
        assert!(
            !profile.contains("(allow file-read*)"),
            "blanket file-read* should not appear:\n{profile}"
        );
        // Blanket metadata is allowed (stat/lstat, path traversal).
        assert!(
            profile.contains("(allow file-read-metadata)"),
            "expected blanket file-read-metadata:\n{profile}"
        );
        // Platform system directories are enumerated.
        assert!(
            profile.contains("file-read-data file-read-xattr"),
            "expected enumerated file-read-data block:\n{profile}"
        );
        for platform in &["/usr", "/System", "/private/etc", "/dev"] {
            assert!(
                profile.contains(&format!("(subpath \"{platform}\")")),
                "expected platform subpath {platform}:\n{profile}"
            );
        }
    }

    #[test]
    fn profile_includes_agent_declared_reads() {
        let ws = Path::new("/tmp/ws");
        let policy = deny_all_policy();
        let reqs = SandboxRequirements {
            file_reads: vec![PathBuf::from("/Users/me/.claude/.credentials.json")],
            ..SandboxRequirements::default()
        };
        let profile = render_profile(&desc(ws, &policy, &reqs));
        assert!(
            profile.contains(".credentials.json"),
            "agent-declared read path must appear in enumerated reads:\n{profile}"
        );
    }

    #[test]
    fn profile_includes_policy_read_outside() {
        let ws = Path::new("/tmp/ws");
        let policy = SandboxPolicy {
            allow_read_outside: vec![PathBuf::from("/Users/me/data")],
            ..deny_all_policy()
        };
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));
        assert!(
            profile.contains("/Users/me/data"),
            "policy allow_read_outside must appear in profile:\n{profile}"
        );
    }

    #[test]
    fn profile_restricts_mach_lookup_to_named_services() {
        let ws = Path::new("/tmp/ws");
        let policy = deny_all_policy();
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));

        assert!(profile.contains("(global-name \"com.apple.SecurityServer\")"));
        assert!(profile.contains("(global-name \"com.apple.trustd\")"));
        assert!(profile.contains("(global-name \"com.apple.mDNSResponder\")"));
        assert!(
            !profile.contains("(allow mach-lookup)\n"),
            "blanket mach-lookup allow should not appear:\n{profile}"
        );
    }

    #[test]
    fn profile_adds_agent_and_policy_write_paths() {
        let ws = Path::new("/tmp/ws");
        let policy = SandboxPolicy {
            allow_write_outside: vec![PathBuf::from("/var/my-policy-out")],
            ..deny_all_policy()
        };
        let reqs = SandboxRequirements {
            file_writes: vec![PathBuf::from("/Users/me/.claude/projects")],
            ..SandboxRequirements::default()
        };
        let profile = render_profile(&desc(ws, &policy, &reqs));

        assert!(profile.contains("/Users/me/.claude/projects"));
        assert!(
            profile.contains("/var/my-policy-out")
                || profile.contains("/private/var/my-policy-out")
        );
    }

    #[test]
    fn profile_emits_agent_write_regexes_as_regex_allows() {
        let ws = Path::new("/tmp/ws");
        let policy = deny_all_policy();
        let reqs = SandboxRequirements {
            file_write_regexes: vec![r"^/private/tmp/claude-[0-9a-f]+-cwd$".into()],
            ..SandboxRequirements::default()
        };
        let profile = render_profile(&desc(ws, &policy, &reqs));
        assert!(
            profile
                .contains("(allow file-write* (regex #\"^/private/tmp/claude-[0-9a-f]+-cwd$\"))"),
            "regex write allow must be rendered verbatim, got:\n{profile}",
        );
    }

    #[test]
    fn profile_respects_extra_deny_paths_and_emits_them_last() {
        let ws = Path::new("/tmp/ws");
        let policy = SandboxPolicy {
            extra_deny_paths: vec![PathBuf::from("/Users/me/.ssh")],
            ..deny_all_policy()
        };
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));
        assert!(profile.contains("/Users/me/.ssh"));
        let deny_idx = profile
            .find("(deny file-read* file-write*")
            .expect("deny block present");
        let metadata_idx = profile
            .find("(allow file-read-metadata)")
            .expect("metadata allow present");
        assert!(
            deny_idx > metadata_idx,
            "deny must come after allows — got deny@{deny_idx}, metadata@{metadata_idx}"
        );
    }

    #[test]
    fn profile_escapes_quotes_in_paths() {
        let ws = Path::new("/tmp/with\"quote");
        let policy = deny_all_policy();
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));
        assert!(profile.contains("/tmp/with\\\"quote"));
    }

    #[test]
    fn network_all_emits_outbound_not_blanket() {
        let ws = Path::new("/tmp/ws");
        let policy = SandboxPolicy {
            network: NetworkAccess::All,
            ..deny_all_policy()
        };
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));
        assert!(profile.contains("(allow network-outbound)"));
        assert!(
            !profile.contains("(allow network*)"),
            "blanket network allow should not appear:\n{profile}"
        );
    }

    #[test]
    fn network_hosts_without_agent_declarations_denies() {
        let ws = Path::new("/tmp/ws");
        let policy = SandboxPolicy {
            network: NetworkAccess::Hosts(vec!["api.anthropic.com".into()]),
            ..deny_all_policy()
        };
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));
        assert!(
            profile.contains("denying all network"),
            "expected fallback deny, got:\n{profile}"
        );
    }

    #[test]
    fn network_hosts_with_agent_declarations_opens_outbound() {
        let ws = Path::new("/tmp/ws");
        let policy = SandboxPolicy {
            network: NetworkAccess::Hosts(vec!["api.anthropic.com".into()]),
            ..deny_all_policy()
        };
        let reqs = SandboxRequirements {
            network_hosts: vec!["api.anthropic.com:443".into()],
            ..SandboxRequirements::default()
        };
        let profile = render_profile(&desc(ws, &policy, &reqs));
        assert!(profile.contains("(allow network-outbound)"));
        assert!(
            !profile.contains("(allow network*)"),
            "blanket network allow should not appear:\n{profile}"
        );
    }

    #[test]
    fn profile_blanket_exec_when_allow_exec_empty() {
        let ws = Path::new("/tmp/ws");
        let policy = deny_all_policy();
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));
        assert!(
            profile.contains("(allow process-exec)\n"),
            "empty allow_exec must emit blanket process-exec:\n{profile}"
        );
    }

    #[test]
    fn profile_restricts_exec_to_listed_binaries() {
        let ws = Path::new("/tmp/ws");
        let policy = SandboxPolicy {
            allow_exec: vec![
                PathBuf::from("/usr/bin/git"),
                PathBuf::from("/usr/bin/cargo"),
            ],
            ..deny_all_policy()
        };
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));
        assert!(
            !profile.contains("(allow process-exec)\n"),
            "blanket process-exec must not appear when allow_exec is populated:\n{profile}"
        );
        assert!(
            profile.contains("(literal \"/usr/bin/git\")"),
            "expected literal entry for /usr/bin/git:\n{profile}"
        );
        assert!(
            profile.contains("(literal \"/usr/bin/cargo\")"),
            "expected literal entry for /usr/bin/cargo:\n{profile}"
        );
        assert!(
            profile.contains("(allow process-exec\n"),
            "expected process-exec block with literal entries:\n{profile}"
        );
    }

    #[test]
    fn profile_exec_interpreter_emitted_when_allow_exec_populated() {
        let ws = Path::new("/tmp/ws");
        let policy = SandboxPolicy {
            allow_exec: vec![PathBuf::from("/usr/bin/git")],
            ..deny_all_policy()
        };
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));
        assert!(
            profile.contains("(allow process-exec-interpreter\n"),
            "process-exec-interpreter block must appear when allow_exec is populated:\n{profile}"
        );
        assert!(
            profile.contains("(allow process-exec-interpreter\n    (literal \"/usr/bin/git\")"),
            "interpreter block must contain the same literal entries:\n{profile}"
        );
    }

    #[test]
    fn profile_no_interpreter_block_when_allow_exec_empty() {
        let ws = Path::new("/tmp/ws");
        let policy = deny_all_policy();
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));
        assert!(
            !profile.contains("process-exec-interpreter"),
            "interpreter block must not appear when allow_exec is empty:\n{profile}"
        );
    }

    #[test]
    fn profile_exec_single_entry() {
        let ws = Path::new("/tmp/ws");
        let policy = SandboxPolicy {
            allow_exec: vec![PathBuf::from("/usr/bin/git")],
            ..deny_all_policy()
        };
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));
        assert!(
            profile.contains("(allow process-exec\n    (literal \"/usr/bin/git\")"),
            "single allow_exec entry must produce a literal rule:\n{profile}"
        );
    }

    #[test]
    fn profile_exec_nonexistent_path_emits_raw() {
        let ws = Path::new("/tmp/ws");
        let policy = SandboxPolicy {
            allow_exec: vec![PathBuf::from("/nonexistent/path/to/binary")],
            ..deny_all_policy()
        };
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));
        assert!(
            profile.contains("(literal \"/nonexistent/path/to/binary\")"),
            "non-existent path must appear verbatim (canonicalize fallback):\n{profile}"
        );
    }

    #[test]
    fn profile_exec_escapes_quotes_in_paths() {
        let ws = Path::new("/tmp/ws");
        let policy = SandboxPolicy {
            allow_exec: vec![PathBuf::from("/usr/bin/with\"quote")],
            ..deny_all_policy()
        };
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));
        assert!(
            profile.contains("/usr/bin/with\\\"quote"),
            "quotes in allow_exec paths must be escaped:\n{profile}"
        );
    }

    #[test]
    fn profile_exec_paths_added_to_read_block_as_literal() {
        let ws = Path::new("/tmp/ws");
        let exec_path = PathBuf::from("/opt/custom/bin/my-agent");
        let policy = SandboxPolicy {
            allow_exec: vec![exec_path.clone()],
            ..deny_all_policy()
        };
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));
        let read_block_start = profile
            .find("file-read-data file-read-xattr")
            .expect("read block present");
        let read_block_end = profile[read_block_start..]
            .find("\n\n")
            .map_or(profile.len(), |i| read_block_start + i);
        let read_block = &profile[read_block_start..read_block_end];
        assert!(
            read_block.contains("(literal \"/opt/custom/bin/my-agent\")"),
            "allow_exec path must appear as literal in file-read-data block:\n{read_block}"
        );
        assert!(
            !read_block.contains("(subpath \"/opt/custom/bin/my-agent\")"),
            "allow_exec path must not appear as subpath in file-read-data block:\n{read_block}"
        );
    }

    #[test]
    fn profile_exec_inside_platform_subpath_skips_redundant_literal() {
        let ws = Path::new("/tmp/ws");
        let policy = SandboxPolicy {
            allow_exec: vec![PathBuf::from("/usr/bin/git")],
            ..deny_all_policy()
        };
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));
        let read_block_start = profile
            .find("file-read-data file-read-xattr")
            .expect("read block present");
        let read_block_end = profile[read_block_start..]
            .find("\n\n")
            .map_or(profile.len(), |i| read_block_start + i);
        let read_block = &profile[read_block_start..read_block_end];
        assert!(
            !read_block.contains("(literal \"/usr/bin/git\")"),
            "/usr/bin/git is covered by (subpath \"/usr\"); no redundant literal needed:\n{read_block}"
        );
    }

    #[test]
    fn profile_exec_deduplicates_entries() {
        let ws = Path::new("/tmp/ws");
        let policy = SandboxPolicy {
            allow_exec: vec![
                PathBuf::from("/nonexistent/dedup-test-binary"),
                PathBuf::from("/nonexistent/dedup-test-binary"),
            ],
            ..deny_all_policy()
        };
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));
        assert!(
            profile.contains(
                "(allow process-exec\n    (literal \"/nonexistent/dedup-test-binary\")\n)"
            ),
            "process-exec block should contain exactly one deduplicated entry:\n{profile}"
        );
        assert!(
            profile.contains("(allow process-exec-interpreter\n    (literal \"/nonexistent/dedup-test-binary\")\n)"),
            "process-exec-interpreter block should contain exactly one deduplicated entry:\n{profile}"
        );
    }

    #[test]
    fn workspace_path_is_canonicalized_when_possible() {
        let ws = Path::new("/tmp");
        let policy = deny_all_policy();
        let reqs = SandboxRequirements::default();
        let profile = render_profile(&desc(ws, &policy, &reqs));
        let has_canonical = profile.contains("(allow file-write* (subpath \"/private/tmp\"))");
        let has_raw = profile.contains("(allow file-write* (subpath \"/tmp\"))");
        assert!(
            has_canonical || has_raw,
            "expected /tmp → canonical /private/tmp (or raw /tmp fallback):\n{profile}"
        );
    }
}
