# iter_webhook_cli

`iter-webhook` — single-process HTTP webhook trigger that publishes signals into an iter queue.

## Overview

Spawns an HTTP server that accepts POST requests and routes them to signals
based on configurable event matching. Designed for GitHub webhooks but
works with any JSON-posting service.

Features:

- HMAC-SHA256 signature verification (`X-Hub-Signature-256`).
- Event routing via `<event>.<action>` pattern matching.
- Per-route guard expressions for fine-grained filtering.
- Handlebars metadata templates rendered against the request body.
- Configurable bind address and path.

## Workspace dependencies

`iter_trigger`, `iter_core`.

## Non-dependencies (explicit)

`iter_compose`, `iter_language` — this CLI is a standalone binary.
