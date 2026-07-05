# claude_code_cli

Type-safe Rust command builders and executor for Claude Code CLI
`2.1.178`.

The crate models Claude Code as its own CLI surface, not as iter-specific
adapter code. Command families live in separate modules internally so future
Claude Code version updates can be reviewed by the affected command area.

## Scope

- Models Claude Code commands, subcommands, options, and positionals.
- Executes Claude Code commands through `ClaudeCode::execute`.
- Provides typed parsers for `--print --output-format json` and
  `--print --output-format stream-json`.
- Preserves forward-compatible unknown output fields and event types.

## Example

```rust
use claude_code_cli::{
    ClaudeCode, ExecuteCommand, PermissionMode,
};

# async fn example() -> Result<(), claude_code_cli::Error> {
let claude = ClaudeCode::default().with_current_dir(".");
let mut command = ExecuteCommand::prompt("Summarize this repository.");
command.permission_mode = Some(PermissionMode::BypassPermissions);

let _result = claude.execute(&command.json()).await?;
# Ok(())
# }
```

## Versioning

`SUPPORTED_CLAUDE_CODE_VERSION` records the Claude Code version the surface was
authored against. Updating the supported CLI version should update the command
types, output models, and tests together.

Command option structs intentionally support Rust struct literals with
`..Default::default()` where that is ergonomic. Adding newly discovered CLI
fields to those public structs is therefore treated as a semver-major API
change for this crate.
