# Compose Hooks

Compose Hooks run Shell actions at lifecycle points visible to the Compose
orchestrator. They are separate from Runner Hooks: Compose never observes a
Runner's completion condition, iteration count, Signal, Workspace, or Agent
lifecycle.

```hcl
on services_completed {
  services = [explorer_a, explorer_b, explorer_c, explorer_d]
  shell "scripts/evaluate-reports.sh"
}

on service_failed {
  services = [explorer_a, explorer_b, explorer_c, explorer_d]
  shell "scripts/report-failure.sh {{service.name}}"
}
```

Handlers and their `shell` actions execute in source order. Multiple handlers
for the same event are allowed.

## Selectors

`services` and `triggers` are lists of bare declaration names:

```hcl
services = [planner, implementer]
triggers = [nightly, repository_watch]
```

- A service event accepts `services`; a Trigger event accepts `triggers`.
- Omitting the selector means every managed declaration of that kind in the
  flattened plan.
- If that kind has no declarations, an omitted-selector handler has no
  resource transition to drive it and does not fire.
- An empty selector, duplicate name, unknown name, quoted name, or selector on
  a Compose-wide event is a validation error.
- For a single-resource event, the selector filters which resource instances
  fire the handler.
- For an aggregate event, the selector is the exact rendezvous set. Each
  aggregate handler fires at most once per Compose run.

Hooks declared by a nested `compose` file are not imported. Importing a
Compose declaration flattens its resources into the parent, but does not create
a second orchestrator whose lifecycle could drive those hooks. Hooks on the
root Compose declaration may select imported resource names.

## Compose run events

| Event | Fires |
| --- | --- |
| `compose_starting` | After the plan is built and before the first resource starts. |
| `compose_started` | After every initial service has reached `Running` and every Trigger has reached `Running` once. |
| `compose_completing` | After every managed task completed normally and before queues are closed. |
| `compose_completed` | After normal shutdown and queue close complete. |
| `compose_failing` | When the first fatal managed-task termination is known, before applying the failure policy. |
| `compose_failed` | After failure handling, remaining resource shutdown, and queue close complete. |
| `compose_stopping` | After an external stop request and before cancellation is forwarded to managed resources. |
| `compose_stopped` | After externally requested resource shutdown and queue close complete. |

`compose_completed`, `compose_failed`, and `compose_stopped` are mutually
exclusive. Parse, semantic-analysis, and plan-construction failures happen
before the orchestrator exists and do not fire hooks.

Natural completion requires every managed service to stop normally and every
managed Trigger to reach `Completed`. A long-running Trigger therefore keeps
the Compose run active until an external stop or a fatal failure cancels it.

## Service events

These events are derived only from the iter process state managed by Compose.

| Event | Fires |
| --- | --- |
| `service_starting` | Immediately before starting the service's iter process. |
| `service_started` | After the iter process reaches `Running`. |
| `service_completed` | After the iter process reaches `Stopped`. |
| `service_failed` | After start, run, monitor, or finalization fails. |
| `service_killed` | After external stop or orchestrator cancellation reaches `Killed`. |

## Trigger events

These events observe an existing Compose-managed Trigger supervisor; they do
not define a new Trigger kind.

| Event | Fires |
| --- | --- |
| `trigger_starting` | Immediately before the initial supervisor start. |
| `trigger_started` | Every time the Trigger reaches `Running`, including after restart. |
| `trigger_restarting` | When the supervisor enters restart backoff. |
| `trigger_completed` | When a finite Trigger completes normally. |
| `trigger_failed` | When a Trigger becomes permanently failed. |
| `trigger_stopped` | When orchestrator shutdown stops the Trigger. |

## Aggregate events

| Event | Condition |
| --- | --- |
| `services_started` | Every selected service reached `Running` at least once. |
| `services_completed` | Every selected service reached `Stopped`. |
| `services_failed` | The first selected service became failed. |
| `services_settled` | Every selected service is completed, failed, or killed. |
| `triggers_started` | Every selected Trigger reached `Running` at least once. |
| `triggers_completed` | Every selected Trigger reached `Completed`. |
| `triggers_failed` | The first selected Trigger became permanently failed. |
| `triggers_settled` | Every selected Trigger is completed, failed, or stopped. |

For `services_failed` and `triggers_failed`, the individual `service.*` or
`trigger.*` context identifies the first failing resource. Other aggregate
events expose the resource transition that satisfied the rendezvous as the
individual context as well.

## Shell contract

Compose Hook Shell actions use the same execution contract as Runner Hook
Shell actions:

- `sh -c <command>`
- null stdin and inherited stdout/stderr
- best effort: a non-zero exit is logged and does not change Compose state
- source-order execution

The cwd is the directory containing the root Compose file.

Templates always expose:

| Root | Fields |
| --- | --- |
| `event` | `name` |
| `compose` | `project`, `file` |
| `today` | local `YYYY-MM-DD` date |

Contextual roots are:

| Root | Fields |
| --- | --- |
| `service` | `name`, `status` |
| `trigger` | `name`, `status`, `restart_count` |
| `services` | comma-separated `names`, `count` |
| `triggers` | comma-separated `names`, `count` |
| `error` | `message` |

Referencing a contextual root that is absent for the current event logs a
template-render warning and skips that action.

The same context is exported for ordinary Shell scripts:

- `ITER_COMPOSE_EVENT`, `ITER_COMPOSE_PROJECT`, `ITER_COMPOSE_FILE`
- `ITER_COMPOSE_SERVICE`, `ITER_COMPOSE_SERVICE_STATUS`
- `ITER_COMPOSE_TRIGGER`, `ITER_COMPOSE_TRIGGER_STATUS`,
  `ITER_COMPOSE_TRIGGER_RESTART_COUNT`
- `ITER_COMPOSE_SERVICES`, `ITER_COMPOSE_TRIGGERS`
- `ITER_COMPOSE_ERROR`

Variables that are not meaningful for the current event are set to an empty
string so inherited parent values cannot leak into a hook.
