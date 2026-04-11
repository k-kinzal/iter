# iter_watch_cli

`iter-watch` — single-process filesystem watch trigger that publishes signals into an iter queue.

## Overview

Watches one or more paths for filesystem changes and emits signals when
files are created, modified, or removed. Supports:

- Glob-based include/exclude filtering.
- Configurable debounce (cooldown between emissions for the same path).
- Per-file or batched emission modes.
- Backend selection (recommended, poll, or inotify/kqueue/FSEvents).

Each emitted signal carries `path`, `kind`, and `timestamp` metadata
describing the change event.

## Workspace dependencies

`iter_trigger`, `iter_core`.

## Non-dependencies (explicit)

`iter_compose`, `iter_language` — this CLI is a standalone binary.
