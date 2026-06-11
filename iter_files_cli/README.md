# iter_files_cli

`iter-files` — single-process trigger that turns a list of paths into iter signals.

## Overview

Drains each `--from` source in order and publishes one signal per non-empty,
non-comment line. Sources are either `stdin` (default) or `path:<file>`.

Empty lines and lines beginning with `#` are skipped. Multiple `--from`
flags can be chained to process several sources sequentially.

Use `--no-exit-on-eof` to keep the process alive after draining all sources
(useful when paired with `--max-signals` for budget-aware operation).

## Workspace dependencies

`iter_core`.

## Non-dependencies (explicit)

`iter_compose`, `iter_language` — this CLI is a standalone binary.
