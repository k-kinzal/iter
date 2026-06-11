# iter_cli

The `iter` CLI binary — entry point for the iter agent control framework.

## Overview

This crate is the main entry point users interact with. It provides:

- `iter run` — run a single service from an Iterfile.
- `iter compose up` — orchestrate multiple services from a `compose.iter`.
- `iter compose down/ls/ps/config/validate` — lifecycle management.
- `iter trigger run` — launch trigger subprocesses.
- `iter stop/logs/inspect` — process management.
- Shell completions and help generation.

The CLI delegates the six domain concepts to `iter_core` and analysis to
`iter_language`. The composition layer — turning a definition into a running
`Runner`, and the multi-service compose run on top of those iter processes —
lives in this crate (absorbed from the former `iter_compose`, since compose
differs from `iter run` in cardinality, not kind). This crate also owns
argument parsing, output formatting, and process control.

## Workspace dependencies

`iter_core`, `iter_language`, `iter_tracing`, and the five Signal-source
binaries (`iter_cron_cli`, `iter_watch_cli`, `iter_command_cli`,
`iter_files_cli`, `iter_webhook_cli`) that the compose run spawns.
