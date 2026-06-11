# iter_watch_cli

`iter-watch` — single-process filesystem watch trigger that publishes signals into an iter queue.

## Overview

Watches one or more paths for filesystem changes and emits signals when
files are created, modified, or removed. Supports:

- Glob-based include/exclude filtering.
- Configurable publish interval (events within an interval are merged).
- Per-file or batched emission modes.
- Backend selection (recommended, poll, or inotify/kqueue/FSEvents).

Per-file signals carry `path`, `kind`, and `timestamp` metadata.
Merged signals carry `files` (unique paths), `events` (ordered detail),
`changed_count`, and `event_count`.

## Workspace dependencies

`iter_core`.

## Non-dependencies (explicit)

`iter_compose`, `iter_language` — this CLI is a standalone binary.
