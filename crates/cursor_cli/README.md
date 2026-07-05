# cursor_cli

Type-safe Rust command builders and executor for the Cursor Agent CLI
(`cursor-agent`) `2026.03.11-6dfa30c`.

The crate models `cursor-agent` as its own CLI surface, not as adapter code for
any one consumer. Command families live in separate modules internally so
future `cursor-agent` version updates can be reviewed by the affected command
area.

## Scope

- Models the `cursor-agent` surface: the root agent run (interactive and the
  `--print` headless mode), and the subcommand tree — `login` / `logout` /
  `status`, `mcp` (`login` / `list` / `list-tools` / `enable` / `disable`),
  `models`, `about`, `update`, `create-chat`, `ls`, `resume`, `generate-rule`,
  and `install-shell-integration` / `uninstall-shell-integration`.
- Executes commands through `Cursor::execute`.
- Provides a typed parser for `cursor-agent --print`'s `json` and `stream-json`
  output, keyed off the terminal `result` record (session id, request id, final
  message, and usage) with a `type: "error"` fallback for failure diagnostics.
- Preserves forward-compatible unknown output fields and event types behind a
  `#[non_exhaustive]` event enum with an `Other` catch-all.

## Example

```rust
use cursor_cli::{Cursor, PrintCommand};

# async fn example() -> Result<(), cursor_cli::Error> {
let cursor = Cursor::default().with_current_dir(".");
let command = PrintCommand::json().prompt("Summarize this repository.");
let _output = cursor.execute(&command).await?;
# Ok(())
# }
```

## Output contract

`cursor-agent --print --output-format json` emits a single terminal `result`
record; `--output-format stream-json` emits a newline-delimited event stream
that ends with the same `result` record. `PrintOutput::parse` accepts both
shapes. On success the `result` record carries `session_id`, `request_id`, the
final `result` message, and a `usage` object. The record's `is_error` field is
hard-coded `false` in this CLI revision and therefore carries no information;
success is the *presence* of the terminal `result` record, and callers should
not consult `is_error`. On failure there is no terminal `result` record: the
error surfaces as a `type: "error"` record or on stderr.

## Versioning

`SUPPORTED_CURSOR_VERSION` records the `cursor-agent` version the surface was
authored against. Updating the supported CLI version should update the command
types, output models, and tests together.

Command option structs intentionally support Rust struct literals with
`..Default::default()` where that is ergonomic. Adding newly discovered CLI
fields to those public structs is therefore treated as a semver-major API change
for this crate.
