# iter_core

The iter engine. Defines the core abstractions (Runner, Agent, Workspace,
Queue), the runner engine, and ships standard drivers for Queue and Agent.

## Overview

- **Runner** — binds a Queue, Workspace, and Agent, iterating one signal
  per turn.
- **Agent** — which AI agent runs, and how it is invoked.
- **Queue** — the channel through which signals enter a runner.
- **Workspace** — where and how the agent runs (core concept, not a driver).

## Workspace dependencies

None — leaf crate.

## External dependencies

Feature-gated. `cargo check -p iter_core --no-default-features` compiles
without any cloud SDK.

## Queue Drivers (`queue/drivers/`)

| Feature | Backend |
|---------|---------|
| `driver-sqs` | AWS SQS |
| `driver-kinesis` | AWS Kinesis |
| `driver-pubsub` | Google Cloud Pub/Sub |
| `driver-servicebus` | Azure Service Bus |
| `driver-kafka` | Apache Kafka (via librdkafka) |
| `driver-redis` | Redis / Rediss |

The in-memory, file, and shell queue backends are always available.

## Agent Drivers (`agent/drivers/`)

All agent drivers compile unconditionally (process-based, no external SDK):

Claude, Codex, Gemini, Copilot, Cursor, Cline, OpenCode, Grok, Generic.

## Public API

Runner, Agent trait, Workspace trait, Queue trait, `queue::drivers::*`,
`agent::drivers::*`, Signal, Prompt, Template, Event.
