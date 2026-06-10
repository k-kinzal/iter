# AGENTS

NOTE: DO NOT ASK THE USER ANYTHING OUTSIDE OF PLAN MODE.

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

## Core Modeling

### The six nouns

iter's domain is six nouns; three form the exploration loop and three form its boundary:

- **Runner** — the unit of exploration. A Runner binds one Workspace with one Agent and repeats **iterations**. The iteration is the unit of execution — iter schedules and counts nothing smaller; the operations inside it are observable as lifecycle events but never run independently — and each iteration consumes exactly one Signal: dequeue → render the prompt → set up the Workspace → run the Agent → apply results back / tear down → record the outcome.
- **Workspace** — where and how the agent runs: which files it sees, under which isolation (local path, git worktree, sandbox). Half of Factor control lives here: what a Workspace lets the agent see.
- **Agent** — which AI agent runs and how it is invoked (the CLI subprocess, its mode, env, session handling). The other half of Factor control: what an Agent carries between iterations.
- **Signal** — the unit of outside information that crosses into a Runner.
- **Queue** — the channel that carries Signals (`memory://`, `file://`, `redis://`, …). Triggers publish onto it, Runners consume from it, and the operator can enqueue directly (`iter enqueue`).
- **Trigger** — a source of Signals: it turns events from outside the Runner — a schedule, appended lines, a polled command, filesystem changes, a webhook — into Signals on a Queue.

A Runner alone holds exploration inside its own radius. When that radius is too narrow, Triggers bring in what the Runner cannot reach from within, widening the circle through Signals on its Queue.

### The iter process

How a Runner is run and managed is the CLI's concern; its unit of management is the **iter process** — what `iter ps` lists, what `iter stop` targets, what the run record under `~/.iter/proc` remembers. The model is Docker's: an iter process is to its Runner what a container is to the process it runs — `iter run`/`ps`/`logs`/`stop`/`rm` mirror their `docker` counterparts, and `iter compose` shells over iter processes the way Docker Compose shells over containers (services, projects, labels, bulk `up`/`down`).

- **One iter process ↔ one Runner**, always. A compose service registers its own record exactly the way an `iter run` invocation does.
- **An iter process is not an OS process** — an OS process embodies it. In-process compose services share the orchestrator's pid, yet each is its own iter process; a detached run gets its own OS process. Liveness is pid plus start-time fingerprint, never pid alone. The `compose up` orchestrator is an OS process but not an iter process — it hosts several, and is deliberately absent from the registry. Triggers are not iter processes either: they host no Runner; they are Signal sources.
- **The iter process is the center; compose is the outer shell.** An iter process works without compose (`iter run`), and compose depends on iter processes, never the reverse. A **service** is a named exploration declaration that yields exactly one iter process; a compose run is the integration of several iter processes plus supervised triggers, started and stopped as a unit (`up`/`down`). Compose's knowledge of its children rides in generic record labels (`iter.compose.*`); the record schema knows nothing of compose.

### The concept map

A declaration becomes a managed run in six phases — declare → analyze → plan → start → run → operate. The concepts line up along that pipeline; the crate in parentheses is where each lives today.

- **The declaration** — the text as written: an `Iterfile` or a `*.iter` compose file. Declaring is the user's act; everything after is iter's.
- **The definition** — what survives **analysis** of a declaration: lexer → parser → semantic analyzer; references resolved, defaults applied, illegal forms rejected, all diagnostics accumulated (`iter_language` — independent of `iter_core` by design; `iter_compose` binds the two).
- **The plan** — definitions completed with **run-time inputs**: secrets resolved, args substituted, overrides applied. Fully decided, still data — nothing live in it (`iter_compose`).
- **The start** — bringing a plan to life: each of the Runner's collaborators is made from its definition, and the Runner begins iterating (`iter_compose`, dispatched by `iter_cli`).
- **The run** — the six nouns at work: the Runner consumes Signals, one per iteration (`iter_core`; Trigger implementations live in `iter_trigger` and the five `iter_{command,cron,files,watch,webhook}_cli` binaries — each Signal-source kind its own standalone binary, though under compose today they are linked in and run as supervised tasks inside the orchestrator).
- **Operating** — the **operator** (the human; `iter_cli` is their surface) manages the running Runner as an **iter process**. The **run record** under `~/.iter/proc` is the operator's durable memory — which OS process embodies which iter process, its status, its output, its labels — and **discovery** is its read side: how `ps`/`logs` find what is running and what ran (surface in `iter_cli`; the record implementation sits in `iter_core::process` and `iter_compose` today).
- **Compose** — the outer shell over iter processes: the **service** (a named exploration declaration yielding exactly one iter process), the **project** (the named grouping of a compose run's managed processes — the slug and the `iter.compose.*` labels), the **orchestrator** (the `compose up` OS process: hosts or spawns services, supervises triggers, is not itself an iter process), and per-trigger **checkpoints** (a trigger's resume memory) (`iter_compose`).
- **Observability** — exported telemetry: traces of iterations and lifecycle events (`iter_tracing`).

Crate dependencies run one way: `iter_cli` → `iter_compose` → `iter_core` / `iter_language` / the five trigger crates; the trigger crates → `iter_trigger` → `iter_core`; `iter_core` and the operator crates use `iter_tracing`; `iter_language` depends on nothing internal; nothing depends on `iter_cli`.

## Related Projects

- **~/Projects/agent-loop:** Predecessor to iter's agent control implementation.
- **~/Projects/repository-monitoring-agent:** A distributed system that explores repositories by other methods.
- **~/Projects/spec-oracle:** Generates specifications as an outcome of exploration.
