# iter completions

Generate a shell completion script.

## Usage

```sh
iter completions <SHELL>
```

Supported shells:

- `bash`
- `zsh`
- `fish`
- `powershell`
- `elvish`

## Behavior

The command renders a completion script from the CLI definition and writes it to
stdout. It does not install the script.

## Examples

Bash:

```sh
source <(iter completions bash)
```

Zsh:

```sh
iter completions zsh > ~/.zfunc/_iter
```

Fish:

```sh
iter completions fish > ~/.config/fish/completions/iter.fish
```

PowerShell:

```powershell
iter completions powershell | Out-String | Invoke-Expression
```

Elvish:

```sh
iter completions elvish > ~/.elvish/lib/iter-completions.elv
```

## Related

- [`index.md`](index.md)
