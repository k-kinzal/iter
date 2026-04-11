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

The CLI delegates all domain logic to `iter_compose` and `iter_core`;
this crate owns argument parsing, output formatting, and process control.

## Workspace dependencies

`iter_compose`, `iter_core`, `iter_language`.
