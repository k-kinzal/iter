# opencode_cli

Type-safe Rust command builders and executor for the opencode CLI `1.2.20`.

The crate models opencode as its own CLI surface, not as adapter code for any
one consumer. Command families live in separate modules internally so future
opencode version updates can be reviewed by the affected command area.

## Scope

- Models opencode commands, subcommands, options, and positionals — the root
  interactive TUI (`opencode [project]`), `run` (with its `--format json`
  event stream), the server commands (`acp` / `serve` / `web`), the session
  and provider management trees (`session`, `auth`, `agent`, `mcp`, `github`,
  `db`, `debug`), and the operational commands (`completion`, `attach`,
  `upgrade`, `uninstall`, `models`, `stats`, `export`, `import`, `pr`).
- Executes opencode commands through `Opencode::execute`.
- Provides a typed parser for `opencode run --format json`'s event stream,
  keyed off the `session` / `session.error` / `result.error` / `result`
  events, tolerating both the single-JSON-object and the JSON-lines shapes.
- Preserves forward-compatible unknown output fields and event types behind a
  `#[non_exhaustive]` event enum with an `Other` catch-all.

## The exit code lies

opencode is one of the exit-0-but-failed CLIs: the process can exit `0` while
the run failed. The authoritative failure signal is the **presence of an error
event** (`session.error` or `result.error`) in the stream, not the exit code.
`RunOutput` reports that presence faithfully; deciding what it *means* for a
given consumer is left to the caller.

## Example

```rust
use opencode_cli::{Opencode, RunCommand};

# async fn example() -> Result<(), opencode_cli::Error> {
let opencode = Opencode::default().with_current_dir(".");
let command = RunCommand::message("Summarize this repository.").json();
let _output = opencode.execute(&command).await?;
# Ok(())
# }
```

## Versioning

`SUPPORTED_OPENCODE_VERSION` records the opencode version the surface was
authored against. Updating the supported CLI version should update the command
types, output models, and tests together.

Command option structs intentionally support Rust struct literals with
`..Default::default()` where that is ergonomic. Adding newly discovered CLI
fields to those public structs is therefore treated as a semver-major API
change for this crate.
