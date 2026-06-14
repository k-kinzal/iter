# iter process promote

Alias: `iter promote`

Execute the pending disposition recorded by `source ... { disposition = defer { promote = ... } }`.

```sh
iter promote <PROCESS>
```

After the disposition succeeds, iter clears the pending source decision from the
process record.

See [`../../config/iterfile/source.md`](../../config/iterfile/source.md).
