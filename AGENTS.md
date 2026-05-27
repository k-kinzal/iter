# AGENTS

NOTE: DO NOT ASK USER

## Motivation

AI Agent tooling today is built for *certainty*. Context engineering, spec-driven development, and harness engineering all work by converging an agent on a specified goal.

Software engineering carries a second tradition, built for *uncertainty* — spiral, Lean, Set-Based Design, probe-sense-respond. Its move is the opposite of convergence: defer commitment, preserve options, let evidence do the narrowing.

iter brings that second tradition to AI Agents. It is a foundation for exploration, learning, and execution as a cycle — so agents can work where the goal itself is still taking shape.

## Core Concepts

Exploration is shaped by **Factors** — workspace files, git history, file timestamps, agent session logs, continuous context persistence. The breadth and depth of an exploration change with which Factors are present in the loop.

1. **Wide & Shallow:** Workspace only.
2. **Standard:** Workspace, plus git history and file timestamps.
3. **Narrow & Deep:** Standard, plus agent session logs.
4. **Very Narrow & Deep:** Narrow & Deep, plus continuous context persistence.

These are illustrative. Finer control comes from adding other elements such as progress tracking (e.g., ralph-loop).

An agent that holds knowledge of its past work becomes fixated on those actions and repeats similar patterns. Fixation deepens exploration along one path while narrowing its radius; widening the radius shallows each path.

Breadth here is relative, not absolute. A workspace is required for exploration, yet its presence biases the agent to treat the current state as truth. Absolute breadth cannot be reached from within.

The parable of the Circle of Sin, from *Haibane Renmei*:

> "To admit your sin is to have no sin. This is the riddle of the Circle of Sin. Consider this: if one who admits their sin has no sin, then I ask you — are you a sinner?"

The circle will not yield from within. An agent's own biases — the context it accumulates, the factors it weights — bend every next step back into the same radius. Widening requires something from outside. iter does not resolve this philosophically; it sets a stepping stone where that can happen.

## Architecture

iter is a CLI tool. The engine lives in Core; the surface syntax lives in a separate Language.

    CLI ──── Core ──── Language

Core is built from three concepts:

- **Runner** — the unit of exploration. An Runner binds a **Workspace** (where and how the agent runs) with an **Agent** (which AI agent runs, and how it is invoked), and iterates over them. Exploration breadth is controlled at this layer: what a Workspace lets the agent see, and what an Agent carries between turns.

- **Signal / Queue** — the boundary through which external information enters an Runner. A **Signal** is the unit that crosses the boundary; a **Queue** is the channel that carries it. Each turn of an Runner consumes one Signal.

- **Trigger** — a source of Signals. A Trigger watches something outside the Runner — a schedule, a file change, a webhook — and turns it into Signals on a Queue.

An Runner alone holds exploration inside its own radius. When that radius is too narrow, Triggers bring in what the Runner cannot reach from within, widening the circle through Signals on its Queue.

## Project Tradeoff Sliders

- Scope     ●————————→ HIGH — Full intended scope is delivered; corners are not cut.
- Quality   ●————————→ HIGH — Correctness, test coverage, and strict static analysis come first.
- Time      ←————————● LOW — No deadline pressure.
- Cost      ←————————● LOW — Resource constraints are not a concern.

Quality takes precedence when in doubt. Less shipped with confidence beats more shipped with uncertainty.

It is a given that the code works. Beyond that, it must be designed with separation of concerns and the single responsibility principle, as well as appropriate module management and layering. Please note that this is not about simply breaking things into small pieces. Aim for a level of granularity based on concepts and behaviors.

## Protected Files

`.sloc-guard.toml` is a protected configuration file. Do not modify it.

## Related Projects

- **~/Projects/agent-loop:** Predecessor to iter's agent control implementation.
- **~/Projects/repository-monitoring-agent:** A distributed system that explores repositories by other methods.
- **~/Projects/spec-oracle:** Generates specifications as an outcome of exploration.
