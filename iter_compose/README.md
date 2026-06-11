# iter_compose

Composition layer bridging `iter_language` ASTs to `iter_core` Runner instances.

## Overview

This crate binds `iter_language` declarations to `iter_core` execution types.
It operates in two modes:

- **Runner mode** (`iter run`): one Iterfile produces one Runner.
- **Compose mode** (`iter compose up`): a `compose.iter` produces multiple
  Runners plus trigger CLI subprocesses.

Key responsibilities:

- **AST-to-runtime builders** (`queue.rs`, `agent.rs`, `workspace.rs`) —
  construct the runtime `Arc<dyn Queue>`, `AnyAgent`, and `AnyWorkspace` from
  parsed declarations.
- **Compose orchestration** (`compose/`) — load, plan, and run services
  concurrently with failure policy handling. Trigger CLIs are launched as
  subprocesses.
- **Process lifecycle** — registry bootstrap, finalization, and record
  management for `iter ps` / `iter stop` / `iter inspect`.
- **Project discovery** — find active orchestrators and project members.
- **Trigger CLI argv** (`trigger_argv.rs`) — builds `--queue-url` for
  trigger subprocess invocation.

## Workspace dependencies

`iter_core`, `iter_language`.

## Non-dependencies (explicit)

`iter_trigger` — separated by subprocess boundary.

## Public API

Build (queue/agent/workspace/runner), compose (plan/run), iterfile,
process lifecycle, project discovery.
