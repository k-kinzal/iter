# iter_language

Parser, AST, and semantic analyzer for the iter workflow definition language.

## Overview

This crate turns `Iterfile` and `compose.iter` source text into typed ASTs
that downstream crates (`iter_compose`, `iter_cli`) consume. It owns:

- The grammar (parsed via `chumsky`).
- The AST types (`Root`, `ComposeRoot`, declarations for queues, agents,
  workspaces, runners, services, triggers, prompts, and event handlers).
- Semantic validation (e.g. resolving queue references, checking trigger
  schedule expressions).
- Diagnostic rendering (via `ariadne`).

No runtime behaviour lives here — this crate is pure data transformation.

## Workspace dependencies

None — leaf crate.

## External dependencies

`ariadne` (diagnostic rendering).

## Public API

`parse`, AST types (IterfileDecl, ComposeDecl, QueueDecl, AgentDecl,
WorkspaceDecl, TriggerDecl, etc.), diagnostic rendering.
