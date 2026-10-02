# Logging

When something isn't working, the server's log files are the first place to look,
and they're useful to include in a bug report.

## Finding the logs

The server writes one log file per day, named `djls.log.YYYY-MM-DD`, in your
platform's cache directory:

| Platform | Directory |
| --- | --- |
| Linux | `$XDG_CACHE_HOME/djls`, or `~/.cache/djls` |
| macOS | `~/Library/Caches/djls` |
| Windows | `%LOCALAPPDATA%\djls\cache` |

The server keeps the seven most recent files and deletes older ones when it
starts and each time it starts a new file.

Your editor's output panel for the language server also shows the
server's main messages, but not debug output.

## Getting more detail

By default, the log files contain the server's informational messages, warnings,
and errors. To include debug output as well, set the `RUST_LOG` environment
variable for the server and restart it:

```sh
RUST_LOG=warn,djls=debug djls serve
```

How you set the environment depends on your editor; launching the editor from a
shell where `RUST_LOG` is exported works for most. Debug output only goes to the
log files, not the editor's output panel.

Debug output can be large, so unset `RUST_LOG` once you're done
troubleshooting.

## Sharing logs

Debug logs can include file paths and details about your project. Review a log
before attaching it to an issue.
