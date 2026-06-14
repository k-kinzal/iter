# iter process discard

Alias: `iter discard`

Drop the parked base recorded by `source ... { disposition = defer { ... } }`
without touching the canonical source.

```sh
iter discard <PROCESS>
```

After the discard succeeds, iter clears the pending source decision from the
process record.

See [`../../config/iterfile/source.md`](../../config/iterfile/source.md).
