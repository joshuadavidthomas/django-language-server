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

Files live in the platform application cache directory:

| Platform | Default directory |
| --- | --- |
| Linux | `$XDG_CACHE_HOME/djls`, or `~/.cache/djls` |
| macOS | `~/Library/Caches/djls` |
| Windows | `%LOCALAPPDATA%\djls\cache` |

If no platform cache directory is available, DJLS uses `/tmp`. Logs never use
stdout, which carries LSP. The logging worker stays alive through service and
runtime teardown.

## Hard limits and failure policy

The managed family is `djls-bounded.log` (active), `djls-bounded.log.1` (newest
archive), `.2`, and `.3` (oldest). Each file is at most **16 MiB**, for a maximum
**64 MiB of managed payload**. Before a record would exceed the active limit,
DJLS removes `.3`, moves `.2` to `.3`, `.1` to `.2`, and active to `.1`, then
opens a new active file. Exact-limit records do not rotate until the next write.
On the next accepted write after restart, existing oversized managed files are
truncated to 16 MiB before rotation; actual file lengths, not cached counters,
determine space available. If repair fails, logging stops rather than growing
the family. Pre-existing oversized data cannot be bounded until repair succeeds.

Cooperating upgraded processes share this budget using `djls-bounded.lock`.
Each record acquires its own exclusive OS file lock, checks file lengths, rotates
if needed, writes, and closes active before releasing the lock. The background
worker waits for the lock; if another process holds it for long, the lossy queue
fills and new records are dropped instead of blocking the server. **Do not delete the lock file while any
server is running.** The bound does not cover non-cooperating writers, filesystem
metadata/allocation overhead, or files outside this family.

Formatted records above **16 KiB** are dropped in full before enqueueing. Each
process's lossy **1,024-record** queue retains at most **16 MiB of record payload**,
plus queue metadata and one worker record. Formatting itself may temporarily allocate
a larger string; this is a retained-output bound, not a process-memory limit.
Oversize, full-queue, and unavailable-output drops, plus filesystem failures, have separate saturating in-process counters;
drops never emit tracing events. These internal counters are not currently
exposed through LSP.

Any filesystem error disables that process's file sink for the rest of its
lifetime, with at most one direct stderr warning and no fallback event stream.
A partial failed write may leave a partial final record, but cannot exceed the
file limit. Restarting retries file output. A private worker closes admission
on shutdown, drains the finite queue, flushes, and joins without a shutdown
message competing for queue space. Shutdown waits for outstanding filesystem
operations; it cannot promise a time limit on a stalled filesystem.

Daily `djls.log.YYYY-MM-DD` files written by earlier releases are deleted at
startup once they have not been modified for a day; a recently written one is
kept in case an older server is still running, and is removed on a later start.
Unrelated cache files are never touched.

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
cannot enable dependency, DEBUG, or TRACE events in the editor or raise disk,
record, or queue limits. Verbose overrides rotate history faster and may drop
records. Prefer narrow targets and remove diagnostic overrides after
troubleshooting. Review logs for sensitive paths or project details before
sharing them.

## Instrumentation model

DJLS instruments operation boundaries rather than individual Salsa queries. INFO
records cover low-frequency lifecycle summaries such as `Project reload completed`.
DEBUG spans cover LSP requests and notifications, Session snapshots and mutations,
project reload phases and cache warm-up, and diagnostic publication and refresh.
Blocking computations run inside their operation's span, so their records remain
attributable without holding an entered span across an await.

Records follow shared conventions so they can be queried together:

- Span names are `area.operation`, for example `lsp.request`,
  `session.ready_snapshot`, `project.reload`, `ide_cache.warmup`, and
  `diagnostics.publication`.
- `outcome` uses one vocabulary: `success`, `empty`, `cancelled`, `failed`,
  `blocking_task_failed`, `superseded`, `stale`, `skipped`, and `retried`, plus
  operation-specific values such as `ignored` or `accepted`.
- `elapsed_ms` is the whole operation's duration. Parts of it are separate
  fields: `ready_wait_ms` (waiting for project readiness), `compute_ms`
  (computation inside blocking work, excluding time queued for a worker), and
  `transport_ms` (LSP transport). All timings are fractional milliseconds.

A transport `success` means the send future completed, not that an editor
displayed or acted on the message. Cancellation, stale-generation suppression,
unavailable payloads, and partial work are recorded as outcomes instead of being
inferred from missing completion events.

Default-visible (INFO, WARN, and ERROR) records avoid raw project paths, URIs,
environment names or values, client option contents, source-derived module
names, panic payloads, and arbitrary error strings. Where a warning needs that
detail to be actionable, a DEBUG record next to it carries it, so
`RUST_LOG=warn,djls_project=debug` (or the relevant crate) shows what failed.
DEBUG, TRACE, and third-party targets enabled through `RUST_LOG` can contain
paths and implementation detail, so inspect any log before sharing it.
