# iter_trigger

SDK for iter trigger CLIs — queue connection and signal emission.

## Overview

This crate provides the building blocks that trigger CLIs use to connect
to an iter queue and emit signals. It is an SDK, not a framework: it
provides capabilities but does not own `main()` or control the execution
loop.

- **Trigger** — signal emitter with emission counting and optional
  `max_signals` budget enforcement.
- **TriggerConfig** — signal defaults and termination policy.
- **TriggerEvent** — per-emission metadata builder.
- **QueueLoader** — resolves a queue URL (`memory://`, `file://`, `redis://`)
  into a ready-to-use queue handle.
- **QueueHandle** — opaque queue wrapper implementing `Queue`.

## Workspace dependencies

`iter_core` only.

## Non-dependencies (explicit)

- `iter_language` — the SDK takes resolved URLs, not config declarations.
- `iter_compose` — the SDK is consumed by trigger CLIs, not by compose.

## Public API

Trigger, TriggerConfig, TriggerEvent, QueueLoader, QueueHandle,
CountingQueue, install_shutdown_handler. Re-exports core signal types.
