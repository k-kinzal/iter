# iter_cron_cli

`iter-cron` — single-process cron trigger that publishes signals into an iter queue.

## Overview

Reads a queue declaration from `--config <Iterfile>` or `--queue-url`, parses
`--schedule` as a cron expression (5-field standard or 6-field with seconds),
and emits one signal per scheduled tick.

Features:

- IANA timezone support (`--timezone Asia/Tokyo`).
- Emit on startup (`--at-startup`).
- Catch-up window for missed ticks (`--catch-up-window`).
- Random jitter before each tick (`--jitter`).
- Budget enforcement via `--max-signals`.

Lives until SIGTERM or budget exhaustion. Startup/shutdown banners go to
stderr; stdout is reserved.

## Workspace dependencies

`iter_trigger`, `iter_core`.

## Non-dependencies (explicit)

`iter_compose`, `iter_language` — this CLI is a standalone binary.
