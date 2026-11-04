# Edit and LSP measurement contracts

These workloads supplement existing benchmarks. They do not rename or replace
`validate_incremental`, `validate_primed_project_cold_templates`, or the
`real_project` benchmarks.

## Semantic validation

Each new semantic benchmark has separate `medium/admin_login.html` and
`large/views_technical_500.html` cases. One sample validates eight Templates or
applies eight edits. Divide sample time or instructions by eight for a per-operation
figure.

| Benchmark | Timed operation |
| --- | --- |
| `validate_incremental_text_edit` | Alternate a one-byte insertion and deletion in markup before later template syntax. |
| `validate_incremental_same_length_text_edit` | Alternate a same-length replacement at the same anchor. |
| `validate_incremental_semantic_edit` | Alternate a one-byte change to an `if` expression. |
| `validate_new_template_warm_project` | Validate eight pre-registered, never-validated Templates after validating another Template with identical source in the same primed database. |

The admin text edits use the first `<p class="errornote">`, replacing `errornote`
with `errornotex` or `errornotx`. The large-template text edits replace the first
`Django Version:</th>` with `Django Versionx:</th>` or `Django Versioz:</th>`.
The semantic edits change `form.errors` to `form.errorsx` in the first
`{% if form.errors and not` or `raising_view_name` to `raising_view_namex` in
`{% if raising_view_name %}`. Both directions run before timing, with diagnostic
codes checked for stability. Each measured edit includes source synchronization
and validation, but not LSP document mutation or transport.

New-template setup registers the entire pool before warming the comparison
Template. Registration, Project priming, and validation of that comparison
Template are outside timing. Target parsing and validation remain cold. No pool
member is reused within an input. Database destruction is outside timing.

## Transport-independent mutation and UTF-16

The `lsp` target runs in the existing core simulation and memory shards. It has no
corpus dependency.

| Benchmark | Timed operation |
| --- | --- |
| `apply_sequential_changes` | One `AnalysisHost::update_document` with 1, 8, or 32 ranged changes in a 512-line Unicode/CRLF Template. |
| `position_to_offset_utf16` | 384 incoming-position conversions on an already-built line index. |
| `diagnostics_utf16` | Cached semantic collection and conversion of 129 diagnostic ranges to outgoing UTF-16 positions. |

Mutation first inserts a Unicode line at line 128. Later ranges address the
shifted document and replace variable names after accented and astral characters.
Setup verifies the entire resulting buffer and version. The timed kernel includes
all line-index rebuilds, overlay mutation, source synchronization, and change
classification. It excludes host construction, initial open, JSON encoding,
native session scheduling, and diagnostics.

Position fixtures have 128 CRLF lines with 8 to 64 repeated chunks per line.
ASCII uses `Cafes item `, with 11 bytes and 11 UTF-16 units per chunk. Unicode uses
`Café😀é `, with 13 bytes and 9 UTF-16 units per chunk. Incoming queries address
the end of the first chunk, the middle, and the end of each line. Expected byte
offsets use those literal widths, not the conversion under measurement.
Outgoing diagnostics start after the repeated prefix. Setup checks literal
diagnostic codes and both range endpoints. These include a final unknown Filter
as well as 128 unknown Tags.

## Native stdio transport

`tools/bench_lsp.py` is a standard-library Python runner against a supplied native
binary. It does not use private server APIs or change any Project or Template
files. Its results are wall time, not CodSpeed instruction counts.

- Cold startup starts before process creation and ends at version-1 push
  diagnostics after `initialize`, `initialized`, and `didOpen`. Each sample uses
  a fresh process. Filesystem caches remain warm between samples.
- Edit latency starts before JSON serialization and framed notification writing,
  and ends when matching-version push diagnostics arrive. Two preliminary
  toggles warm both edit directions. The runner does not wait for all optional
  background cache warming before reporting first diagnostics.
- Default input is a 512-line Unicode/CRLF Template with an unknown-Tag sentinel.
  Forward and reverse notifications have 1, 8, or 32 sequential changes. Later
  ranges depend on the inserted line and on UTF-16 widths. For multi-change
  packets the final change renames the sentinel. Every publication must have
  the expected code, message, line, and UTF-16 column.
- Explicit-template mode edits the markup-only line nearest one-third through
  the file. It appends/removes `x` at the end of that line, without changing line
  breaks. Repeated insert output must match the first insert output. Delete
  output must match the initial output. Different binaries need not emit equal
  diagnostics; their results retain diagnostics for comparison.

The runner accepts `--binary`, `--samples`, `--edits`, `--changes`, and `--timeout`.
Explicit-template mode requires both `--project-root` and `--template` and accepts
`--initialization-options` as JSON, including `venv_path`,
`django_settings_module`, and `pythonpath`.

Output schema version 1 contains `median_cold_start_ns`, `median_did_change_ns`,
and raw `samples`. Each sample has `cold_start_ns`, `did_change_ns`,
`initial_diagnostics`, `inserted_diagnostics`, and `server_info`. All durations
are nanoseconds. Top-level `source_sha256` and `changes_sha256` identify the
source and forward/reverse packets. `changes_per_notification` and
`position_encoding` identify the other workload settings.

These timing boundaries differ from `real_project::cold_start_check_templates`
and `real_project::keystroke_diagnostics`. Results must remain separate series.
The runner is compatible with published v6.1.1 as well as current main.

## Reproduction

```sh
cargo bench -p djls-bench --bench semantic -- 'validate_incremental_'
cargo bench -p djls-bench --bench semantic -- validate_new_template_warm_project
cargo bench -p djls-bench --bench lsp
cargo build --release -p djls
python tools/bench_lsp.py --binary target/release/djls --changes 8
```

The native runner has no corpus dependency in default mode. Real-project runs
require the same source checkouts and Python environments as the existing
`real_project` benchmarks.

Native Divan baselines use `--sample-count 30 --sample-size 1 --min-time 0`.
When invoking a built benchmark directly, `--bench` selects measurement rather
than test mode.

For the edit controls, local Callgrind baselines use the same optimized executable
with 2 and 4 samples of size 1. The selected part must have a `desc: Trigger`
ending in the exact benchmark name and fixture argument. The per-edit count is
the difference between that part's `totals:` values, divided by 16. This cancels
fixed measurement overhead and accounts for eight edits per sample. The
metadata part and `summary:` are not benchmark totals.

Native wall-time hooks bracket the sampling loop, including input generation.
Consequently, this instruction-count subtraction does not isolate kernels that
use `with_inputs`, such as mutation and new-template validation. Their kernel
instruction counts come from the existing CodSpeed simulation shard, whose
compatibility runner brackets the operation separately from input generation.
