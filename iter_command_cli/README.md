# iter_command_cli

`iter-command` — single-process command-poll trigger that publishes signals into an iter queue.

## Overview

Runs `--run` under `--shell` (default `sh -c`) on a fixed `--poll-secs`
interval, applies `--extract` to captured stdout, and publishes one signal
per extracted record.

Extraction modes:

- `lines` (default) — each non-empty stdout line becomes a signal.
- `json-array` — parse stdout as a JSON array; each element becomes a signal.
- `regex:<pattern>` — named capture groups become signal metadata fields.

Supports deduplication (`--dedupe`) to suppress re-emission of unchanged
records across polls, and configurable error handling (`--on-error`).

## Workspace dependencies

`iter_core`.

## Non-dependencies (explicit)

`iter_compose`, `iter_language` — this CLI is a standalone binary.
