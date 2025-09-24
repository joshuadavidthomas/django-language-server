# Contributing

All contributions are welcome! Besides code contributions, this includes things like documentation improvements, bug reports, and feature requests.

You should first check if there is a [GitHub issue](https://github.com/joshuadavidthomas/django-language-server/issues) already open or related to what you would like to contribute. If there is, please comment on that issue to let others know you are working on it. If there is not, please open a new issue to discuss your contribution.

Not all contributions need to start with an issue, such as typo fixes in documentation or version bumps to Python or Django that require no internal code changes, but generally, it is a good idea to open an issue first.

We adhere to Django's Code of Conduct in all interactions and expect all contributors to do the same. Please read the [Code of Conduct](https://www.djangoproject.com/conduct/) before contributing.

### `Justfile`

The repository includes a [`Justfile`](./Justfile) that provides all common development tasks with a consistent interface. Running `just` without arguments shows all available commands and their descriptions.

<!-- [[[cog
import subprocess
import cog

output_raw = subprocess.run(["just", "--list", "--list-submodules"], stdout=subprocess.PIPE)
output_list = output_raw.stdout.decode("utf-8").split("\n")

cog.outl("""\
```bash
$ just
$ # just --list --list-submodules
""")

for i, line in enumerate(output_list):
    if not line:
        continue
    cog.out(line)
    if i < len(output_list):
        cog.out("\n")

cog.out("```")
]]] -->
```bash
$ just
$ # just --list --list-submodules

Available recipes:
    bumpver *ARGS
    check *ARGS
    clean
    clippy *ARGS
    fmt *ARGS
    lint          # run pre-commit on all files
    test *ARGS
    testall *ARGS
    dev:
        debug
        explore FILENAME="djls.db"
        inspect
        record FILENAME="djls.db"
    docs:
        build LOCATION="site" # Build documentation
        serve PORT="8000"     # Serve documentation locally
```
<!-- [[[end]]] -->
