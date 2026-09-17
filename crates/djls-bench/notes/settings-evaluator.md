# Python settings evaluator measurements

Recorded 2026-09-09. Baseline: `1fd95c3d` (the corpus demo script, before evaluator changes). Both CLI binaries used the normal Cargo release profile on an AMD Ryzen 7 PRO 5850U, Linux, 16 logical CPUs. The machine was shared with other work; timings varied substantially.

## CLI workload

`uv run tools/demo_check.py --binary <binary> healthchecks netbox pretix` ran each repository in a fresh process, sequentially, with its manifest settings module and project root. Each check received the repository directory as an explicit input. Timings include discovery, template analysis, rendering, and log I/O. Builds and downloads were outside the timer. Filesystem caches were not cleared.

The dependency-free `uv` script environment supplied no installed Django packages. This workload does not measure LSP readiness or a fully installed Pretix environment.

Corpus revisions:

| Repository | Commit |
| --- | --- |
| Healthchecks | `b9eac590c6cbc7ba1417175bebe4f15c51b54f4f` |
| NetBox | `7300104cea5d658324b9aba4873e9b867de0b9c1` |
| pretix | `fbd8bbbeaaa2564c3e29bd4447ae5a8b17fe0cf3` |

Three interleaved baseline/changed runs, seconds:

| Repository | Baseline runs | Changed runs | Baseline median | Changed median |
| --- | --- | --- | --- | --- |
| Healthchecks | 0.378, 0.382, 0.822 | 0.272, 0.532, 0.294 | 0.382 | 0.294 |
| NetBox | 3.271, 3.162, 6.935 | 0.966, 2.261, 1.059 | 3.271 | 1.059 |
| pretix | 8.156, 13.221, 9.145 | 1.839, 4.622, 2.088 | 9.145 | 2.088 |

Earlier baseline Pretix runs took 7.8–8.4s. Changing only identity lookup equality produced 1.8–2.5s. The additional borrowing and full-equality changes had smaller effects that these noisy CLI runs do not isolate reliably.

Diagnostics matched byte-for-byte in the compared logs. Healthchecks and NetBox reported none. Pretix reported two findings in Sphinx/Jinja documentation templates included by the repository-wide scan; these are not Django template errors. No files or diagnostic codes were excluded to improve the timings.

## Changes measured

`BranchJoin::identity_cmp` orders branches by kind, module identity, origin, and discriminator. The old duplicate lookup in `ConstraintNode::collect_joins` used that full ordering even though it needed only equality. Most candidates have different origins, but the comparison walked module paths before reaching the origin.

`same_identity` now compares cheap fields first and module identity last. Full join equality uses that check plus `arm_count`. Canonical structural ordering is unchanged. This matters because ordering determines which predicates and exact alternatives survive the existing precision limits.

Join collection now borrows identities. Domain validation clones none; predicate widening clones only the excess predicates it must forget before consuming the original tree. The full domain checks still run, including checks for inconsistent domains under disjoint branches.

## Regression benchmarks

The `extraction` target now includes two additional workloads. Existing tag benchmarks remain unchanged.

- `settings_cold_branches`: 8, 32, or 64 independent conditional assignments. Source generation and in-memory database construction are setup. Parsing, evaluation, and settings projection use a fresh database for every input.
- `settings_cold_corpus`: Healthchecks, NetBox, and pretix settings, including first-party imports. Metadata loading, search-path construction, and entry-module resolution are setup. Source reads, parsing, evaluation, and projection are measured. An explicit nonexistent virtual environment prevents caller-installed packages from changing this workload.

Command: `cargo bench -p djls-bench --bench extraction -- settings_ --sample-count 5`.

The synthetic 64-branch median was 11.19ms before the changes (10 samples), 9.396ms with equality lookup alone (10 samples), and 8.761ms with all changes (5 samples). Different sample counts and machine contention limit conclusions about the smaller changes.

The new corpus settings-only medians after the changes were 149.8ms, 799.8ms, and 1.365s respectively (5 samples). These exclude template checking and are not interchangeable with the CLI measurements.

## Profiles and remaining costs

Baseline Pretix profiling attributed about 97% of sampled CPU to settings discovery and 72–74% to path comparison. Much of it passed through guarded branch merging and repeated domain checks.

A second profile, after equality lookup and borrowed collection but before the full join equality change, moved the costs to:

| Call path | Inclusive sampled CPU |
| --- | ---: |
| `PythonModuleEffects::join_guarded_branches` | 39% |
| `BranchConstraints::intersection` | 29% |
| `PythonBinding::normalize` | 28% |
| `ConstraintNode::clone` | 17% |
| `BranchJoin::identity_cmp` | 16% |
| `ConstraintNode::collect_joins` | 6% |

These percentages overlap. They identify calls worth inspecting, not independent savings that can be added together.

The profiling binary used `CARGO_PROFILE_RELEASE_DEBUG=line-tables-only RUSTFLAGS='-C force-frame-pointers=yes' cargo build --release -p djls --target-dir target/pretix-profile`. Sampling used `perf record --call-graph fp`. CLI timings above used the ordinary release binary instead.

An additional run inherited the development checkout's `VIRTUAL_ENV`. It exceeded a 30-second limit even after these changes. A 12-second sampled run of the equality/borrowing build spent more than 99% of CPU in settings evaluation; dictionary binding combination, normalization, and module-effect branch merging were prominent. It did not complete, so that profile describes only the sampled prefix. The dependency or import responsible for the difference has not been isolated. No project Python code was executed.

Candidates for the next experiments, not implemented here:

1. Minimize the installed-dependency case and give it a fixed fixture. The dependency-free benchmarks do not cover this much slower behavior.
2. Share immutable constraint subtrees and module identities to reduce deep clones. Any compact identity must preserve module executions under overlapping search roots and must not make canonical ordering depend on insertion order.
3. Avoid reconstructing unchanged module effects at every guarded join. A shortcut must account for the guards, unbound alternatives, and origin evidence; equal-looking values alone do not prove that the inputs cover the same feasible cases.
4. Answer constraint compatibility without constructing a complete intersection. A direct overlap query would still need the domain checks and an equivalence test against the existing intersection result.
5. Revisit linear join deduplication if it becomes significant again. Hashing full paths can cost more than the current cheap rejection on small trees. Predicate widening also collects control joins it later discards.

## Follow-up: pinned Django dependency

Recorded 2026-09-15. The ignored `settings_cold_pretix_with_django` workload keeps the pinned Pretix and latest lockfile-pinned Django corpus checkouts unchanged. The selected Django checkout was 6.1rc1 at `0f1b39b28b20a1094c4c02dd72d0ba840ed7e10b`. It exposes the Django checkout as an explicit site-packages search root, verifies that `django` resolves from that checkout, and uses an explicit nonexistent virtual environment so the caller's environment cannot change the workload. Completed but unpinned directories on disk are ineligible.

The ordinary dependency-free corpus benchmark remains unchanged. On the same orb, its Pretix median was 1.695s over three samples. Before the evaluator changes below, the new Django-backed workload exceeded a 45-second limit before completing its single sample, reproducing the installed-Django behavior without relying on locally installed packages.

Statement-level timing localized the cost to the root Pretix settings module rather than recursively evaluated Pretix imports. Two evaluator costs were investigated there:

- Cartesian expression alternatives are joined one at a time, repeatedly normalizing the complete accumulated binding. Attempts to batch these joins changed bounded predicate evidence even below the 64-alternative limit. Conservative predicate fallback and exact incremental normalization did not improve this workload, so no expression-join optimization was retained.
- Module-effect branch joins reselected every loaded-child coordinate even when every branch retained the same binding. Keeping an unchanged coordinate avoids adding a redundant control coordinate and repeatedly rebuilding its constraints. Guarded joins intersect the binding with each branch guard before joining those feasible contributions, preserving the predicate-widening order.

Before overflow-equivalence review, the unchanged pinned-corpus workload completed in 20.47s without diagnostic instrumentation (one sample, one iteration). That batching implementation could change which provenance-sensitive container alternatives survived overflow and still materialized the Cartesian product, so it was not retained.

An intermediate implementation completed in 48.04s, followed by 49.26s, 48.65s, and 49.81s in a controlled comparison. Further equivalence review found two precision bugs: batched joins skipped intermediate predicate-evidence widening, and merging branch coverage before intersecting an unchanged child binding could retain an infeasible coordinate. Those timings do not represent a valid implementation.

A controlled follow-up on `0e1a7679` compared fixed binaries with the same benchmark harness, lockfile, corpus root, toolchain, and benchmark profile. The unchanged evaluator reached an explicit five-minute limit without completing and showed severe page-cache churn on the 4 GiB measurement orb. The final guarded module-effect shortcut completed in 90.78s (one sample, one iteration), establishing a conservative improvement of more than 3.3× over the censored baseline; the exact ratio remains unknown.

After fixing the predicate regressions, conservative join batching completed in 90.36s and exact incremental pairwise normalization completed in 96.54s. Neither improved meaningfully on the module-effect shortcut alone, so both expression-join experiments were removed rather than retaining unearned complexity.

The slow workload is ignored so it does not add minutes to normal Divan or CodSpeed runs. Run it explicitly with `cargo bench -p djls-bench --bench extraction -- --ignored settings_cold_pretix_with_django`. It uses one iteration and one sample to preserve the full operation honestly while it remains this slow.

### Follow-up validation

- Controlled unchanged evaluator on `0e1a7679`: over five minutes.
- Final guarded module-effect shortcut on the same revision: 90.78s.
- Partial, disjoint, and predicate-budget guard regressions check that unchanged module-child coordinates remain restricted to feasible branch coverage.
- A corpus regression checks that a completed but unpinned newer Django directory cannot change package selection.

## Validation for the original 2026-09-09 changes

- `cargo test -q`: full workspace suite passed.
- `cargo test --release -p djls-project --test settings_extraction --test corpus_settings`: 315 settings tests and 3 corpus tests passed before the final equality refinement; the full workspace run covered that refinement afterward.
- All existing settings snapshots remained unchanged.
- A new test checks identity equality against canonical comparison and checks full equality against structural comparison across different origins, kinds, discriminators, domains, and module search roots.
- `just fmt` and all-target/all-feature Clippy passed.

## Demand-driven baseline (2026-09-16)

Revision `0e1a7679186d8bf3a769ae03e59036836e52c1ad` was measured on an Intel Xeon
Processor @ 2.60GHz orb with 2 logical CPUs, Linux 6.1.158, Rust/Cargo 1.97.1,
Python 3.11.6, and uv 0.12.13. `VIRTUAL_ENV` was unset. The corpus revisions were
the same Healthchecks, NetBox, and pretix revisions listed above. Filesystem caches
were not cleared, and the machine was not isolated from other orb work.

The reproducible dependency-free command was:

```console
cargo bench -p djls-bench --bench extraction -- settings_ --sample-count 10
```

| Benchmark | Median | Samples | Total iterations |
| --- | ---: | ---: | ---: |
| `settings_cold_branches::8` | 349.2µs | 10 | 10 |
| `settings_cold_branches::32` | 2.906ms | 10 | 10 |
| `settings_cold_branches::64` | 10.01ms | 10 | 10 |
| `settings_cold_try_prefixes::2` | 82.57µs | 10 | 10 |
| `settings_cold_try_prefixes::9` | 220.0µs | 10 | 10 |
| `settings_cold_try_prefixes::64` | 3.636ms | 10 | 10 |
| `settings_cold_corpus::healthchecks` | 136.1ms | 10 | 10 |
| `settings_cold_corpus::netbox` | 677.2ms | 10 | 10 |
| `settings_cold_corpus::pretix` | 1.493s | 10 | 10 |

These are current same-machine comparison anchors, not estimates of future savings.
They are not directly comparable to the older 16-CPU machine's results or to the
overlapping inclusive profile percentages above.

Two fixed controls were added without changing existing benchmarks. The first keeps
every branch relevant by appending a branch-specific value to `INSTALLED_APPS`; the
second resolves an empty external `django.contrib.messages.constants` module and uses
four unknown attributes only as keys in irrelevant `MESSAGE_TAGS`. Their command and
baseline were:

```console
cargo bench -p djls-bench --bench extraction -- \
  settings_cold_required_branches settings_cold_external_constants --sample-count 10
```

| Benchmark | Median | Samples | Total iterations |
| --- | ---: | ---: | ---: |
| `settings_cold_required_branches::2` | 468.7µs | 10 | 10 |
| `settings_cold_required_branches::4` | 5.184ms | 10 | 10 |
| `settings_cold_required_branches::8` | 263.6ms | 10 | 10 |
| `settings_cold_external_constants` | 6.980ms | 10 | 10 |

The historical installed-environment slowdown was reproduced. A temporary benchmark
probe pointed pretix at this checkout's `.venv` (36 distributions, including Django)
and timed out during one `django_settings` evaluation:

```console
/usr/bin/time -f 'elapsed=%e exit=%x' timeout 45s \
  target/release/deps/extraction-00fbd43eb8ce32f7 \
  --test --exact extraction::settings_cold_pretix_installed
# elapsed=45.02 exit=124
```

The probe was then reduced to synthetic site-packages layouts. Merely resolving the
`django` package completed in 3.17s, and resolving only `django.contrib.messages`
completed in 4.11s. Adding an empty
`django/contrib/messages/constants.py` was sufficient to exceed 20 seconds. Other
single direct Django imports from pretix settings (`conf.locale`, `utils.translation`,
`core.exceptions`, or `utils.crypto`) each completed in 4.52–4.89s. Each reduction
was a single `--test` execution with a 10- or 20-second timeout, so these are bounded
diagnostic timings rather than statistical benchmark results.

External-package bodies are not evaluated: the minimized trigger's constants module
was empty. The difference is instead caused by successful resolution of the external
module changing later first-party evaluation: pretix uses `messages.INFO`, `ERROR`,
`WARNING`, and `SUCCESS` as dictionary keys in `MESSAGE_TAGS`. The fixed external
constants benchmark retains that shape without requiring an installed dependency.
Because the conservative slice treats attribute access as an uncertain-effect barrier,
this control is expected to retain its cost; an unchanged result is useful evidence,
not a failed optimization. All temporary probes and instrumentation were removed.

For the first same-machine after comparison, rerun both commands above at the
implementation revision. Compare the irrelevant branch, try-prefix, external-constant,
and corpus rows for avoided work, and require the `settings_cold_required_branches`
control to continue evaluating demanded `INSTALLED_APPS` alternatives.

### Conservative slice comparison

The same commands were rerun on `2148e6d` (the slice implementation applied after
the baseline fixture commit), on the same orb and without clearing filesystem caches:

| Benchmark | Baseline median | Slice median | Change |
| --- | ---: | ---: | ---: |
| `settings_cold_branches::8` | 349.2µs | 70.80µs | -79.7% |
| `settings_cold_branches::32` | 2.906ms | 116.1µs | -96.0% |
| `settings_cold_branches::64` | 10.01ms | 185.6µs | -98.1% |
| `settings_cold_try_prefixes::2` | 82.57µs | 75.74µs | -8.3% |
| `settings_cold_try_prefixes::9` | 220.0µs | 208.7µs | -5.1% |
| `settings_cold_try_prefixes::64` | 3.636ms | 3.524ms | -3.1% |
| `settings_cold_corpus::healthchecks` | 136.1ms | 138.7ms | +1.9% |
| `settings_cold_corpus::netbox` | 677.2ms | 680.8ms | +0.5% |
| `settings_cold_corpus::pretix` | 1.493s | 1.447s | -3.1% |

All rows used 10 samples with one iteration per sample, for 10 measured iterations
total. Divan's `iters` column is the total across samples, not the sample size.
The independent irrelevant conditionals show the intended scaling change. Try bodies and the three real projects are
effectively null results at this sample count, consistent with a conservative slice
that skips only pure top-level regions after the last uncertain-effect barrier.

The controls were compared using the dedicated command, again with 10 samples and
10 measured iterations total:

| Benchmark | Baseline median | Slice median | Change |
| --- | ---: | ---: | ---: |
| `settings_cold_required_branches::2` | 468.7µs | 441.5µs | -5.8% |
| `settings_cold_required_branches::4` | 5.184ms | 5.118ms | -1.3% |
| `settings_cold_required_branches::8` | 263.6ms | 251.3ms | -4.7% |
| `settings_cold_external_constants` | 6.980ms | 7.253ms | +3.9% |

The demanded `INSTALLED_APPS` branch scaling remains, and the external-attribute
barrier retains its cost. These small changes come from one noisy before/after run and
should not be interpreted as improvements or regressions. The decisive comparison is
the large reduction for irrelevant independent branches without a corresponding
collapse in the required-branch control.

## Wave 2 avoidable-work baseline (2026-09-16)

Current `origin/main` at `aa0141898177890782ef806bc7b1b09eb08c3177` was compared
with the rebased wave 1 slice at `0e2f431d0c4c26e6fcb0011c545114df4a94bed7` on the
same orb and corpus. Both used the existing release benchmark profile, unchanged
fixtures, warm filesystem caches, and an unset `VIRTUAL_ENV`.

```console
cargo bench -p djls-bench --bench extraction -- \
  settings_cold_branches settings_cold_try_prefixes settings_cold_corpus \
  --sample-count 10
```

| Benchmark | `origin/main` median | Wave 1 median | Change |
| --- | ---: | ---: | ---: |
| `settings_cold_branches::8` | 326.0µs | 63.34µs | -80.6% |
| `settings_cold_branches::32` | 2.904ms | 102.3µs | -96.5% |
| `settings_cold_branches::64` | 9.280ms | 172.6µs | -98.1% |
| `settings_cold_try_prefixes::2` | 97.10µs | 79.28µs | -18.4% |
| `settings_cold_try_prefixes::9` | 223.5µs | 198.8µs | -11.1% |
| `settings_cold_try_prefixes::64` | 3.887ms | 3.350ms | -13.8% |
| `settings_cold_corpus::healthchecks` | 144.7ms | 130.2ms | -10.0% |
| `settings_cold_corpus::netbox` | 266.2ms | 246.8ms | -7.3% |
| `settings_cold_corpus::pretix` | 637.9ms | 570.3ms | -10.6% |

Each row used 10 samples with one iteration per sample: 10 total iterations. The
synthetic branch result remains decisive. The smaller try and corpus differences are
single paired runs and do not establish that wave 1 avoids their expensive regions.

The pinned installed workload used its unchanged ignored harness:

```console
/usr/bin/time -f 'wall=%e exit=%x max_rss_kib=%M' timeout 180s \
  cargo bench -p djls-bench --bench extraction -- \
  --ignored settings_cold_pretix_with_django
```

It completed in 1.503 minutes on `origin/main` and 1.495 minutes on wave 1, each one
sample and one total iteration. Enclosing wall time was 93.20s versus 90.57s and peak
RSS was 2,739,948KiB versus 2,743,712KiB. This is a null result: wave 1 did not avoid
the retained installed-Django work.

Temporary statement instrumentation explained that result. The wave 1 slice selected
184 of 185 top-level statements in `pretix/settings.py`. A bounded statement run
completed 153 root statements through line 568, totaling 6.299s of instrumented
statement time, before reaching the expensive tail. The targeted assignment split at
lines 572–577 found:

- `walk_assign` → `record_unsupported_call_effects(MESSAGE_TAGS)`: 2µs;
- `walk_assign` → `evaluate_binding(MESSAGE_TAGS)`: did not complete within the
  overall 45-second run limit.

The dictionary uses four attributes from the external
`django.contrib.messages.constants` module as keys. Resolution and the existing RHS
effect traversal are not the retained cost; constructing and binding the irrelevant
dictionary value is. The diagnostic runs were intentionally censored, are not
benchmark samples, and were removed after recording these counts.

Wave 1 control baselines for the next comparison were 460.7µs, 5.090ms, and 241.9ms
for `settings_cold_required_branches::{2,4,8}`, and 6.925ms for
`settings_cold_external_constants` (10 samples and 10 total iterations each). Wave 2
must preserve demanded branch work and external effects while avoiding unnecessary
value materialization; deferring that materialization to a later settings request does
not count as avoided work.

### Effect-preserving materialization avoidance

Transferred wave 2 production commit `bdb5c474a74eb340a2a15c6c09f5451ac8d97a34`
(locally cherry-picked as `fdfa83fe37bcd00d70ac4d4e9b2260f1c69c2569`) was measured
with the same commands, benchmark inputs, corpus, release profile, and orb. Each
normal row again used 10 samples with one iteration per sample (10 total iterations).

| Benchmark | Wave 1 median | Wave 2 median | Change |
| --- | ---: | ---: | ---: |
| `settings_cold_branches::8` | 63.34µs | 79.18µs | +25.0% |
| `settings_cold_branches::32` | 102.3µs | 135.7µs | +32.6% |
| `settings_cold_branches::64` | 172.6µs | 216.1µs | +25.2% |
| `settings_cold_try_prefixes::2` | 79.28µs | 93.42µs | +17.8% |
| `settings_cold_try_prefixes::9` | 198.8µs | 241.2µs | +21.3% |
| `settings_cold_try_prefixes::64` | 3.350ms | 3.865ms | +15.4% |
| `settings_cold_corpus::healthchecks` | 130.2ms | 145.4ms | +11.7% |
| `settings_cold_corpus::netbox` | 246.8ms | 273.1ms | +10.7% |
| `settings_cold_corpus::pretix` | 570.3ms | 609.7ms | +6.9% |

The extra dependency and aggregate-certification analysis is visible when there is
little expensive materialization to remove. Relative to current `origin/main`, the
wave 2 corpus medians are +0.5%, +2.6%, and -4.4%; one paired run cannot distinguish
those differences from ordinary variation.

The fixed external-constants control fell from 6.925ms to 156.4µs (-97.7%). A repeat
was 145.1µs, confirming that effect traversal remains while its irrelevant dictionary
is no longer constructed. Required branch medians changed from 460.7µs, 5.090ms, and
241.9ms to 471.5µs, 5.199ms, and 294.9ms. A repeat produced 466.3µs, 5.281ms, and
312.3ms. The apparent 8-branch regression required a controlled follow-up rather than
being dismissed as noise.

Uninstrumented executables were built separately at exact wave 1 and wave 2 revisions
and run in three idle A/B pairs:

```console
target/amp-transfer/extraction-wave{1,2} \
  'settings_cold_required_branches::8' --sample-count 10 --bench
```

| Pair | Wave 1 median | Wave 2 median |
| --- | ---: | ---: |
| 1 | 235.9ms | 236.2ms |
| 2 | 238.0ms | 239.5ms |
| 3 | 239.8ms | 267.0ms |

Each cell is 10 samples and 10 total iterations, for 30 measured iterations per
revision. The first two pairs differ by 0.1% and 0.6%; the third pair had a wider wave
2 range (237.7–314.7ms) and raised its median by 11.3%. The median of batch medians is
238.0ms versus 239.5ms (+0.6%). The isolated comparison therefore does not reproduce
the earlier 22–29% result as a stable regression.

Work-count diagnostics agree: the exact fixture has one demanded initial assignment
followed by eight `if`/`else` regions of augmented assignments. All nine top-level
statements remain `Full`, no unobserved-assignment certificate runs, full and sliced
evaluation both visit 42 expressions, and the demanded result remains the same 65
precision-bounded normalized alternatives. Wave 2 reaches ordinary `finish_assign`
only for the initial `['core']`; all 16 branch bodies retain the existing augmented
assignment path. No production hot-path change is justified by this control.

The unchanged ignored installed workload completed in 8.196s, compared with 1.495
minutes (about 89.7s) on wave 1: about 10.9× faster. Both measurements were one sample
and one total iteration. Enclosing wall time fell from 90.57s to 9.03s, and peak RSS
fell from 2,743,712KiB to 411,968KiB. The run completed well inside the same 180-second
bound; neither result is censored.

Every cold benchmark iteration constructs a fresh database and measures the first
settings request. The implementation tests independently count one full dictionary
materialization for a demanded value, zero for the undemanded value on its first
request, and no additional materialization on cached facts/import-trace requests.
Therefore the 81.5s first-request reduction is work avoided by the settings stage, not
work deferred until a later request. Cached requests remain Salsa memo hits rather than
a separate timed workload.

### No-candidate observer guard

Follow-up commit `7e7a67c549159707221a07ba3051219264024fa8` avoids scanning the
whole original module for observers when the backward slice contains no possible
unobserved simple assignment outside the needed set. The unchanged focused command was:

```console
cargo bench -p djls-bench --bench extraction -- \
  settings_cold_branches settings_cold_required_branches \
  settings_cold_try_prefixes settings_cold_external_constants --sample-count 10
```

| Benchmark | Wave 2 median | Guard median | Change |
| --- | ---: | ---: | ---: |
| `settings_cold_branches::8` | 79.18µs | 76.42µs | -3.5% |
| `settings_cold_branches::32` | 135.7µs | 123.8µs | -8.8% |
| `settings_cold_branches::64` | 216.1µs | 172.7µs | -20.1% |
| `settings_cold_try_prefixes::2` | 93.42µs | 95.93µs | +2.7% |
| `settings_cold_try_prefixes::9` | 241.2µs | 228.7µs | -5.2% |
| `settings_cold_try_prefixes::64` | 3.865ms | 3.819ms | -1.2% |
| `settings_cold_required_branches::2` | 471.5µs | 455.5µs | -3.4% |
| `settings_cold_required_branches::4` | 5.199ms | 5.234ms | +0.7% |
| `settings_cold_required_branches::8` | 294.9ms | 271.6ms | -7.9% |
| `settings_cold_external_constants` | 156.4µs | 128.3µs | -18.0% |

Each row is one run of 10 samples and 10 total iterations. These are timings, not
claims that each fixture executes less semantic work. The irrelevant-branch fixture
has no retained candidate and can skip observer traversal. The external-constants
fixture still has an unobserved aggregate candidate and retains observation and effect
work; its timing change is not evidence of another omitted operation. Required-branch
work counts remain the identical `Full` evaluation documented above.

The unchanged pinned workload completed in 7.335s (one sample, one total iteration),
down from 8.196s before the guard and about 89.7s on wave 1. Peak RSS was 412,240KiB,
effectively unchanged from wave 2's 411,968KiB. The guard therefore preserves the
first-request materialization win and brings the total speedup over wave 1 to about
12.2×. No result was censored.

## Demanded correlated values: research and measurement (2026-09-16)

The next pass initially opened a clean checkout at
`aa0141898177890782ef806bc7b1b09eb08c3177`, before the previous pass. The fixed
baseline is instead `7bb5b27584a6fa9626a2e338a2d0b3c7ee1db33a`, recovered with
its nine-commit history in a verified bundle whose sole prerequisite is the initial
revision. Bundle SHA-256:
`2b75e4898566611485640b49c9b94ad95ac7966bd375a572e256445893bbe10c`.
Separate orbs owned measurement, representation research, and demand/precision
research; production changes and integration remained in the parent checkout.

### What the prior art does and does not supply

- [MultiSE](https://people.eecs.berkeley.edu/~ksen/papers/multise_tr.pdf),
  §§2.2, 3.2 and 5.1: guarded values, equal-value guard coalescing, and reduced
  decision-diagram guards are already present in `PythonBinding` and
  `BranchConstraints`. Figure 3 and §5.2 still combine operand alternatives;
  these representations do not eliminate exponential worst cases. Its §3.3
  path dropping/concretization is incompatible with conservative settings analysis.
  The missing representation optimization here is sharing immutable subtrees,
  not introducing value summaries or decision diagrams for the first time.
- The [MultiSE implementation's BDD module](https://github.com/SRA-SiliconValley/jalangi/blob/symfront/src/js/analyses/puresymbolic/BDD.js)
  separates node construction and memoized apply within an operation's graph.
  [CUDD's apply implementation](https://github.com/ivmai/cudd/blob/master/cudd/cuddBddIte.c)
  likewise distinguishes its computed-operation cache from unique-node construction;
  its manager, reference counts and garbage collection own those lifetimes. A global
  pointer-keyed cache without equivalent ownership would not be a safe translation.
  Branch-only `Arc` sharing is a smaller experiment: it shares interior residuals,
  keeps terminals allocation-free, and needs no manager across Salsa snapshots.
  It does not intern independently constructed equal nodes or memoize operations.
  Equality and ordering must remain structural, not depend on allocation addresses.
- [Rosette sequence compression](https://github.com/emina/rosette/blob/master/rosette/base/adt/seq.rkt)
  factors equal-length sequences by corresponding elements. Its vector/store handling
  treats mutable identity separately. This is useful precedent, not a ready-made
  replacement for DJLS's ordered dictionary write/unpack logs, mutable allocation
  sites, and provenance-sensitive bounded sequence alternatives. A factored
  representation must preserve cross-field correlation, positions and variable-length
  unpacking, dictionary overwrite order, aliases, and every intermediate cap/widening
  boundary. No aggregate representation or normalization schedule is changed here.
- [Horwitz–Reps–Sagiv, Demand Interprocedural Dataflow Analysis](https://doi.org/10.1145/222132.222146),
  §§2–3: demand propagates through cached realizable-path summaries over a finite,
  distributive fact framework. DJLS's bounded alias/evidence domain has not been shown
  to satisfy those assumptions. The paper's graph-cache amortization is not source-edit
  invalidation, and §4.2 reports demand sequences that can cost more than exhaustive
  analysis. Export demand needs module identity, incoming intrinsic contamination,
  demanded correlated bindings, observable module effects, and typed dependency/coverage
  summaries together. An unused export can still mutate an exported alias or contaminate
  an intrinsic used by the importer. Star/open imports, recovery and root-reaching
  cycles need an explicit Full fallback. This is a separate semantic contract, not a
  local representation optimization.
- [Rival–Mauborgne, The Trace Partitioning Abstract Domain](https://www.di.ens.fr/~rival/papers/toplas07.pdf),
  §§3.2–3.3 and 5: partitioning must cover every original trace; merging joins the
  represented possibilities rather than picking likely worlds. §7.2 explicitly
  distinguishes execution frequency from the precision needed to prove a path safe.
  Demand-informed partition allocation is future precision-policy work. The current
  64 exact alternatives plus unknown remainder, four-predicate budget, structural
  retention order, and intermediate forgetting remain fixed in this pass.

Intermodule demand and per-symbol library detail therefore remain separate candidates,
not additions to this pass. Existing library priming is latency deferral when full
detail is immediately consumed; its narrow structural compatibility fallback remains
unchanged. `compatible_with` also looked promising in isolation, but its only production
caller is settings `feasible_cases`, not evaluator aggregate construction. Replacing
that overlap check cannot be assumed to improve a settings-evaluation benchmark.

### Current baseline profile

The measurement orb has two logical Xeon 2.60GHz CPUs, about 3.84GiB RAM, no swap,
and Rust 1.97.1. The benchmark uses the unchanged `bench` profile (`debug = 2`),
lockfile-selected corpus and explicit nonexistent virtual environment. Builds and
downloads completed outside timed runs; benchmarks ran sequentially, with warm
filesystem caches. The initial required pinned command reproduced 9.741s (one sample,
one iteration), enclosing wall 11.06s and peak RSS 411,884KiB. This is this orb's
reproduction, not confirmation of the historical 7.335s sample.

Installing libc debug symbols upgraded Debian libc from 2.36-9+deb12u3 to u14.
The baseline was rerun after that change, and all candidate comparisons below use
u14 throughout. The initial u3 timing is not used as an A/B denominator.

The refreshed baseline profile used `perf record -e cpu-clock:u -F 499
--call-graph dwarf,32768`, yielding 4,536 samples without lost samples. Profile runs
are separate from uninstrumented timing. Inclusive percentages count each sample
once per category even if recursion/inlining contributes several matching frames:

| Baseline call path | Inclusive sampled CPU |
| --- | ---: |
| Guarded module-effect joins | 57.76% |
| Constraint intersection | 28.11% |
| Constraint-node cloning and descendants | 26.28% |
| Module-identity cloning and descendants | 15.87% |
| Binding normalization | 13.16% |
| Join collection | 7.67% |
| Value cloning and descendants | 6.68% |
| Domain validation | 5.20% |
| Sequence construction | 0.42% |
| `combine_bindings` | 0.24% |
| Dictionary construction | 0.20% |

These rows overlap and cannot be summed. Exclusive leaf buckets, which do not
overlap each other, attribute 29.78% to allocation/free, 11.57% to `memcpy`, and
6.06% to `memcmp`. Of 525 `memcpy` leaf samples, 439 descend from constraint-node
cloning; 248 of 1,351 allocator leaf samples do too. The clone function itself is
only 1.85% exclusive. Thus its inclusive cost is not just a misleading large wrapper:
it includes actual recursive copying/allocation. Sampling does not isolate layout or
cache effects. Import-loader frames occurred in only 0.11% of samples and
`compatible_with` in none; these are stack classifications, not an exhaustive causal
accounting of every imported-module dependency.

### Rejected smaller experiment: operand clone placement

Candidate `74d57a97f266942d4f587d10e181523af2961b6a` moved operand copies in
`combine_bindings` after the existing guard intersection, removing the redundant
outer-left copy and copies of infeasible right operands. Pair order, operations,
normalization and widening were unchanged. It passed the evaluator/settings tests,
but only three baseline profile samples (0.066%) were value-clone descendants of
`combine_bindings`, so a large pinned-workload benefit was unlikely.

Three isolated pairs used order AB, BA, AB, where A is the fixed baseline binary and
B is the clone-placement binary. Each ordinary cell is a median of ten samples with
ten total iterations; each pinned cell is one sample/iteration:

| Workload | Pair 1 A / B | Pair 2 A / B | Pair 3 A / B |
| --- | ---: | ---: | ---: |
| Ordinary Pretix | 725.6 / 719.9ms | 655.4 / 665.9ms | 671.5 / 712.4ms |
| Required branches 8 | 313.9 / 311.1ms | 282.4 / 278.9ms | 323.8 / 323.6ms |
| Pinned Pretix+Django | 9.071 / 8.908s | 7.943 / 8.314s | 8.776 / 9.357s |

Median batch medians changed by +6.1%, -0.9%, and +1.5%, respectively, with no
stable benefit. Pinned RSS was A: 411,976 / 412,076 / 411,936KiB versus
B: 411,748 / 412,212 / 412,032KiB. This experiment is not in the retained branch.
It is not evidence that removing copies is generally harmful; it is evidence that
this particular small change did not earn retention in these workloads.

### Retained experiment: immutable constraint branches

The measured implementation is `91f62914ba5148491b8743d810096a3b68f57575`,
directly on the fixed baseline, without the rejected operand-copy change. The retained
implementation commit `93a8f547dcad21c70ce75ce5fcdebdabdeae24d4` adds only a
changelog entry relative to that measured revision; its Rust source is identical.

`ConstraintNode::Branch` now owns an `Arc<ConstraintBranch>`, while terminals and
the root remain inline. Cloning a root or interior residual increments a reference
count instead of recursively cloning descendants and module identities. There is no
global interner, operation cache, mutable shared node, or new query key. Separately
allocated equal trees still compare structurally equal; a shared pointer only shortcuts
an equality already guaranteed by immutability. Structural order still uses the same
join fields and reverse arm comparison, never allocation addresses.

The guard union/intersection operations still perform whole-input domain validation,
the same recursive apply, and the same ordered predicate forgetting. Selection still
forgets/reassigns a coordinate, while a requirement still intersects it. No binding
join, normalization, provenance merge, alternative cap, effect, import, or statement
selection was moved. This preserves the evaluator's bounded operation sequence rather
than relying on associativity or distributivity. Source dependencies and Salsa equality
remain unchanged, including equal results reconstructed in another query execution.

The final pinned comparison used three AB/BA/AB pairs, each one sample and one total
iteration per binary, with no censored runs:

| Pair | Baseline time | Shared time | Baseline RSS | Shared RSS |
| --- | ---: | ---: | ---: | ---: |
| 1 | 8.122s | 3.428s | 412,052KiB | 136,108KiB |
| 2 | 9.031s | 3.662s | 411,900KiB | 136,152KiB |
| 3 | 8.825s | 3.930s | 411,656KiB | 135,996KiB |

The observed medians are 8.825s versus 3.662s (58.5% less time, 2.41× faster),
and 411,900KiB versus 136,108KiB (67.0% less peak RSS). These are three full
first-request iterations per revision on one orb, not cross-machine estimates or
formal confidence intervals. Every timed input uses a fresh database; no cost is moved
to a later settings/detail request. Cached requests are not a separate timed workload.

The post-change profile has 1,737 samples at the same 499Hz rate. Constraint-clone
inclusive samples fell from 1,192/4,536 (26.28%) to 12/1,737 (0.69%). Exclusive
allocator samples fell from 1,351 (29.78%) to 320 (18.42%), and `memcpy` samples
from 525 (11.57%) to 56 (3.22%). Guarded effect joins rose from 57.76% to 80.08%
of the smaller profile, but their sample count fell from 2,620 to 1,391. That rising
percentage is not a regression. Counts here are sampled CPU observations, not
deterministic invocation/allocation counters; the unchanged execution schedule and
the removed recursive clone are independently visible in the implementation.

The full ordinary suite ran in the same three AB/BA/AB pairs. Each cell below lists
the three batch medians in milliseconds; each batch has ten samples and ten total
iterations, giving 30 samples/30 iterations per row/revision. The last column compares
medians of batch medians, **not** pooled observation medians. Benchmark names retain
the `settings_cold_` prefix:

| Benchmark suffix | Baseline batch medians (ms) | Shared batch medians (ms) | Median change |
| --- | --- | --- | ---: |
| `branches::8` | 0.07785, 0.07187, 0.08153 | 0.1029, 0.06417, 0.07228 | -7.2% |
| `branches::32` | 0.2398, 0.1295, 0.1762 | 0.1593, 0.1197, 0.1273 | -27.8% |
| `branches::64` | 0.3157, 0.1772, 0.2026 | 0.2273, 0.1770, 0.3381 | +12.2% |
| `corpus::healthchecks` | 165.5, 152.7, 170.0 | 107.6, 107.0, 118.1 | -35.0% |
| `corpus::netbox` | 289.4, 335.7, 325.4 | 150.4, 146.9, 164.4 | -53.8% |
| `corpus::pretix` | 685.4, 706.0, 716.6 | 297.6, 291.8, 320.5 | -57.8% |
| `external_constants` | 0.1468, 0.1542, 0.1463 | 0.1251, 0.1275, 0.1462 | -13.1% |
| `required_branches::2` | 0.4779, 0.5541, 0.5151 | 0.2541, 0.2560, 0.2559 | -50.3% |
| `required_branches::4` | 5.331, 5.628, 6.045 | 2.299, 2.245, 2.312 | -59.2% |
| `required_branches::8` | 268.9, 311.9, 299.0 | 118.8, 126.9, 130.0 | -57.6% |
| `try_prefixes::2` | 0.09078, 0.09748, 0.1026 | 0.07333, 0.07476, 0.1095 | -23.3% |
| `try_prefixes::9` | 0.2270, 0.2513, 0.2508 | 0.1651, 0.1579, 0.1653 | -34.2% |
| `try_prefixes::64` | 3.768, 3.922, 4.767 | 2.249, 2.245, 2.697 | -42.7% |

The demanded eight-branch control's individual-iteration ranges were 260.5–381.6ms
versus 117.3–155.6ms. Ordinary Pretix ranges were 630.2–842.3ms versus
287.3–466.8ms. These gains are substantially larger than observed run variation.
Tiny irrelevant-branch rows have much wider relative variation, including a 4.606ms
baseline outlier for `branches::8`; their percentages should not be read as precise
savings. The apparent `branches::64` regression was investigated rather than hidden:
five additional alternating ten-sample batches gave median batch medians 196.9µs
versus 190.8µs, while `branches::32` varied in the opposite direction. Three further
100-sample/100-iteration batches per row/revision (300 total each, same AB/BA/AB
order and unchanged fixture) produced these medians in microseconds:

| Branches | Baseline batches (µs) | Shared batches (µs) |
| --- | --- | --- |
| 8 | 112.3, 73.4, 72.47 | 67.84, 67.70, 66.49 |
| 32 | 211.7, 120.6, 120.2 | 120.2, 120.0, 121.1 |
| 64 | 193.7, 188.0, 183.3 | 183.7, 185.3, 191.0 |

Those followups do not establish a small-row regression or precise speedup. They do
not replace the unfavorable original rows above. No confidence intervals are claimed.

### Reproduction details, limits, and verification

Each exact revision was built once with
`cargo bench -p djls-bench --bench extraction --no-run`; its executable was preserved
before switching revisions. Fixed binaries used `--bench settings_ --sample-count 10`
for the ordinary suite and `--bench --ignored settings_cold_pretix_with_django`
under `/usr/bin/time -f 'wall=%e exit=%x max_rss_kib=%M' timeout 180s` for the pinned
case. Pinned enclosing walls were 8.16 / 9.07 / 8.86s versus 3.44 / 3.68 / 3.95s.
Every measured run exited zero. No CPU affinity, frequency control, or filesystem-cache
flush was used. This measures first settings requests, not whole CLI/LSP startup.

SHA-256 identities for the controlled comparison:

| Input or executable | SHA-256 |
| --- | --- |
| `Cargo.lock` | `0d00393441e45c9a927b4b906a9f37af6f37b2b88d4233a36af1afe2ee272453` |
| Corpus `manifest.lock` | `6b5ce6ee4899819101557dbd417f0d7df27855724131f0bb63028e03c2d99b7d` |
| `benches/extraction.rs` | `a31b00dc2b95f7826b55ac269bb2bf67c87a2ce6d1635d56eee6f458b5cd1fbd` |
| Baseline executable | `e7e4d219bad53fe492ddcf1d6c3bd821a3533ab5bc1efc4f39364dab2cd55dcf` |
| Rejected clone executable | `229b90df382b28bd106a68ace113d61c593bf18977e1618ca0c23b724c66e672` |
| Shared-constraint executable | `206de0b2348a70827dad6fe319c6b2f4e4fb05fb290fc57ad267d7b4b8fbb500` |

The measurement orb's raw timing logs, parsed data, environment and exact profile
category rules were transferred and verified in an evidence archive with SHA-256
`202673cfdeec53797feac651ad7226f05defa156ffed4284e79a73c44c3ecea6`.
The parent independently recomputed the final table and checked the local lockfile
and benchmark hashes against that archive.

Sampling covers the whole process, including harness/setup/drop, without timed-region
markers. User-space CPU-clock samples exclude kernel/blocked time; 32KiB DWARF stacks
can truncate, and inlining/tail calls can obscure ancestors. The baseline has 168
samples with unknown ancestor frames but no unknown leaves; the shared profile has
one unknown leaf. Missing symbols are not proof of absent work. In particular,
`load_import_chain` frame coverage is not total transitive imported-module work.

Verification on the retained Rust implementation:

- `cargo test -q`: 2,409 passed, zero failed, seven existing ignored tests.
- Targeted evaluator tests: 175 passed; settings extraction: 315; project settings:
  106; corpus settings: seven. Existing snapshots were unchanged.
- A new private test verifies physical sharing, independent reselection of a clone,
  reuse of an interior residual, and structural equality/order of separately allocated
  equal trees. Existing tests cover multi-arm domains, disjoint domain mismatches,
  module/search-root identity, predicate forgetting, the 64-plus-unknown cap,
  provenance, alias mutation, import cycles, recovery and source invalidation.
- `just e2e`: all 48 existing Python protocol tests passed, including startup and
  diagnostic republishing. No parallel Rust LSP harness was added.
- `just fmt`, `just fmt --check`, `just clippy`, `just lint`, and `just hawk` passed;
  Hawk reported zero findings. No temporary production instrumentation was added.

The next justified investigation is repeated intersection and domain/identity work
inside guarded module-effect joins: intersection still has 788/1,737 inclusive
samples (45.37%), overlapping the effect-join samples. Measure repeated operand pairs
and potential cache hit rates before considering memoized apply or interning, preserving
the current public widening boundaries and self-owned query lifetimes. Factored
aggregates, export-demand summaries and demand-informed precision allocation remain
separate research; this profile does not justify expanding into them or server work.

## Follow-up: repeated operations versus shared module identity

This pass starts at `dd5ecd039fc222b15fd4d8a8e9d46b9d93fab6b9`, after rebasing the
retained constraint-sharing work onto `6aeef2fec4a8e2a18d271559c58dee194a2f4a74`.
The baseline executable is byte-identical to the preceding pass's shared-constraint
executable (`206de0b2…bb500`). All comparisons use the same measurement orb, libc
u14, toolchain, corpus, benchmark profile and fixtures. No new demand policy,
precision budget, normalization schedule or benchmark contract is introduced.

### Prior art and the cost it must actually remove

- [Bryant's Apply algorithm](https://www.cs.cmu.edu/~bryant/pubdir/ieeetc86.pdf),
  §4.3, memoizes pairs within one operation; §6 discusses cross-operation reuse.
  [CUDD's implementation](https://github.com/ivmai/cudd/blob/master/cudd/cuddBddIte.c)
  separates this computed cache from unique-node construction. A DJLS-local pointer
  cache can borrow live input roots, but cannot safely span `forget`'s successive
  union-fold accumulators without retaining operands: freed addresses can be reused.
  Separate exact Apply caches from already-widened public results, and preserve
  validation → Apply → ordered single-coordinate forgetting at every public call.
- [CUDD's node/support traversal](https://github.com/ivmai/cudd/blob/master/cudd/cuddUtil.c)
  visits each physical node once. DJLS also needs complete coordinate/domain checks
  under all arms. Skipping a repeated allocation is safe during a borrowed-root
  traversal; skipping a repeated coordinate can miss different descendants or an
  incompatible arm domain. Persistent support vectors are a larger tradeoff: querying
  every suffix of a chain can retain quadratic metadata, including owned module paths.
- [LLVM's immutable maps](https://github.com/llvm/llvm-project/blob/main/llvm/include/llvm/ADT/ImmutableMap.h)
  and [sets](https://github.com/llvm/llvm-project/blob/main/llvm/include/llvm/ADT/ImmutableSet.h)
  reuse unchanged paths. This supports testing unchanged-node reconstruction avoidance,
  not skipping guarded effects. An unchanged module-effect table still needs each
  branch's individual restriction before joining; `Combine(a, a) == a` is not justified
  merely by equal source tables. Persistent effect maps and query-owned contexts would
  require broader changes than the experiments below.

### Fresh profile and bounded reuse diagnostics

The normal baseline suite completed with ten samples/iterations per row. The initial
pinned reproduction was 4.278s, enclosing cargo wall 5.31s, peak RSS 136,172KiB.
A separate 499Hz, 32KiB-DWARF user-space CPU profile contains 1,849 samples:

| Baseline category | Inclusive samples | Share |
| --- | ---: | ---: |
| Guarded module-effect joins | 1,482 | 80.15% |
| Constraint intersection | 799 | 43.21% |
| Predicate widening | 618 | 33.42% |
| Forgetting | 582 | 31.48% |
| Join collection | 309 | 16.71% |
| Domain validation | 194 | 10.49% |
| Module structural comparison | 416 | 22.50% |
| Module equality | 239 | 12.93% |

These categories overlap. In particular, forgetting occurs inside widening, and
module comparison occurs inside join operations. `identity_cmp` under forgetting
accounts for 395 samples (21.36%), while construction under forgetting accounts for
80 (4.33%). The profile suggests checking identity work rather than inferring a cache
opportunity from intersection's inclusive percentage.

Temporary instrumentation, removed before candidate timing, found:

- Canonical union: 344,543 root Apply invocations, 292,503 branch-pair visits,
  1,668 repeated physical pairs (0.57%). Intersection: 128,347 root invocations,
  1,415,758 branch-pair visits, 33,047 repeats (2.33%). These are observed repeated
  visits, not effective cache hit rates after pruning descendants of a hit.
- Forgetting: 6,476 root invocations, 1,805,463 branch visits, 162,434 physical
  repeats (9.00%). Union folds use separate pointer-lifetime scopes.
- Domain collection: 5,122,320 expanded branch visits and 436,613 known repeats
  (8.52%); 25,476 visits exceeded the diagnostic tracking cap. Predicate collection:
  4,147,410 visits and 267,125 repeats (6.44%), without cap misses. Coordinate identity
  comparisons total 51,465,251 and 44,804,471, respectively.
- Sampled structural duplication is higher than physical reuse. In a window of 128
  recent weakly held sampled nodes, 43,233 of 92,024 construction samples matched a
  structurally equal live node; only 8,674 matches also had identical physical child
  identities. Weak upgrades check liveness. This bounded sample is not a global
  interner hit rate. Cross-call pair screens include trivial cases and likewise do
  not establish a nontrivial computed-cache benefit.

Low physical reuse argues against adding a hash table to every Apply. Independently
allocated equal trees remain an interning research lead, but introducing a manager,
structural fingerprints or pervasive query-context plumbing is not justified by these
bounded observations alone.

### Rejected: traversal-local visited nodes

Candidate `0f7be9d13db4b76d51be4aa87d551137ddd22dac` added one local visited-Arc set to
join collection, shared across both roots during domain validation. Coordinate checks,
first-DFS encounter order, predicate sorting and forgetting were unchanged. A test
distinguished shared residuals from distinct nodes at the same coordinate.

The isolated screen had no build or diagnostic competition. Ordinary Pretix changed
303.7 → 313.8ms (+3.3%), required-8 121.9 → 154.2ms (+26.5%), each ten samples/ten
iterations. Required-8 ranges were disjoint: 119.5–127.1 versus 151.2–182.5ms. Pinned
Pretix changed 3.878 → 4.118s (+6.2%), one sample/iteration each, with RSS
135,824 → 136,068KiB. All runs exited zero. The modest observed reuse did not pay for
the added traversal table. No further timing rounds were used to seek a favorable result.

### Rejected: copy only changed paths during forgetting

Independent candidate `0d6eee5c4bcf3de39b12761c739ae95ed46e6125` delayed arm-vector
allocation until the first changed child, retaining the original node if all children
were unchanged. It kept recursive order and the exact existing union fold. Its initial
screen suggested 4–8% less time, but three full AB/BA/AB pairs did not confirm that:

| Workload | Baseline batch times | Candidate batch times |
| --- | --- | --- |
| Ordinary Pretix (ms) | 322.9, 291.7, 291.8 | 318.7, 311.5, 285.6 |
| Required-8 (ms) | 133.9, 118.2, 119.8 | 131.9, 130.4, 117.1 |
| Pinned Pretix (s) | 3.938, 3.615, 3.646 | 3.939, 4.387, 3.623 |

Ordinary rows have ten samples/iterations per batch; pinned rows have one. Median batch
times rose 6.8%, 8.8% and 8.0%, respectively, with overlapping ranges and a broadly slow
second candidate batch. Peak pinned RSS did consistently fall: baseline 135,920 /
136,000 / 135,992KiB versus candidate 132,320 / 132,280 / 132,380KiB (median -2.7%).
The small memory reduction did not earn retention without a repeatable timing benefit.

A separate profile investigated the new equality check rather than dismissing the
unfavorable measurements. It reduced construction under forgetting from 80/1,849 to
37/2,214 samples, but added 47 samples (2.12%) whose first constraint-operation caller
of structural equality was `forget`. This is observable extra work, not an explanation
of all timing variation. No pointer-only variant was pursued; the larger module-identity
comparison cost supplied a more direct experiment.

### Retained: share complete immutable module identities

Independent candidate `e31a67f1693ce10820e923bb3b2f1d79970121c1` wraps the existing
five-field `PythonSourceModule` identity in an immutable `Arc`. Evaluator forks,
different branch joins, predicate identities and cloned results can retain that one
payload. Module comparison returns equal immediately for the same allocation, then
uses the unchanged name → package → path → file → search-path comparison for distinct
allocations. The enclosing branch comparison still compares origin and discriminator;
domain validation still checks arm counts. Derived equality/hash remain value-based,
and the custom Debug representation is unchanged. No resolver key, interner, computed
cache, constraint operation, widening boundary or effect join changes.

This is immutable sharing at the identity owner, not hash-consing. Independently
resolved equal identities can have different allocations and must still compare equal.
It targets the common module shared by different coordinates rather than requiring
repeated physical pairs of entire constraint nodes. It also reduces copied identity
storage; sampling does not separate all representation/layout and cache effects.

The initial isolated screen improved ordinary Pretix 297.5 → 130.9ms, required-8
121.5 → 54.61ms, and pinned Pretix 3.729 → 0.9134s. The full repeated comparison
confirmed the gain. Three AB/BA/AB pairs used the unchanged ordinary 13-row suite
(ten samples/ten iterations per batch, 30 per row/revision) and pinned workload
(one sample/one iteration per batch, three per revision):

| Pinned pair | Baseline time / wall | Shared time / wall | Baseline RSS | Shared RSS |
| --- | --- | --- | ---: | ---: |
| 1 | 3.950 / 3.97s | 1.077 / 1.09s | 135,776KiB | 61,496KiB |
| 2 | 3.987 / 4.01s | 1.029 / 1.04s | 135,944KiB | 61,284KiB |
| 3 | 4.011 / 4.03s | 0.9665 / 0.98s | 135,896KiB | 61,436KiB |

Median pinned time fell 3.987 → 1.029s (74.2% less time, 3.87× faster); median peak
RSS fell 135,896 → 61,436KiB (54.8%). These are additional gains against the already
shared-constraint baseline, not against the pre-sharing or historical demand baseline.

The ordinary table lists every batch median in milliseconds. Changes compare medians
of the three batch medians, not pooled observations. Names retain `settings_cold_`:

| Benchmark suffix | Baseline batch medians (ms) | Shared batch medians (ms) | Median change |
| --- | --- | --- | ---: |
| `branches::8` | 0.08724, 0.1241, 0.1133 | 0.1114, 0.07004, 0.07557 | -33.3% |
| `branches::32` | 0.1249, 0.1247, 0.2147 | 0.1221, 0.2208, 0.1241 | -0.6% |
| `branches::64` | 0.1773, 0.1903, 0.3098 | 0.1831, 0.3052, 0.1725 | -3.8% |
| `corpus::healthchecks` | 109.9, 116.9, 111.5 | 66.45, 61.82, 65.85 | -40.9% |
| `corpus::netbox` | 161.9, 161.9, 147.5 | 92.60, 90.16, 89.74 | -44.3% |
| `corpus::pretix` | 317.1, 316.0, 322.2 | 141.1, 138.1, 142.8 | -55.5% |
| `external_constants` | 0.1236, 0.1212, 0.1624 | 0.1205, 0.1129, 0.1139 | -7.8% |
| `required_branches::2` | 0.2528, 0.3014, 0.2540 | 0.2014, 0.2702, 0.3064 | +6.4% |
| `required_branches::4` | 2.277, 2.457, 2.685 | 1.447, 1.468, 1.494 | -40.3% |
| `required_branches::8` | 130.8, 140.6, 133.4 | 61.61, 61.40, 58.50 | -54.0% |
| `try_prefixes::2` | 0.07335, 0.1293, 0.06998 | 0.06312, 0.06907, 0.06999 | -5.8% |
| `try_prefixes::9` | 0.1594, 0.2850, 0.1735 | 0.1358, 0.1691, 0.1468 | -15.4% |
| `try_prefixes::64` | 2.272, 3.227, 2.446 | 2.217, 2.197, 2.022 | -10.2% |

Individual-iteration ranges were 289.6–345.6 versus 129.0–161.8ms for ordinary Pretix,
and 120.5–196.1 versus 56.42–71.10ms for required-8. Small rows remain noisy. The
unfavorable required-2 row was investigated with three further AB/BA/AB pairs, each
100 samples/100 iterations: baseline medians 368.4 / 355.9 / 304.7µs versus shared
189.7 / 183.8 / 186.0µs. The apparent regression did not repeat. These followups do
not replace the original row or establish precise small-row savings.

The final profile used the same 499Hz event and DWARF configuration, with 510 samples
versus the baseline's 1,849. `BranchJoin::identity_cmp` fell from 777 samples (42.02%)
to 19 (3.73%); module structural comparison from 416 (22.50%) to two (0.39%); module
equality from 239 (12.93%) to three (0.59%). This supports the intended mechanism,
without treating tiny remaining counts as precise estimates. Guarded-effect samples
fell 1,482 → 319, intersection 799 → 157, join collection 309 → 132, and forgetting
582 → 82. Join collection's rising share of the smaller denominator is not a
regression. Categories overlap; whole-process sampling includes setup/drop, excludes
kernel/blocked time and can lose ancestor information through inlining/truncation.
There are no formal confidence intervals or total-import-work claims.

### Follow-up reproduction and verification

Build/fixed-binary commands and measurement boundaries are unchanged from the preceding
pass. Each iteration still uses a fresh database and computes the first settings
result; filesystem caches may be warm. All measured runs exited zero, with builds and
downloads outside timing and no competing benchmark processes. The final measured
binary SHA-256 is `22f3ccb0218d7d99c89848d53f984361e1677e8af0c6a6fe69d9eb2339ca36f7`;
lockfile, corpus and harness hashes match the previous table.

The evidence archive SHA-256 is
`7c00725fac0363b93bb27bab145ebb60548ae7630fa8d9ff7f6b50150e92db24`.
It preserves all 49 raw measurement/profile logs, all ordinary rows for both accepted
and rejected full comparisons, environment/commands, diagnostic source and bounds,
and profile selection rules/counts. The parent independently parsed the raw timing
rows, checked them against the supplied JSON, and recomputed every retained table
median/sample count. Large perf recordings and preserved binaries remain in the
measurement orb rather than this small archive.

Final source differs from the measured candidate only by making the now module-local
`PythonSourceModule::package` getter private, as required by Hawk, plus changelog and
these notes. It is not claimed byte-identical to the measured source. Neither rejected
constraint experiment nor temporary instrumentation is retained.

Verification of the retained implementation:

- `cargo test -q`: 2,410 passed, zero failed, seven existing ignored tests; rerun after
  the visibility fix. Existing snapshots are unchanged.
- Focused Python unit tests: 214 passed; extraction/resolver/settings/corpus integration
  tests: 691 passed. The identity test checks shared clones and independent equal
  allocations, structural order, hash-table lookup and Debug agreement; field coverage
  includes distinct search roots of the same kind and distinct File identities.
- `just e2e`: all 48 existing Python LSP tests passed on the measured implementation.
  The later getter-visibility reduction changes no runtime behavior.
- `just fmt --check`, `just clippy`, and `just lint` passed after the visibility fix.
  Hawk reported zero findings after applying that one fix; no new public API was added.
- The retained diff leaves `constraints.rs`, operation/cap schedules, effects, import
  policies, query keys and benchmark inputs untouched relative to this pass's baseline.

## Follow-up: bounded constraint hash-consing

This pass starts at `0b316c11bd9632c939860259341b55bc4776e238`, including both
retained immutable-sharing changes. **Neither interner is enabled by default.**
Both reduce pinned Pretix time and memory substantially, but neither removes
ordinary-workload overhead. Production remains at that baseline; only a regression
test for intermediate predicate forgetting and these measurements are retained.
The local `constraint-interning-experiment` branch preserves the complete prototypes.

### Construction reuse, not Apply-cache reuse

The previous pass found little repeated physical-pair Apply work. Hash-consing asks
a different question: do newly constructed branches equal other live branches?
Fresh 499Hz user-CPU sampling on the shared-module baseline found 83/495 samples
under branch construction (16.77%) and 154/495 exclusive allocator samples (31.11%).
These are cost observations, not predicted savings or cache hits.

Temporary diagnostics sampled every 32nd public call of each operation kind. A scope
includes that operation's existing Apply and ordered forgetting, without adopting
input graphs. An initial owned-representative table measured only an upper bound;
the decision used a separate weak-only run, with no owned payloads and liveness
checked before full structural equality. Counts below exclude reduced constructors:

| Workload / operation | Sampled scopes | Nonreduced candidates | Live equal prior construction | Same physical children |
| --- | ---: | ---: | ---: | ---: |
| Pinned / union | 162 | 7,094 | 4,592 (64.7%) | 401 |
| Pinned / intersection | 3,892 | 58,909 | 48,648 (82.6%) | 9,296 |
| Pinned / select | 118 | 21,219 | 15,154 (71.4%) | 0 |
| Ordinary Pretix / union | 57 | 297 | 114 | 38 |
| Ordinary Pretix / intersection | 3,649 | 5,005 | 2,054 | 560 |
| Ordinary Pretix / select | 15 | 169 | 54 | 2 |
| Required-8 / union | 38 | 266 | 0 | 0 |
| Required-8 / intersection | 2,265 | 1,573 | 0 | 0 |
| Required-8 / select | 12 | 144 | 0 | 0 |

All observed owned-table duplicates also had a live equivalent in the weak run.
That does not make these production cache-hit rates: the observer stores different
representatives, performs no interning, and uses a deliberately shallow fingerprint.
Its 5.236 million pinned equality checks, mostly unequal, are not factory cost.
The weak table had an 8,192-record cap with no cap misses. Pinned maximum occupancy
was 634/5,058/1,475 for union/intersection/select; 9,982 expired intersection records
were removed. Occupancy is not a per-operation construction histogram. Weak records
can retain allocation backing after payload death, but do not retain child graphs.
Fourteen `select_arms` calls had no systematic sample; NetBox was not instrumented.
The separate 2,048-slot cross-call weak window is only a sampled lower-bound screen,
not a reason to expand ownership to a query or database.

### The factory preserves structural semantics and operation boundaries

[weak-table-rs](https://github.com/tov/weak-table-rs/blob/master/src/inner.rs)
provides useful precedent for cached hashes, weak keys, liveness checks and equality
verification, but its cleanup policy is not a hard bound. The Rust
[hashconsing crate](https://github.com/AdrienChampion/hashconsing/blob/master/src/lib.rs)
is not a drop-in solution: `HConsign` owns complete `T` keys even though its values
are weak, so keys retain children and collection may require a fixed point. Its
UID-based equality/order must not replace DJLS structural identity. This experiment
is a unique-node construction table, not CUDD's computed Apply cache.

Control `c05b9046ee54345725d070fb28fad2c65bceb50b` adds one cached fingerprint word
per immutable branch, computed from kind, origin, domain, discriminator and ordered
child fingerprints. Module data is intentionally omitted from bucket selection to
avoid rehashing paths; complete equality still checks it. Fingerprints never enter
semantic equality, order or Debug. The control allocates every original node and
isolates metadata cost from interning benefit.

Eager factory `e586e944b9689bb5932e06317cf3eb4b9050cd1a` adds a private per-public-call
`FxHashMap<u64, Weak<ConstraintBranch>>`. It keeps at most 512 weak entries, one
candidate per fingerprint, upgrades before comparing the complete join and ordered
children, and prunes dead entries then clears when full. Collisions replace sharing
metadata, not graph nodes. The table owns no child-bearing keys. Reductions still
precede lookup; standalone literals do not allocate a pointless table. Unregistered
input residuals and independently allocated equal children need no adoption because
lookup uses full structural equality. Pointer inequality remains nonsemantic.

The scope includes validation → canonical Apply → ordered single-coordinate
forgetting without moving any boundary. Select still forgets before requiring its
new arm. Returned nodes own their data and retain no factory, borrowed identity,
global manager or query-context state. Eviction only loses sharing. Four predicates,
64 exact alternatives plus unknown, branch/arm order, provenance and cycle equality
are unchanged.

### Eager interning wins the pinned workload but slows ordinary controls

All comparisons use fixed uninstrumented executables, unchanged Rust 1.97.1, libc
`2.36-9+deb12u14`, corpus/harness locks and explicit missing hermetic virtualenv.
Builds, diagnostics and downloads are outside timing, with no competing benchmark.
The initial cargo-enclosing baseline RSS (103,696KiB) is not compared with direct
executable RSS. Normal rows use ten samples/ten fresh-DB iterations per process;
pinned rows use one/one. Filesystem caches may be warm. These are cold settings
operations, not whole-server or cached-query measurements.

An initial control-only AB/BA/AB comparison found about 4MiB more pinned RSS from
fingerprints alone. Its median batch times were 931.3 → 1,083ms, but large variation
and a slow first baseline run prevent a precise overhead claim. Those runs remain
in the archive rather than being replaced by the following comparison.

The full baseline/control/eager comparison ran ABC/BCA/CAB: three batches, 30
samples/iterations per normal row/revision and three/three pinned. This table shows
medians of batch medians in milliseconds, not pooled medians. Names retain
`settings_cold_`; small rows remain noisy.

| Benchmark suffix | Baseline | Fingerprint only | Eager factory | Eager change |
| --- | ---: | ---: | ---: | ---: |
| `branches::8` | 0.07452 | 0.1017 | 0.08943 | +20.0% |
| `branches::32` | 0.1395 | 0.1224 | 0.1349 | -3.3% |
| `branches::64` | 0.1890 | 0.2275 | 0.1865 | -1.3% |
| `corpus::healthchecks` | 64.84 | 61.97 | 67.18 | +3.6% |
| `corpus::netbox` | 88.31 | 86.15 | 94.06 | +6.5% |
| `corpus::pretix` | 138.8 | 138.1 | 144.0 | +3.7% |
| `external_constants` | 0.1185 | 0.1204 | 0.1405 | +18.6% |
| `required_branches::2` | 0.2031 | 0.2022 | 0.2004 | -1.3% |
| `required_branches::4` | 1.404 | 1.458 | 1.574 | +12.1% |
| `required_branches::8` | 58.91 | 60.70 | 66.32 | +12.6% |
| `try_prefixes::2` | 0.08122 | 0.07398 | 0.08199 | +0.9% |
| `try_prefixes::9` | 0.1516 | 0.1720 | 0.1513 | -0.2% |
| `try_prefixes::64` | 2.104 | 2.236 | 2.311 | +9.8% |

Pinned baseline batches were 989.1 / 1,173 / 1,065ms; fingerprint control 1,035 /
1,068 / 1,111ms; eager factory 711.0 / 723.6 / 704.5ms. Baseline → eager median
improved 33.2%, and median peak RSS fell 61,324 → 19,184KiB (68.7%). However,
required-8 baseline medians 58.91 / 59.90 / 58.72ms were all below eager medians
67.30 / 66.32 / 65.80ms. NetBox likewise had disjoint baseline/eager batch-median
ranges. These are unfavorable results, not benchmark contracts to rename or remove.

A separate eager profile contained 323 samples versus baseline 495. Exclusive
allocator samples fell 154 → 52, while the new factory appeared in 79 samples
(24.46% inclusive). Node equality fell 43 → 19. Constructor coverage rose 83 → 106;
intersection 157 → 159; collection fell 96 → 77; forgetting 81 → 77. These categories
overlap, the denominators differ, and neither count differences nor percentage
changes alone establish exact time savings. Sampling covers whole-process user CPU,
including setup/drop, excludes kernel/blocked time, and can lose ancestors through
inlining or the 32KiB DWARF window. Actual production hits/evictions were not counted.

### Delaying table activation does not remove the tradeoff

Variant `e7fa53a994f649aa7548ed91aca0af25d5888d22` skips lookup/storage for the first
16 nonreduced constructions of each operation, then uses the identical bounded
factory. Fingerprints are still computed. This single amortization heuristic was
motivated by many small operations, not by a measured per-scope histogram or a
proven cutoff for no-hit workloads. No threshold sweep or source-specific policy
was used.

A fresh balanced baseline/eager/delayed ABC/BCA/CAB screen included the three
ordinary controls and pinned workload. Each ordinary cell has ten samples/iterations
per batch, 30 per revision; each pinned cell one/one, three per revision. Values are
all three batch medians in milliseconds; do not compare phase medians as matched pairs.

| Workload | Baseline | Eager | Delayed | Delayed median change |
| --- | --- | --- | --- | ---: |
| NetBox | 90.44, 88.18, 90.07 | 96.84, 95.42, 91.36 | 96.64, 91.98, 93.35 | +3.6% |
| Ordinary Pretix | 133.9, 149.4, 136.5 | 148.3, 150.1, 141.3 | 146.4, 140.2, 140.4 | +2.9% |
| Required-8 | 59.55, 59.35, 58.47 | 65.36, 64.65, 61.39 | 62.40, 83.28, 55.68 | +5.1% |
| Pinned Pretix | 1,051, 1,059, 1,040 | 758.6, 713.3, 753.6 | 737.2, 736.6, 710.1 | -29.9% |

Delayed pinned median RSS was 19,540KiB versus baseline 61,252KiB (-68.1%). Every
delayed NetBox batch median exceeded every baseline median. Ordinary Pretix ranges
overlapped, and required-8 was too noisy to call a persistent delayed regression.
The ordinary overhead was not clearly removed, so this variant stopped after the
screen; it did not receive a further full 13-row timing pass. All runs exited zero.

The default remains the simpler shared-node/shared-module implementation. Hash-consing
has demonstrated a useful large-workload memory/time tradeoff, not a general speedup.
A future memory-prioritized decision can revisit the preserved prototypes explicitly.
This result does not establish that wider interning, Apply memoization, persistent
effect maps or cached coordinate summaries would pay; those need their own evidence.

### Semantic verification and reproducible evidence

The eager experiment passed 2,414 Rust tests (seven existing ignored), 48 LSP E2E
tests, fmt, Clippy and all lint hooks. The delayed experiment passed 2,415 Rust tests,
48 LSP E2E tests and its fmt/Clippy commit hooks. Tests cover forced fingerprint
collisions, complete identity/domain checks, independent equal children, reverse-arm
ordering, bounded eviction, weak expiry, factory drop and self-owned Send/Sync results.
An explicit warmup test checks both sides of the 16-construction boundary.

An independent orb compiled the verbatim baseline alongside each candidate and
replayed a 23-operation require/select/select_arms/union/intersection trace. For the
delayed variant, capacities 0/1/2/512 × warmups 0/1/16 × ordinary/constant fingerprints
gave 552 intermediate structural comparisons, 9,936 independently derived truth-table
checks and 6,072 each equality/order comparisons. The small trace remains below
activation at warmup 16; warmup 1 exercises mixed cached/uncached operations. Separate
six-predicate tests check exact intermediate forgetting boundaries, including a later
contradiction against an already forgotten predicate. No semantic defect was found.

Fixed executable SHA-256 values:

| Variant | SHA-256 |
| --- | --- |
| Baseline | `3e1104a7ee9531af578988abe9b06b7d4ff18f9f3b7197306733f7f7bc8ca884` |
| Fingerprint only | `f818692bcfaa31afa8cb6c55792b92db52c8ab21b921962894a5e77c5e9ea9ac` |
| Eager | `90a73029406e669f7366f05a203510994466efe87f4c1df9102101fc3cbe3ec8` |
| Delayed | `0d8981feaeb438823372d26b409d2b934f04859e006e3bfbedeafd5c3c8a5be4` |

The local archive `target/constraint-interning-evidence.tar.gz` has SHA-256
`472e584ed84dd0df5bd392fe37cd5e506e2e45663423a4080270e5b534811285`.
It contains raw timing logs, all favorable and unfavorable tables,
counter source and caps, decoded baseline/eager stacks, selection rules and environment
provenance. Large binaries/raw perf files remain in the measurement orb. The parent
independently parsed all three comparisons' raw logs (60 records, 264 timing rows),
recomputed all table medians, verified corpus/harness hashes, and reproduced all
published baseline/eager profile counts from decoded stacks. There are no formal
confidence intervals, global hit-rate claims or whole-import-work inferences.

After restoring production to the baseline, `cargo test -q` passed 2,411 tests
(zero failed, seven existing ignored); `just fmt`, `just clippy`, and `just lint`
also passed. The final Rust diff is entirely inside the private test module. No
runtime metadata, factory, new dependency, public API or benchmark change is retained.

## Follow-up: explain overhead and test a genuinely inactive path

The user requested a matched dependency comparison and an explanation of the
ordinary-workload cost before choosing a tradeoff. This experiment retains the same
`0b316c11` baseline. Reachable dependency sources, a discoverable interpreter, and a
working application runtime are different conditions: the earlier winning fixture
has an explicitly missing virtualenv and an explicit Django source search root.
Interpreter presence would therefore be the wrong switch for that fixture.

### Actual factory counters isolate wasted work

A temporary diagnostic based on the delayed factory ran each workload once, with
warmup set to either zero or 16. It counted every operation using fixed-size counters,
without retaining node payloads or pointer history. These are actual factory lookups,
not the earlier observational weak-table opportunity counts. Diagnostic runs are
excluded from performance comparisons.

| Workload | Nonreduced constructions | Eager lookups / hits | Delayed lookups / hits | Eager first table allocations | Delayed first table allocations |
| --- | ---: | ---: | ---: | ---: | ---: |
| NetBox | 37,993 | 37,941 / 1,002 | 2,057 / 548 | 10,898 | 93 |
| Ordinary Pretix | 146,364 | 146,298 / 45,953 | 64,965 / 35,307 | 26,216 | 611 |
| Required-8 | 64,568 | 64,568 / 0 | 0 / 0 | 10,168 | 0 |
| Pinned Pretix | 2,944,795 | 2,944,729 / 2,283,142 | 2,761,942 / 2,168,173 | 34,595 | 6,145 |

The lookup-free standalone `required` constructor explains counts below the total
nonreduced constructions. Empty maps allocate nothing; first insertion and later
capacity growth are counted separately. Required-8 eager had another 10,296 growths;
delayed had none. Its measured maximum nonreduced constructions per operation were
7 for union, 7 for intersection, and 15 for selection. Unlike the earlier occupancy
maxima, these actual per-operation counts prove that this trace never activates at 16.
Any remaining delayed Required-8 difference cannot be table lookup/allocation cost;
fingerprint computation, the larger node layout, generated code, and noise remain.

All eight diagnostic runs had zero unequal full-equality misses and zero capacity
cleanup/clear events. Collision handling and the 512-entry cleanup policy are not
the observed cause on these traces. This is not a proof that those paths never cost
anything on other inputs. Pinned eager avoided 2,283,142 branch allocations, leaving
661,653; delayed left 776,622. Both still constructed arm vectors before looking up a
parent. The pinned vectors presented to constructors totalled 102,267,616 capacity
bytes in either mode, versus 2,105,600 for Required-8. These totals describe allocation
work, not peak memory, live memory, or allocator-rounded bytes.

The parent reviewed the diagnostic hooks and independently checked histogram totals,
lookup-outcome sums, and `nonreduced constructions = allocations + hits` in all
eight reports.
No collision/cleanup performance bug was demonstrated. The evidence instead shows
that eager sharing adds many unproductive lookups and small table allocations on
low-reuse inputs, while eliminating millions of branches on the pinned input.

### Side tables remove hashing and node metadata from the inactive path

Experimental `23cd0530c6a6ff600b39e5e2ca3e3b6133cdb3f0` restores the original
`ConstraintBranch { join, arms }` layout (88 bytes on this x64 orb). The operation
factory holds two weak side tables: a structural-fingerprint reuse table and an
address-keyed fingerprint cache. Each is capped at 512 records. Cache reads upgrade
the weak reference and verify pointer equality; structural reuse still verifies
complete equality. No raw pointer is dereferenced, and no table owns a child graph.

The first 16 nonreduced constructions neither hash nor probe or allocate either
table; construction 17 activates. Capacity zero is a fully disabled diagnostic mode,
and warmup zero is eager. Reductions precede counting. Nodes contain no factory or
metadata, and equality, order, validation, forgetting, and widening schedules remain
unchanged. Active costs now include hashing unregistered descendants, weak-cache
probes, and possible recomputation after eviction. A cheap inactive path does not
establish that the active path is cheaper than the cached-word experiment.

The parent independently ran `cargo test -q -p djls-project`: 1,506 passed, zero
failed. The verbatim-baseline differential trace also passed 552 exact shapes,
9,936 world-membership checks, and 6,072 each equality/order comparisons. As before,
warmup 16 stays inactive on that small trace; warmup 1 exercises mixed operation
behavior, and separate tests verify activation on construction 17, zero inactive
hash/lookup work, bounds, collisions, expiry, stale-address rejection and factory drop.

### The side-table screen does not improve the tradeoff

Four balanced orders compared A=baseline, B=eager, C=sidecar-16, D=sidecar-disabled:
ABCD / BDAC / CADB / DCBA. Each workload ran in its own process. Ordinary cells
have ten samples/iterations per process (40 total); pinned cells one (four total).
These fixed executables contain no diagnostics. All 64 processes exited zero.
Values below are medians of the four process medians, in milliseconds.

| Workload | Baseline | Eager | Sidecar-16 | Sidecar disabled | Sidecar-16 change |
| --- | ---: | ---: | ---: | ---: | ---: |
| NetBox | 86.475 | 91.080 | 94.965 | 85.295 | +9.8% |
| Ordinary Pretix | 126.85 | 137.60 | 147.30 | 132.35 | +16.1% |
| Required-8 | 56.055 | 60.470 | 59.810 | 55.720 | +6.7% |
| Pinned Pretix | 1,067.5 | 725.6 | 901.0 | 954.9 | -15.6% |

Pinned peak RSS ranges were baseline 61,096–61,376KiB, eager 19,092–19,392KiB,
sidecar-16 19,064–19,292KiB, and disabled 61,296–61,332KiB. The sidecar preserves
the memory benefit but gives up much of eager's time benefit and does not remove
ordinary overhead. No threshold sweep or second sidecar design followed this screen.

Process drift is visible even in the disabled control: its pinned medians were
865.6 / 870.8 / 1,055 / 1,039ms, versus baseline 970.7 / 1,104 / 1,054 / 1,081ms.
Ranges overlap for all disabled-control workloads. This is not evidence that doing
no interning makes the pinned workload 10.5% faster, nor proof of identical code
generation. Disabled is a compile-time constant build, not a measured runtime
policy switch. Likewise, the sidecar's Required-8 delta cannot be assigned to table
work: that operation trace never activates. Counter/branch/frame costs, generated
code, and shared-orb timing variation have not been individually isolated.

### Matched fixtures separate source availability from interpreter availability

Harness `28a0938e01e551b01ba37080d456d9f2f852b8d1` adds a separate ignored benchmark,
without changing any existing name or body. Healthchecks, NetBox, and Pretix each
have three modes: project-only, common pinned Django 6.1rc1-only, and manifest-matched
dependency sources. Each mode retains the identical discovered first-party prefix,
settings entry, and explicit missing virtualenv; only `SitePackages` roots differ.
External module bodies are not evaluated.

Manifest inputs are the projects' default requirements plus their resolved transitive
closure, frozen into hash-locked fixtures on 2026-09-16. Healthchecks has 29
distributions including Django 6.0.2; NetBox 104 including Django 5.2.11; Pretix 130
including Django 4.2.30. The common Django-only fixture is deliberately incomplete
and not version-matched. Unrequested optional/dev extras, stdlib sources, editable
application installation, native-runtime success and application execution are not
claimed. Dependency `.py`/`.pyi` path/content hashes matched an independent replay.

Preflight checks separately report external-root presence, successful Django/nested
module resolution, and manifest-source completeness. Report mode uses a fresh
database with the same entry-resolution history as timing and serializes complete
settings products. The parent checked all nine root/entry/witness contracts, exact
equality with replay reports, and all three project-only products against existing
corpus snapshots. The combined harness/sidecar benchmark passes `cargo check`.
Only within-fixture baseline/candidate products must agree; different dependency
modes may legitimately yield different evidence.

### Declared dependencies confirm the Pretix benefit, not a universal rule

The final identical-harness matrix ran baseline/eager/sidecar-16 in three balanced
ABC/BCA/CAB passes. Each fixture ran in a separate fixed-executable process with five
samples/iterations: 81 processes and 405 measured operations, all successful. All 27
variant/fixture preflight reports matched the corresponding reference exactly.
This remains cold settings evaluation, not server startup or per-keystroke latency.
Values are medians of three process medians, in milliseconds.

| Project / dependency sources | Baseline | Eager | Sidecar-16 | Eager difference |
| --- | ---: | ---: | ---: | ---: |
| Healthchecks / project-only | 60.69 | 61.20 | 68.93 | +0.51 (+0.8%) |
| Healthchecks / Django-only | 61.10 | 64.12 | 65.39 | +3.02 (+4.9%) |
| Healthchecks / manifest | 65.99 | 60.33 | 63.36 | -5.66 (-8.6%) |
| NetBox / project-only | 81.60 | 86.98 | 94.59 | +5.38 (+6.6%) |
| NetBox / Django-only | 219.8 | 227.6 | 263.4 | +7.8 (+3.5%) |
| NetBox / manifest | 236.2 | 248.2 | 279.6 | +12.0 (+5.1%) |
| Pretix / project-only | 123.2 | 129.6 | 146.6 | +6.4 (+5.2%) |
| Pretix / Django-only | 1,063.0 | 738.8 | 830.0 | -324.2 (-30.5%) |
| Pretix / manifest | 1,018.0 | 774.3 | 977.2 | -243.7 (-23.9%) |

Manifest-Pretix baseline pass medians were 1,097 / 1,018 / 921.5ms; eager 782.9 /
774.3 / 643.6ms; sidecar 1,030 / 977.2 / 865.3ms. Eager's ranges are disjoint from
baseline's, and the gain repeats in every pass. Median peak RSS fell 65,536 →
19,960KiB, saving 44.5MiB (69.5%). The sidecar retains similar memory savings but
its median time benefit is only 40.8ms (4.0%).

Manifest-NetBox baseline medians were 236.2 / 237.0 / 234.3ms, versus eager 248.2 /
270.0 / 234.8ms. Its source-present modes do not show the Pretix speedup; an early
external-root check is not a demonstrated universal win predictor. Small Healthchecks
differences and some ordinary rows have broad, overlapping ranges and process drift;
the signed percentages are observations, not formal significance claims. No assumed
frequency of these projects in the user population is attached to this matrix.

### Profiles support bookkeeping cost, not a complete causal wall-time breakdown

Fresh losing-workload profiles use 499Hz user-CPU sampling over 20 iterations per
fixed binary. Captures with reported loss were preserved but excluded from inference;
replacement captures and all selected decoded sample totals were checked. Factory
frames include existing allocation work as well as newly introduced checks, so their
sampled time must not all be labelled extra table overhead.
Zero reported sample loss is not complete stack attribution: the selected captures
have 0–32 stacks with unresolved frames and 0–1 unresolved leaves each. Inlining and
the bounded DWARF stack window also limit ancestor attribution.

NetBox baseline/eager captures have 820/864 samples. Constructor coverage rises
17 → 39 samples, and eager factory coverage is 29 (roughly 2.9 sampled CPU ms per
iteration). Pretix has 1,240/1,356 samples, constructor coverage 60 → 128, and eager
factory coverage 87 (roughly 8.7ms per iteration). These inclusive categories overlap
with normalization and allocation and are not additive wall-time deltas.

Required-8 baseline/control/eager/delayed captures have 563/572/621/648 samples;
factory coverage is 0/0/31/10. The delayed factory's ten samples cannot be table
time: exact counters establish zero lookups and table allocations for that trace.
Fingerprint-only profiles do not isolate a stable magnitude of timing overhead.

There is concrete layout cost: on the measured x64 layout the baseline/sidecar
branch payload is 88 bytes and the cached-word payload 96. Including the 16-byte
Arc header gives requested sizes of 104 versus 112 bytes. A direct libc
`malloc_usable_size` probe on the measurement orb maps those to 104 versus 120
usable bytes, crossing an allocation size class. That is consistent with the earlier
fingerprint-control RSS increase, but is not a causal wall-time attribution.
Generated-code and locality effects remain possible, not demonstrated bugs.

### Policy conclusion

The original eager experiment remains the better candidate if the product accepts
this tradeoff. A roughly 244ms/44.5MiB saving on declared-source Pretix can reasonably
be worth a 12ms NetBox cost. Requiring every workload to improve is a policy choice,
not a correctness requirement; the measurements do not prescribe that choice.
The sidecar is not retained as an improvement, and no threshold sweep was performed.

Search-path availability is known before settings evaluation, so an early policy is
technically possible. A hypothetical selector could choose the baseline for
project-only fixtures and eager interning when external sources are reachable. This
would avoid the measured project-only penalties by construction but retain the
source-present NetBox penalty. Such a table is a zero-dispatch/representation-cost
estimate, not an implemented or measured runtime switch. Policy would need propagation
through nested binding/value/effect operations, and simply disabling lookups does not
remove the fingerprint field's layout cost. No switch or production interner is enabled
by this experiment. No semantics, environment discovery, or import policy changed.

### Reproducible follow-up evidence

The separate `target/constraint-interning-round2-evidence.tar.gz` is 27,002,527 bytes,
SHA-256 `adfb8c4b04169c76d8e25bfbb5cf17c09e9237d17ce4517060255cb881cc9acb`.
The original archive remains unchanged. The new archive contains all 145 timing
process logs (901 samples/iterations), 27 full preflight reports, eight actual counter
traces and their source, twelve selected decoded profiles, excluded lossy/incomplete
capture evidence, fixture locks/source hashes, exact candidate bundles, final harness,
allocator probe and independent correctness evidence. Fixed binaries and raw perf
recordings remain in the measurement orb.

The parent verified the archive and bundled input checksums, independently parsed all
145 raw timing rows including durations, iteration counts, RSS and exit status,
recomputed every matrix median, compared all 27 complete output reports with the
reviewed references, checked every raw diagnostic field and histogram, and recounted
all twelve selected profiles against the documented selectors and perf sample totals.
The audit output is `target/constraint-interning-round2-parent-audit.log`.

The phone-friendly HTML explainer includes the follow-up separately from historical
comparisons. Chromium inspection covered 320/390/1280 CSS-pixel layouts, the expanded
side-table explanation, and the existing comparison/step controls; no horizontal
overflow or JavaScript error was observed. This is narrow-layout testing, not a
physical-phone or Safari test. Production source was not modified in this follow-up.

## Follow-up: boxed child storage with and without sharing

The next experiment isolates stored child representation from the sharing policy.
The previous allocator probe establishes an avoidable-looking size boundary, not
that a smaller container necessarily improves wall time. A `Vec<ConstraintNode>`
stores pointer, length, and capacity; the immutable stored children do not need to
grow. A boxed slice removes the capacity word, but converting a vector with spare
capacity can introduce shrinking work, including before an eager cache hit.

The four controls are A=exact baseline `0b316c11`, B=exact eager `e586e944`, C=A with
boxed stored children, and D=B with boxed stored children. Child construction stays
in vectors. Reduction, fingerprint, lookup/full-equality, ordering, validation,
forgetting, widening, and table-capacity policies stay fixed. D deliberately boxes
before the existing interner lookup; avoiding temporary construction on hits is a
different experiment. No inline builder, side table, arena, Salsa node interner,
fine-grained query memoization, environment switch, or threshold sweep is included.

Predeclared checks include actual node/container layout and allocator classes;
constructor length/capacity distributions; requested child-buffer bytes; branch
allocations; fingerprint/lookup/hit counts; and boxed conversions with excess
capacity, distinguishing allocator reallocation calls from pointer relocation.
These diagnostics run separately from fixed, uninstrumented timing binaries.
Correctness checks retain the exact-baseline structural/truth-table trace and
compare all 36 variant/fixture settings reports with the nine reviewed references.

The initial screen uses manifest-dependency Pretix and NetBox, project-only Pretix
and NetBox, and unchanged Required-8. Four balanced orders (ABCD / BDAC / CADB /
DCBA), five samples per separate variant/fixture process, yield 80 processes and
400 measured iterations. Compare C against A, D against B, and especially D against
the compact baseline C. Record process RSS, all pass timings and drift, not just a
favorable aggregate. Broader timing of the existing nine fixtures is conditional
on the initial screen. Prior archives remain unchanged and production stays on the
baseline; no new result is claimed by this protocol.

### Exact candidates recover the fingerprint word's space

C is `9232d805255327f4691e4f77c2486a61754e91e5`, with sole bundle prerequisite A;
D is `a979e2d044b6495da0698f90411be5fd4a14798a`, with sole prerequisite B. Parent
review verified both bundle checksums and the complete diffs: only the stored arms
type and conversion change production code. Each adds an excess-capacity test;
the eager test also requires pointer reuse on a repeated construction. D converts
after hashing the vector contents but before finishing the hash and entering the
existing interner. No cache-hit construction optimization is hidden in this change.

Observed x64 payload sizes are A=88, B=96, C=80, D=88 bytes, all aligned to eight.
`ConstraintNode` remains 16 bytes; Vec and boxed-slice headers are 24 and 16 bytes.
Including Arc headers gives allocation requests A=104, B=112, C=96, D=104 bytes.
The measurement orb's unchanged libc maps requests 96 and 104 to 104 usable bytes,
and 112 to 120. C therefore does not cross an allocator class relative to A; D
does relative to B. This is per-node allocation evidence, not a wall-time result.

The existing full project suites passed with 1,500 tests for C and 1,504 for D,
zero failures. Parent parsing of the preserved test logs confirms those totals.
Both candidates passed all-target Clippy and formatting checks. Quiet Clippy logs
are empty; their exit statuses and formatter/hook results were recorded in tool
responses, and the implementation archive labels that distinction explicitly.

Independent focused verification passed two tests per candidate. C covers 23 exact
trace shapes, 414 truth-table memberships, 253 each equality/order comparisons, and
nine extra asymmetric truth checks. D repeats the trace for capacities 0/1/2/512
and normal/constant fingerprints: 184 shapes, 3,312 memberships, 2,024 each
equality/order comparisons, and 72 extra truth checks. Additional checks use spare
capacities 127/129, ordered nested children and reversal, same-child reduction,
metadata-independent equality/debug/order, and cross-thread result lifetime after
dropping factories and local owners. No warmup behavior is claimed. Parent audit
rechecked that the independent baseline source is byte-exact `0b316c11`.

### Boxing did not resize any observed child buffer

All 36 full settings reports matched the previously reviewed fixture references
byte-for-byte. Parent and independent audits checked exact variants, the common
harness (including its Cargo manifest/lockfile change), source hashes, ordered
roots, complete output files, and all successful preflight process exits.

Twenty untimed diagnostic processes covered all four variants on the five screen
fixtures. Each exited zero and emitted one complete thread dump with zero histogram
overflow. Every incoming vector had length equal to capacity. All boxed conversions
had zero allocation/deallocation, reallocation/shrink calls, and pointer relocation.
The separate spare-capacity smoke test recorded a real 256-to-24-byte realloc with
no relocation, confirming that the hook distinguishes in-place shrinking from no
allocator call. These observations do not establish that boxing arbitrary inputs
never reallocates or that conversion has zero instruction cost.

All ten A/C and B/D pairs matched exact constructor/reduction/hash/lookup/hit/miss
and branch-allocation counts, child length/capacity histograms, and cumulative child
byte totals. The boxed representation did not change the observed sharing rate.
For full-dependency Pretix:

| Variant | Branch allocations | Request per branch | Cumulative node requests |
| --- | ---: | ---: | ---: |
| A | 3,174,105 | 104 bytes | 330,106,920 bytes |
| B | 702,146 | 112 bytes | 78,640,352 bytes |
| C | 3,174,105 | 96 bytes | 304,714,080 bytes |
| D | 702,146 | 104 bytes | 73,023,184 bytes |

Every variant presented 110,176,112 cumulative child-capacity bytes to constructors.
D converted 3,174,105 vectors, including 2,471,959 followed by sharing hits, without
shrinking. These are cumulative requested/input bytes, not retained memory or RSS.
The temporary allocator scopes are valid for the frozen, nonnested successful
conversion/Arc calls; they are not a general nested or panic-safe instrumentation
framework. The parser aggregates all dumps, but its request-size consistency check
would reject mixed allocating/zero-allocation thread dumps; that limitation does
not affect the observed single-dump runs. Fixed counters retain no graph nodes.

### The screen supports a smaller representation, not a universal timing winner

The approved screen completed all 80 separate timing processes, five samples and
five iterations each, for 400 measured evaluations. The four fixed executables were
built without diagnostics, their recorded hashes remained unchanged after diagnostic
work, and no build/profile/diagnostic work overlapped timing. Warm filesystem caches
were not cleared. This is cold settings evaluation, not startup or cached LSP latency.

Values below are medians of four process medians, in milliseconds, in the declared
ABCD / BDAC / CADB / DCBA order. All raw minima, maxima, means, medians, sample counts,
wall times and per-process RSS were preserved and independently recomputed by parent.

| Workload | A: baseline Vec | B: eager Vec | C: baseline Box | D: eager Box |
| --- | ---: | ---: | ---: | ---: |
| Pretix / manifest | 1,130.50 | 750.75 | 1,095.50 | 766.45 |
| NetBox / manifest | 266.25 | 264.95 | 241.10 | 247.10 |
| Pretix / project-only | 137.85 | 141.70 | 136.00 | 139.35 |
| NetBox / project-only | 92.630 | 89.380 | 86.775 | 86.120 |
| Required-8 | 59.745 | 61.665 | 55.520 | 60.355 |

Boxed eager D versus Vec eager B improved full NetBox by 17.85ms (6.7%) in aggregate,
with all four paired-pass differences negative: -15.4/-6.6/-23.4/-51.9ms. Full Pretix
instead cost 15.70ms (2.1%) more in aggregate; paired differences were
+46.1/+48.4/+42.6/-14.7ms. Thus the smaller allocation class did not translate into
a uniform wall-time benefit. The screen does not isolate remaining instruction,
generated-code, locality or environment effects, and showed no semantic defect.

Sharing remains a large benefit on full Pretix even against the compact baseline:
D versus C saves 329.05ms (30.0%), and every D process median is below every C median.
Median peak RSS is A=65,456, B=19,922, C=65,580 and D=19,466KiB. D saves 45.0MiB
against C, but only 456KiB against B: the large memory win remains sharing, not the
extra container change. The other D-versus-C aggregate differences are +6.00ms for
full NetBox, +3.35ms for project-only Pretix, -0.655ms for project-only NetBox, and
+4.835ms for Required-8. Small differences have overlapping ranges and mixed signs.

Process drift limits causal conclusions. The unchanged B-versus-A full-NetBox
aggregate changes sign from the previous round's +12ms to -1.3ms here. Medians of
separate pass distributions also need not match typical paired differences:
project-only Pretix C-minus-A is -1.85ms in aggregate despite C being slower in
three of four paired passes. Do not present these small aggregate deltas as stable
universal improvements, or compare absolute times across rounds as one experiment.

The round stops at this screen, without a nine-fixture timing extension. Both boxed
variants remain viable experiments; neither is enabled or declared the fastest
default. The layout hypothesis is confirmed, while the measured performance
tradeoff remains workload-dependent. Eliminating temporary child-buffer construction
on sharing hits is still a distinct possible experiment, not part of these results.

### Preserved round-three evidence and explainer verification

The consolidated archive is `target/constraint-interning-round3-evidence.tar.gz`
(496,950 bytes), SHA256
`432757bb7f20138ca00c079bf127a732fd2fd51fdf68783775316f404c061907`.
Parent verification of its top-level `SHA256SUMS` passed all 248 included files.
It contains the 80 timing logs, 36 complete reports, 20 diagnostic logs, exact
candidate bundles, sources/hooks, harness/fixture identities, and implementation,
independent and parent audits. Large executables remain on the measurement orb;
parent and independent verification checked their recorded manifests, not the
executable bytes. Both earlier archives retain their previously recorded hashes.

The HTML explainer separates all three round-three comparisons from historical
phases. Executed Chromium checks compared every visible value pair, percentage,
and accessible chart label with the audited screen summary; all 15 rows passed.
Both historical comparison views and the repeated/unique step controls passed.
The 320/390/1280 CSS-pixel layouts had no horizontal overflow, comparison controls
were at least 44 pixels tall, and no JavaScript errors appeared. Inspected final
screenshots cover all three comparisons, expanded allocation details, the header
layout diagram, and the hero. These are Chromium narrow-layout checks, not
physical-phone or Safari verification. Production remains unchanged.

### Product recommendation after the screen (2026-09-17)

The experiment letters and changing denominators obscured the product decision.
Use three stable states: main, the current shared-node/shared-module branch, and
that branch plus compact duplicate reuse. The checked `origin/main` revision
`6aeef2fe` already contains demand-driven analysis; the current branch additionally
retains the immutable constraint-tree and module-identity sharing improvements.
Neither interning nor boxed children are enabled in the current implementation.

The recommendation is to integrate bounded eager hash-consing with boxed child
slices, the previously verified D candidate, then stop representation tuning.
This prioritizes a substantial expensive-case benefit over small, workload-dependent
cold-analysis costs. It is a product tradeoff, not a universal-fastest claim or an
assertion that the tested projects represent the user population. The compact form
fits immutable stored children and removes the eager allocation-class growth;
the mixed timing difference between the two eager representations does not justify
another tuning campaign.

The stable current-to-recommended round-three comparison is A to D: full-dependency
Pretix 1,130.50 to 766.45ms (364.05ms, 32.2% less), peak process RSS 65,456 to
19,466KiB; full NetBox 266.25 to 247.10ms; project-only Pretix 137.85 to 139.35ms;
project-only NetBox 92.63 to 86.12ms; Required-8 59.745 to 60.355ms. The expensive
Pretix improvement holds in all four passes. Smaller differences remain noisy;
earlier ordinary-control regressions are still part of the decision. Benchmark
peak RSS is not a measured steady-state server-memory reduction.

`django_settings` and `evaluate_python_module` are Salsa tracked functions.
`settings_consumers_share_one_core_evaluation_without_mutation` and
`settings_slice_caches_facts_and_import_trace` establish shared core evaluation and
zero repeated evaluator executions without input changes. Settings and reached
Python dependency edits can invalidate the result and trigger full project reload;
configuration/resolution changes and fresh server databases can also require work.
Ordinary template edits do not inherently rerun settings evaluation, although active
editing of those Python dependencies can cause repeated recomputation. Local node
reuse optimizes work inside an actual evaluation; it does not replace Salsa caching.

The historical pinned-Django comparisons were 8.825 to 3.662s for shared trees and
3.987 to 1.029s for shared module identities. They show the scale of retained gains,
but are successive matched comparisons with remeasured baselines, not one verified
84% main-to-current result. They must not be combined with the new dependency fixture.

The revised overview keeps current-to-recommended costs fixed and moves the original
interactive artifact to `experiment-notebook.html`, explicitly labeled historical.
This recommendation changes neither production code nor the sealed evidence archives.
The proposed remaining engineering work is candidate integration and combined
workspace/LSP verification, not another data-structure experiment or automatic push.

### Chosen implementation integrated (2026-09-17)

After approval, integrated bounded eager hash-consing with boxed child slices into
the shared-node/shared-module branch. The final `constraints.rs` is byte-identical
to the verified `boxed-constraint-arms-D` candidate after formatting and linting.
Its 512-entry weak table remains operation-local; complete structural equality
checks reuse, and returned values own their graphs independently of the table.
Vector builders, pre-lookup boxing, validation, ordering and intermediate widening
remain exactly as tested. No thresholds, sidecars, routing rules or Salsa plumbing
were added. The existing intermediate-forgetting regression remains covered.

Combined integration verification passed:

- `cargo test -q`: 2,415 passed, zero failed, seven ignored by the suite.
- `cargo build -q -p djls` and `just e2e`: build succeeded; 48 LSP tests passed.
- `just fmt`, `just clippy`, `just lint` and `git diff --check`: passed.

Logs are preserved under `target/constraint-integration/`. No further comparative
timings were run. All three sealed evidence archives retain their recorded hashes.
The overview now labels the same comparison as **before the final step → finished
branch**: its baseline is the retained foundation before duplicate reuse, not a
moving reference to whatever is currently checked out. The research sections above
remain historical, including their then-current recommendation/status statements.

This finishes the representation experiments. Accept the measured occasional-analysis
tradeoff and proceed with ordinary code review rather than another tuning round.
Integration is local and uncommitted; nothing has been pushed or opened as a PR.

### Review follow-up (2026-09-17)

Restored the merge-base constraint `Debug` shape with a node-level formatter and
fixed compact/pretty snapshots. The compact regression failed before the fix.
Renamed the private operation-local `ConstraintFactory` to `ConstraintInterner`
to name its reuse-table responsibility. After normalizing that mechanical rename,
the only differences from the measured candidate are the formatter and its test;
the earlier byte-identity statement describes the initial integration checkpoint.
`cargo test -q -p djls-project` passed all 1,505 tests, and crate all-targets Clippy,
`just fmt --check` and `git diff --check` passed. No benchmark rerun or policy change.

### Remaining evaluator/import work (2026-09-17)

This is a new experiment series, not a reinterpretation of the historical
constraint experiments above. Its baseline is main at
`91023195273117726ede99ae0137b89fa38e91fa`, including #898, #906, #907 and #909.
No constraint interning, batching, branch ordering, per-branch guarded
intersection, or intermediate widening policy changed.

Retained implementation stages:

1. Completed module evaluations use structurally compared `Arc` payloads.
   Single-member projection borrows the cached facts instead of cloning the
   entire module. Cycle recovery still obtains mutable ownership through
   `Arc::unwrap_or_clone`; full imported-module consumption still clones when
   necessary. This removes work proportional to the entire cached payload from
   each member read, not the cost of evaluating an imported module initially.
2. Effect joins borrow branch effects, guards and candidate bindings. Unchanged
   candidate bindings are cloned once for the output rather than once per
   branch; changed candidates retain their original selection/intersection/join
   sequence. Both whole-effect snapshot clones at the evaluator join boundary
   are gone. Coordinate collection and lookup retain their original algorithms.
3. Absolute import-chain resolution is a tracked query keyed by project and
   normalized absolute module name. A test retaining 16 equivalent absolute and
   relative import results observed 16 distinct source-identity allocations
   before the change and one afterwards. A separate settings test verifies that
   creating and deleting an imported file reruns the chain query, while editing
   its contents updates settings without rerunning chain resolution. Hits still
   clone the small component vector; cached source identities are shared.
4. Search-path computation records which site-packages roots it has scanned for
   `.pth` files. The explicit-plus-discovered overlap test goes from two walks
   and four reads to one walk and two reads per computation, preserves editable
   ordering and explicit classification, and observes changed `.pth` contents
   on the next computation. This is computation-local deduplication, not a
   persistent filesystem cache.

#### New measurements and contracts

Linux 6.1.158+, two Xeon 2.60GHz vCPUs, approximately 4 GiB RAM, Rust 1.97.1.
Release executables were copied before changing candidates; no builds or tests
ran during timing. Every pair used the same harness and lockfiles. Ordinary
settings samples use fresh databases with setup outside timing; filesystem
caches may be warm. Existing benchmark names and measured paths are unchanged.
Two companion families were added: `settings_cold_module_members::{8,64}` reads
one member repeatedly from a module with 256 unrelated constants;
`settings_cold_repeated_imports::{8,64}` imports the same three-component package
chain under distinct aliases. These are targeted stress cases, not representative
claims about every Django project.

Commands after `cargo bench -p djls-bench --bench extraction --no-run`:

```text
<fixed-binary> --bench settings_ --sample-count 10
<fixed-binary> --bench --ignored settings_cold_pretix_with_django
```

Each comparison used three pairs ordered AB/BA/AB. Tables report the median of
three batch medians, in milliseconds. The final isolated import-cache comparison
used 30 ordinary samples per batch (90 per case/variant); earlier comparisons
used 10 (30 total). Pinned-Django uses one sample per batch (three total).

| Comparison | Workload | Before ms | After ms |
| --- | --- | ---: | ---: |
| Shared completed evaluations | Module members 8 | 0.8901 | 0.4926 |
| Shared completed evaluations | Module members 64 | 5.386 | 1.953 |
| Shared completed evaluations | Healthchecks | 61.65 | 60.26 |
| Shared completed evaluations | NetBox | 86.58 | 83.53 |
| Shared completed evaluations | Pretix | 148.2 | 131.9 |
| Shared completed evaluations | Required branches 8 | 62.80 | 64.40 |
| Shared completed evaluations | Pinned-Django Pretix | 684.1 | 679.6 |
| Borrowed effects only | Healthchecks | 57.82 | 57.75 |
| Borrowed effects only | NetBox | 79.80 | 79.41 |
| Borrowed effects only | Pretix | 122.1 | 123.5 |
| Borrowed effects only | Required branches 8 | 57.88 | 58.50 |
| Borrowed effects only | Pinned-Django Pretix | 640.2 | 633.2 |
| Isolated import cache | Repeated imports 8 | 0.1686 | 0.1549 |
| Isolated import cache | Repeated imports 64 | 2.136 | 2.017 |
| Isolated import cache | Healthchecks | 58.27 | 57.69 |
| Isolated import cache | NetBox | 77.78 | 77.67 |
| Isolated import cache | Pretix | 121.3 | 122.6 |
| Isolated import cache | External constants | 0.09954 | 0.1086 |
| Isolated import cache | Irrelevant branches 8 | 0.06858 | 0.07432 |
| Isolated import cache | Irrelevant branches 64 | 0.1727 | 0.1941 |
| Isolated import cache | Required branches 8 | 58.25 | 56.89 |
| Isolated import cache | Pinned-Django Pretix | 638.9 | 642.8 |

Repeated member reads improved 45–64%; repeated imports improved 6–8% with
non-overlapping batch-median ranges. Retain the import cache for that measured
benefit and the eliminated repeated constructions, accepting a few microseconds
of additional small cold-query overhead. Some noisy microbenchmark batches were
larger (including irrelevant-branches 64); do not advertise zero regressions.
The corpus and pinned results do not establish a material general speedup for
the latter stages. Borrowing removes identifiable copies without changing join
complexity, but has no convincing broad elapsed-time win here. Cross-series
timings drift: do not combine them into one main-to-final speedup. `.pth` savings
are operation counts, not elapsed-time measurements.

The final isolated cache pair differs only by the tracked-query annotation,
owned name argument and two call-site clones, with the same completed payload,
effect and `.pth` changes. Cached binary SHA-256:
`0a13d36b810612b1a412c6dcc384bcb66f9308f59f17e33a03f8513301abbbba`;
uncached: `0493273cd06deb873cc1fbef77336945f7ded1aa39f350b217dcca0c24f8db10`.
Final harness SHA-256:
`6f6be2301c9226cb7ec322271f7a32b42bddd521d7d39abecd61040111910975`.
Raw batch output and summaries are retained locally in `target/python-perf/`.

#### Rejected and deferred work

- Sorted borrowed coordinate collection plus binary child lookup regressed
  NetBox from 79.50 to 89.96 ms. All three baseline batch medians (78.52–80.35)
  were below all candidate medians (89.90–90.48). It was removed; the narrower
  borrowed-input version above was remeasured independently.
- Temporary inline constraint child builders were inspected but not implemented
  or measured. Surviving nodes still require boxed slices before interner lookup;
  the immediate allocation saving is for branches that collapse before boxing.
  There is no allocator profile proving this remaining path is worth another
  representation or dependency. This is deferred, not a measured rejection.
- No settings-only reload orchestration, discovery changes, new interning or
  batching campaign, or constraint precision changes belong to this work.

Verification: `cargo test -p djls-project` passed 1,511 tests; `cargo test -q`
passed 2,422 tests with seven suite-ignored tests. Commit hooks passed workspace
Clippy and formatting. `just hawk` completed analysis but reported four
`hawk::unnecessary_public` findings for the unchanged
`PythonSourceModule::{name,path,file,search_path}` getters; the integration owner
will reassess them with the other workstreams. Its log is
`/tmp/python-hawk.log`. No visibility cleanup was made. Warm settings requests
and helper-edit reuse are covered by event-count tests, not new timing claims.
No new sampling/allocator profile, Django/Python matrix, dependency-manifest
benchmark matrix, or LSP end-to-end run was performed in this series.
