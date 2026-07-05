# cline_cli

Type-safe Rust command builders and executor for the Cline CLI `3.0.23`.

The crate models Cline as its own CLI surface, not as adapter code for any one
consumer. Command families live in separate modules internally so future Cline
version updates can be reviewed by the affected command area.

## Scope

- Models Cline's root run (`cline [OPTIONS] [PROMPT]`, with the prompt as a
  positional argument) and its full subcommand tree: `auth`, `config`,
  `plugin` (`install` / `uninstall`), `connect`, `mcp`, `doctor` (`fix` /
  `log`), `history` (`delete` / `update` / `export`), `hook`, `schedule` (the
  `create` / `list` / `get` / `delete` / `pause` / `resume` / `trigger` /
  `history` / `stats` / `active` / `upcoming` / `export` family), `hub`
  (`ensure` / `start` / `status` / `stop`), `dashboard`, `update`, `version`,
  and `kanban`.
- Executes Cline commands through `Cline::execute`.
- Provides a typed parser for `cline --json`'s NDJSON run stream, keyed off the
  terminal `run_result` record (`finishReason` / `sessionId` / `message`) and
  the `run_aborted` / `error` failure events.
- Preserves forward-compatible unknown output fields and event types behind a
  `#[non_exhaustive]` event enum with an `Other` catch-all.

## The run contract

Cline `3.0.23` takes the prompt as a **positional** argument — there is no
`--oneshot` flag and no stdin feed:

```text
cline --json <prompt>
```

`--json` makes the run stream machine-readable; without it Cline prints styled
text. The default disposition is act mode with tool auto-approval enabled.

## Example

```rust
use cline_cli::{Cline, RunCommand};

# async fn example() -> Result<(), cline_cli::Error> {
let cline = Cline::default().with_current_dir(".");
let command = RunCommand::prompt("Summarize this repository.").json();
let _output = cline.execute(&command).await?;
# Ok(())
# }
```

## Versioning

`SUPPORTED_CLINE_VERSION` records the Cline version the surface was authored
against. Updating the supported CLI version should update the command types,
output models, and tests together.

Command option structs intentionally support Rust struct literals with
`..Default::default()` where that is ergonomic. Adding newly discovered CLI
fields to those public structs is therefore treated as a semver-major API
change for this crate.
