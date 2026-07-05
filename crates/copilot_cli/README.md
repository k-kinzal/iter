# copilot_cli

Type-safe Rust command builders and executor for the GitHub Copilot CLI
(`copilot`) `1.0.49`.

The crate models Copilot as its own CLI surface, not as adapter code for any
one consumer. Command families live in separate modules internally so future
Copilot version updates can be reviewed by the affected command area.

## Scope

- Models Copilot commands, subcommands, options, and positionals — the root
  run (interactive, or one-shot with `-p/--prompt`, with the full option set:
  session control, model/reasoning, mode, permissions, MCP, output, and
  sharing), plus `mcp` (`add`/`get`/`list`/`remove`), `plugin`
  (`install`/`list`/`uninstall`/`update`/`marketplace`), `completion`,
  `login`, `update`, `version`, `init`, and `help`.
- Executes Copilot commands through `Copilot::execute`.
- Provides a typed parser for `copilot --output-format json`'s JSONL event
  stream (one JSON object per line), keyed off the terminal `result` and
  `session.error` records.
- Preserves forward-compatible unknown output fields and event types behind a
  `#[non_exhaustive]` event enum with an `Other` catch-all.

There is **no `suggest` subcommand**: the root `copilot` command *is* the run
(the `suggest` verb belonged to the older `gh copilot` extension).

## Example

```rust
use copilot_cli::{Copilot, RunCommand};

# async fn example() -> Result<(), copilot_cli::Error> {
let copilot = Copilot::default().with_current_dir(".");
let command = RunCommand::prompt("Summarize this repository.").json();
let _output = copilot.execute(&command).await?;
# Ok(())
# }
```

## Versioning

`SUPPORTED_COPILOT_VERSION` records the Copilot version the surface was
authored against. Updating the supported CLI version should update the command
types, output models, and tests together.

Command option structs intentionally support Rust struct literals with
`..Default::default()` where that is ergonomic. Adding newly discovered CLI
fields to those public structs is therefore treated as a semver-major API
change for this crate.
