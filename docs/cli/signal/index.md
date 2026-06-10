# Signal Commands

Signal commands push outside information into a queue.

| Command | Alias | Purpose |
| --- | --- | --- |
| [`iter signal push`](push.md) | `iter enqueue` | Create one signal and enqueue it. |

Use signal commands when an operator or script needs to inject work manually.
Triggers are the long-running sources of signals in compose projects.
