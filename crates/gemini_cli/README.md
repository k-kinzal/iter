# gemini_cli

Type-safe Rust command builders and executor for Google's Gemini CLI
(`gemini`) `0.41.2`.

The crate models Gemini as its own CLI surface, not as adapter code for any one
consumer. Command families live in separate modules internally so future Gemini
version updates can be reviewed by the affected command area.

## Scope

- Models the root `gemini [query..]` run (interactive and non-interactive
  `-p/--prompt` forms) with its full option set — model selection, approval /
  yolo / sandbox modes, worktree, session resume / id, policy files, ACP, and
  the `-o/--output-format text|json|stream-json` selector.
- Models Gemini's management subcommand tree: `mcp` (add / remove / list /
  enable / disable), `extensions` (install / uninstall / list / update /
  disable / enable / link / new / validate / config), `skills` (list / enable /
  disable / install / link / uninstall), `hooks` (migrate), and `gemma`
  (setup / start / stop / status / logs).
- Executes Gemini commands through `Gemini::execute`.
- Provides a typed parser for `gemini -o json`'s single terminal record
  (`response` / `stats.tokens` / `error`) and a lossless event log for
  `gemini -o stream-json`'s newline-delimited event stream.
- Preserves forward-compatible unknown output fields and event types behind a
  `#[non_exhaustive]` event enum with an `Other` catch-all.

## Example

```rust
use gemini_cli::{Gemini, RunCommand};

# async fn example() -> Result<(), gemini_cli::Error> {
let gemini = Gemini::default().with_current_dir(".");
let command = RunCommand::prompt("Summarize this repository.").json();
let _output = gemini.execute(&command).await?;
# Ok(())
# }
```

## Versioning

`SUPPORTED_GEMINI_VERSION` records the Gemini version the surface was authored
against. Updating the supported CLI version should update the command types,
output models, and tests together.

Command option structs intentionally support Rust struct literals with
`..Default::default()` where that is ergonomic. Adding newly discovered CLI
fields to those public structs is therefore treated as a semver-major API
change for this crate.
