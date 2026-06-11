# iter_core

The six concepts, running. Defines the core abstractions (Runner, Agent,
Workspace, Queue, Signal) and the iteration loop that binds them, and ships
standard drivers for Queue and Agent.

## Overview

- **Runner** — binds a Queue, Workspace, and Agent, consuming one Signal
  per iteration.
- **Agent** — which AI agent runs, and how it is invoked.
- **Queue** — the channel through which Signals enter a runner.
- **Workspace** — where and how the agent runs (core concept, not a driver).
- **Signal** — the unit of outside information a runner consumes, one per
  iteration.

## Workspace dependencies

`iter_tracing` only.

## External dependencies

Feature-gated. `cargo check -p iter_core --no-default-features` compiles
without any cloud SDK.

## Queue Drivers (`queue/drivers/`)

| Feature | Backend |
|---------|---------|
| `driver-sqs` | AWS SQS |
| `driver-redis` | Redis / Rediss |

The in-memory, file, and shell queue backends are always available. The
closed set of backends a declaration may name lives in the grammar (the
language layer), not here; declaring an unknown backend is an analysis-time
diagnostic.

## Agent Drivers (`agent/drivers/`)

All agent drivers compile unconditionally (process-based, no external SDK):

Claude, Codex, Gemini, Copilot, Cursor, Cline, OpenCode, Grok, Antigravity,
Hermes, Generic, Noop, Fake.

## Public API

Runner, Agent trait, Workspace trait, Queue trait, `queue::drivers::*`,
`agent::drivers::*`, Signal, Prompt, Template, HookEvent.
