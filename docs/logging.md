# Logging

`djls serve` uses tracing events for both diagnostic files and the editor's output
panel through `window/logMessage`. The destinations have independent filters and
bounded workers so a slow editor or noisy dependency cannot block request handling
or create one task per event.

The editor receives ERROR, WARN, and INFO events only from DJLS runtime crates,
rendered as the message followed by the event's `key=value` fields. Dependency,
transport, DEBUG, and TRACE events remain file-only. Editor messages are limited
to 4 KiB, wait in a lossy 256-record queue, and are sent by one worker. Saturated
or unavailable editor logging drops messages silently rather than emitting
another tracing event. Sending a
notification means handing it to the LSP transport, not confirming editor display.

Editors supporting work-done progress still receive Begin/Report/End notifications.
If progress is unsupported, rejected, times out, or creation is cancelled, it is
reported through the bounded tracing fallback instead.
Cancelling an item that already sent Begin still sends End.

## File location

Files are named `djls.log.YYYY-MM-DD` in the platform application cache directory:

| Platform | Default directory |
| --- | --- |
| Linux | `$XDG_CACHE_HOME/djls`, or `~/.cache/djls` |
| macOS | `~/Library/Caches/djls` |
| Windows | `%LOCALAPPDATA%\djls\cache` |

If no platform cache directory is available, DJLS uses `/tmp`. Failure to create
the directory falls back to stderr. Logs never use stdout, which carries LSP.
The logging worker stays alive through service and runtime teardown.

Rotation is currently **daily only**, with no byte-size cap or automatic retention
limit. This change reduces default volume but does not bound disk usage. Remove
old logs as needed; byte-size rotation is a separate planned change.

## Filtering

In files, dependencies including Salsa and `tower_lsp_server` default to WARN and
ERROR. These runtime targets additionally accept INFO: `djls_server`, `djls_conf`,
`djls_db`, `djls_ide`, `djls_project`, `djls_source`, `djls_semantic`,
`djls_templates`, and `djls_format` (including their module targets).

A valid `RUST_LOG` in the server process environment replaces these defaults.
For example, to debug the server while leaving dependencies quiet:

```sh
RUST_LOG=warn,djls_server=debug djls serve
```

Unset, empty, non-Unicode, or invalid values silently fall back to the defaults; DJLS
does not print the environment value. `RUST_LOG` changes only file filtering and
cannot enable dependency, DEBUG, or TRACE events in the editor. Broad overrides
such as `RUST_LOG=info` or `RUST_LOG=trace` can still produce very large files
until byte-size rotation is added. Prefer narrow targets and remove diagnostic
overrides after troubleshooting. Review logs for sensitive paths or project
details before sharing them.
