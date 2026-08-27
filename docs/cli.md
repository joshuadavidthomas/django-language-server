# Command-line interface

Installing django-language-server adds the `djls` executable. It can check templates in a terminal or start the language server.

## `djls check`

Check Django templates without an editor:

```console
$ djls check [OPTIONS] [PATH]...
```

Run the command from the Django project root so it can load `djls.toml`, `.djls.toml`, or `pyproject.toml` and discover the project environment.

### Input

| Input | Behavior |
|---|---|
| No path | Check templates in the directories discovered from Django settings |
| File paths | Check the named template files |
| Directory paths | Recursively check supported template files below those directories |
| `-` | Read one template from standard input; cannot be combined with paths |

File and directory scans recognize `.html`, `.htm`, and `.djhtml` files.

```bash
# Check all discovered template directories
djls check

# Check explicit files or directories
djls check templates/ app/templates/page.html

# Check standard input
printf '{{ value|default }}' | djls check -
```

### Options

| Option | Meaning |
|---|---|
| `--select CODE,...` | Enable the named diagnostic codes, overriding project configuration |
| `--ignore CODE,...` | Disable the named diagnostic codes |
| `-g, --glob PATTERN` | Include or exclude matching files; prefix exclusions with `!`; later patterns win |
| `-., --hidden` | Include hidden files and directories |
| `--no-ignore` | Do not read `.gitignore`, `.ignore`, and related ignore files |
| `-L, --follow` | Follow symbolic links |
| `-d, --max-depth DEPTH` | Limit directory traversal depth |
| `--color always\|auto\|never` | Control colored diagnostic output |
| `-q, --quiet` | Suppress output and report the result through the exit status |

`djls check` exits with status 0 when no enabled diagnostics are found and status 1 when diagnostics or a command error occur. Run `djls check --help` for the command's full help text.

The [pre-commit hook](pre-commit.md) runs this command on staged templates.

## `djls serve`

Start the Language Server Protocol server over standard input and output:

```console
$ djls serve
```

Editors normally start this command through their [LSP client configuration](clients/index.md).

To listen for one LSP client over TCP, provide an explicit address:

```console
$ djls serve --connection-type tcp --address 127.0.0.1:9257
```

The server exits when that client disconnects. DJLS does not choose a default TCP port.

## Formatting

Django template formatting is an editor feature. Enable it in [format configuration](configuration/index.md#format), then use the editor's format-document command. There is no `djls format` command.
