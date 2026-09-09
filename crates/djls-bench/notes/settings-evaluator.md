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

## Validation

- `cargo test -q`: full workspace suite passed.
- `cargo test --release -p djls-project --test settings_extraction --test corpus_settings`: 315 settings tests and 3 corpus tests passed before the final equality refinement; the full workspace run covered that refinement afterward.
- All existing settings snapshots remained unchanged.
- A new test checks identity equality against canonical comparison and checks full equality against structural comparison across different origins, kinds, discriminators, domains, and module search roots.
- `just fmt` and all-target/all-feature Clippy passed.
