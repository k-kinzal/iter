# codex_cli

Type-safe Rust command builders and executor for the OpenAI Codex CLI
`0.139.0`.

The crate models Codex as its own CLI surface, not as adapter code for any one
consumer. Command families live in separate modules internally so future Codex
version updates can be reviewed by the affected command area.

## Scope

- Models Codex commands, subcommands, options, and positionals — the root
  interactive run, `exec` (with `exec resume` / `exec review`), `review`,
  `resume`, `fork`, `archive` / `unarchive`, `login` / `logout`, `mcp`,
  `plugin`, `mcp-server`, `features`, `doctor`, `sandbox`, `debug`, `apply`,
  `completion`, `update`, and the experimental `app-server` / `cloud` /
  `exec-server`.
- Executes Codex commands through `Codex::execute`.
- Provides a typed parser for `codex exec --json`'s JSONL event stream, keyed
  off the `thread.started` / `turn.started` / `item.*` / `turn.completed` /
  `turn.failed` / `error` events (plus the legacy `session_configured` /
  `task_complete` / `msg`-wrapped shapes).
- Preserves forward-compatible unknown output fields and event types behind a
  `#[non_exhaustive]` event enum with an `Other` catch-all.

## Example

```rust
use codex_cli::{Codex, ExecCommand};

# async fn example() -> Result<(), codex_cli::Error> {
let codex = Codex::default().with_current_dir(".");
let command = ExecCommand::prompt("Summarize this repository.").json();
let _output = codex.execute(&command).await?;
# Ok(())
# }
```

## Versioning

`SUPPORTED_CODEX_VERSION` records the Codex version the surface was authored
against. Updating the supported CLI version should update the command types,
output models, and tests together.

Command option structs intentionally support Rust struct literals with
`..Default::default()` where that is ergonomic. Adding newly discovered CLI
fields to those public structs is therefore treated as a semver-major API
change for this crate.
