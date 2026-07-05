# hermes_cli

Type-safe Rust command builders and executor for Nous Research's Hermes Agent
CLI (`hermes`) `0.16.0`.

The crate models Hermes as its own CLI surface, not as adapter code for a
particular caller. Command families live in separate modules so future Hermes
version updates can be reviewed by the affected command area.

## The agent path is text-only

Hermes runs the model two ways, and **both are plain text** — there is no
machine-readable event stream for a turn:

- `hermes -z <PROMPT>` (`--oneshot`) — run a single prompt non-interactively and
  print *only* the final response text to stdout. No banner, spinner, tool
  preview, or session-id line. This is the scripted / pipe entry point.
- `hermes chat -Q [-q <QUERY>]` (`--quiet`) — the quiet chat mode for
  programmatic use: suppress the banner, spinner, and tool previews and print
  only the final response and session info.

A run's "output" is therefore its decoded stdout/stderr plus a typed
[`Exit`](crate::Exit) classification. Higher-level meaning — token-limit
notices, tracebacks, argparse rejections — is scanned from that text by the
caller, exactly as with a shell pipe.

## The two JSON surfaces

Only two Hermes commands emit structured output, and neither runs the agent:

- `hermes send --json` — deliver a message to a configured messaging platform
  and print a single JSON result object. Parsed by
  [`SendOutput`](crate::SendOutput).
- `hermes sessions export <OUT>` — export the SQLite session store to JSONL
  (one JSON object per line; `-` writes to stdout). Parsed leniently by
  [`SessionExport`](crate::SessionExport).

Both parsers preserve each JSON payload losslessly (as `serde_json::Value`) and
expose typed accessors, so an unrecognized field across Hermes versions is
retained rather than dropped.

## Example

```rust
use hermes_cli::{Hermes, RunCommand};

# async fn example() -> Result<(), hermes_cli::Error> {
let hermes = Hermes::default().with_current_dir(".");
let output = hermes.execute(&RunCommand::oneshot("Summarize this repository.")).await?;
if output.is_success() {
    println!("{}", output.stdout());
}
# Ok(())
# }
```

## Exit codes

Hermes' scripted mode overloads exit `0`: it means "a response was produced",
which includes empty output and most provider/model failures (Hermes
stringifies those into the response text rather than failing the process).
Exit `1` is an uncaught Python exception (launch / auth / config failure) whose
traceback lands on stderr; exit `2` is an argparse / one-shot validation
rejection. `hermes send` documents its own scheme: `0` ok, `1` delivery/backend
error, `2` usage error. [`Exit`](crate::Exit) classifies the raw disposition
(success, failure code, signal, or indeterminate); the code alone is not
authoritative for the agent path, so higher-level meaning is left to the
caller.

`-h` / `--help` is accepted by every command (argparse convention) and is not
modeled as a distinct field.

## Scope

The typed builders cover the agent path (root run and `chat`) and the
operator-facing command families: `send`, `sessions`, `mcp`, `config`, `tools`,
`model`, `auth` / `login` / `logout`, `status`, `version`, `update`, and
`completion`. Hermes' broader subcommand tree (gateway, proxy, doctor, skills,
…) is reachable through [`RawCommand`](crate::RawCommand), a typed escape hatch
that renders an arbitrary subcommand name with free-form arguments.

## Versioning

`SUPPORTED_HERMES_VERSION` records the Hermes version the surface was authored
against. Updating the supported CLI version should update the command types and
tests together.
