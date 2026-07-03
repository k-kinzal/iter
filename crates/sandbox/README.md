# sandbox

`sandbox` applies a sandbox policy to `std::process::Command` and
`tokio::process::Command` for the compilation target:

- Linux: `bwrap(1)` from Bubblewrap.
- macOS: `sandbox-exec(1)`.

The public interface is `Policy` plus `sandbox::std::Command` and
`sandbox::tokio::Command`. Platform wrapping is internal: callers describe
filesystem, network, environment, process, and target-specific sandbox policy
rather than constructing `bwrap` or `sandbox-exec` argv directly.

The wrapper APIs preserve the parts of `Command` that Rust exposes for reading:
program, args, current directory, and environment overrides. Stdio, pre-exec
hooks, uid/gid, and other write-only process settings are intentionally applied
after wrapping by the caller.

```rust
use sandbox::{Policy, std::CommandExt};

let policy = Policy::new()
    .deny_network()
    .allow_read("/usr")
    .allow_write("/tmp/work")
    .clear_environment()
    .set_env("PATH", "/usr/bin:/bin");

let mut command = std::process::Command::new("/bin/sh");
command.arg("-lc").arg("echo inside sandbox");

let output = command.sandboxed(&policy)?.output()?;
# Ok::<(), sandbox::Error>(())
```
