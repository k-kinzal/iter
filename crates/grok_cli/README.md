# grok_cli

Type-safe Rust command builders and executor for the Grok CLI (`grok`)
`0.2.82`.

The crate models Grok as its own CLI surface, not as adapter code for any one
consumer. Command families live in separate modules internally so future Grok
version updates can be reviewed by the affected command area.

## Scope

- Models Grok commands, subcommands, options, and positionals — the root
  interactive run, the headless single-turn run (`grok -p <PROMPT>`, with
  `--prompt-file` / `--prompt-json` sources and `--output-format`
  `plain` / `json` / `streaming-json`), `agent`
  (`stdio` / `headless` / `serve` / `leader`), `login` / `logout`, `mcp`,
  `memory`, `plugin` (with the nested `marketplace` group), `worktree` (with
  the nested `db` group), `leader`, session history (`sessions`, `export`,
  `import`, `trace`), and the operational leaves (`completions`, `dashboard`,
  `inspect`, `models`, `setup`, `update`, `version`, `wrap`).
- Executes Grok commands through `Grok::execute`.
- Provides a typed parser for `grok -p … --output-format json`'s single
  terminal object (`text` / `stopReason` / `sessionId` / `requestId` /
  `thought`), and reads the `--output-format streaming-json` event stream,
  keyed off the `text` deltas and the terminal `end` event (with the legacy
  `result` shape also recognized).
- Preserves forward-compatible unknown output fields and event types behind a
  `#[non_exhaustive]` event enum with an `Other` catch-all.

## Continuity

Grok resumes prior work through `-r, --resume [<SESSION_ID>]` (an optional id:
given one, that session; omitted, the most recent) and `-c, --continue` (the
most recent session for the working directory), modeled by
`ResumeTarget` and `SingleCommand::continue_session`. In `grok 0.2.82`,
`-s`/`--session-id` names a *new* conversation's UUID (only valid on resume
together with `--fork-session`) rather than resuming an existing session, so it
is not part of the continuity path.

## Example

```rust
use grok_cli::{Grok, SingleCommand};

# async fn example() -> Result<(), grok_cli::Error> {
let grok = Grok::default().with_current_dir(".");
let command = SingleCommand::prompt("Summarize this repository.")
    .always_approve()
    .json();
let _output = grok.execute(&command).await?;
# Ok(())
# }
```

## Versioning

`SUPPORTED_GROK_VERSION` records the Grok version the surface was authored
against. Updating the supported CLI version should update the command types,
output models, and tests together.

Command option structs intentionally support Rust struct literals with
`..Default::default()` where that is ergonomic. Adding newly discovered CLI
fields to those public structs is therefore treated as a semver-major API
change for this crate.

## Coverage note

A few deeply-nested subcommand leaves — the `agent` transports
(`stdio` / `headless` / `serve` / `leader`), `plugin marketplace *`,
`worktree db *`, and the `leader profile *` leaves — do not print their own
`--help` in `grok 0.2.82` (asking for it yields the root help), so their
individual flag sets are not verifiable from the CLI. They are modeled
structurally, with an `args` escape hatch for passing flags this crate does not
name.
