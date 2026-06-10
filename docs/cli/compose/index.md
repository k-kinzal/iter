# Compose Commands

Compose commands operate a compose project declared by `compose.iter`.

A compose project starts service runners and supervised triggers around one or
more queues. The compose orchestrator is not itself an iter process. Service
runners are registered as iter processes and carry `iter.compose.*` labels so
runtime commands can reconstruct project state.

Project names are derived in this order:

1. `-p`, `--project-name <NAME>`.
2. `COMPOSE_PROJECT_NAME`, when set.
3. The canonical basename of the compose file's parent directory.

Project names are normalized to lowercase `[a-z0-9_-]` form.

| Command | Purpose |
| --- | --- |
| [`iter compose up`](up.md) | Start services and triggers from a compose file. |
| [`iter compose config`](config.md) | List the static resources declared by a compose file. |
| [`iter compose ls`](ls.md) | List active compose projects. |
| [`iter compose ps`](ps.md) | List runners and trigger status for one project. |
| [`iter compose down`](down.md) | Stop a project or selected services. |
| [`iter compose validate`](validate.md) | Parse and semantic-check a compose file. |

Use compose commands when the operational target is the project. Use
[`process`](../process/index.md) commands when the operational target is one
runner process.

Compose file syntax is documented in [`../../config/compose.md`](../../config/compose.md).
