# Live Django facts with pytest

`tools/pytest_django_facts.py` is a repository-local pytest plugin loaded by
`tests/conftest.py`. It is not an LSP test and is not an installable user plugin.
The Rust binary asks production queries over real source and runs Django in the
target environment's subprocess, including Python 3.6.

## Run a shard

```bash
just django-facts -m facts_fixtures --facts-required-shard fixtures --junitxml target/oracle/fixtures/junit.xml --facts-output target/oracle/fixtures
just django-facts -m facts_blocks --facts-required-shard blocks
just django-facts -m facts_helpers --facts-required-shard helpers
just django-facts -m facts_nightly --facts-required-shard nightly
```

`just django-facts` runs `nox -s facts`, which installs pytest and runs ordinary
`pytest tests/facts --django-facts --facts-prepare`. Environment preparation uses
the existing corpus manifest, exact `requirements/*.txt`, and configured Python versions.
The PR workflow runs fixtures, blocks, and helpers. Nightly also runs nightly.
Both workflows upload the JUnit file and native evidence, including on failure.

With prepared environments, use pytest directly:

```bash
uv run --no-install-project pytest tests/facts --django-facts -m facts_blocks
uv run --no-install-project pytest tests/facts/django.facts.json --django-facts -k django-bird
uv run --no-install-project pytest tests/facts --django-facts --collect-only -m facts_helpers
```

The custom collector reads `tests/facts/django.facts.json`, a required-work
inventory, and yields one item per library, tag, filter, template name, or
compilation seed in each settings/backend scope. Probe texts and exploration
status belong to those subjects; the inventory preserves their identities too.
It contains no expected DJLS answers. Targets run lazily, once when the first
selected item needs them, so marker and `-k` selection do not start other
environments. Live facts and their policy tests require `--django-facts`.
Plain pytest discovery does not collect them or build/run the oracle.
The e2e nox session uses default pytest discovery under `tests/`, including
`tests/test_release.py` and new test files, without opting in to live facts.

`--facts-required-shard` additionally checks the written PR matrix, target
partition, and selection completeness. CI cannot accidentally pass with `-k`
excluding required items, a removed inventory file, or missing target membership.

## Results and allowances

Pytest owns the terminal and JUnit results. A subject with exact known
disagreements is **xfail**, with reasons, issue numbers, and triage status. An
unlisted or changed disagreement fails. A known disagreement is not agreement.
One item can contain several findings, so xfail item counts differ from class totals.
The terminal summary prints all four finding counts per target and settings mode.
JUnit properties preserve the full evidence and allowance for each finding.

At session end, unused ledger entries and lost subject identities fail as the
`django-facts::session-accounting` JUnit case. Added subjects, changed environment
versions/setup/exploration counts, and deleted scopes also fail. Even a focused
developer selection checks the full live target it started. Unselected targets
are not called stale. `--facts-required-shard` enforces complete CI selection.

Native JSON and Markdown remain in `target/oracle/<output>/`, with one file per
target and an aggregate `report.json`. JSON includes every compared native probe,
its actual DJLS diagnostics, and scoped symbol lookups, including agreements.
The maintained ledger is `crates/djls-testing/oracle/disagreements.json`.
Its evidence fingerprints detect changes; they never supply the Django answer.

The ledger is **not human-reviewed**. `triage: triaged` means a root cause has
source or runtime evidence. `triage: untriaged` means the disagreement is observed
but its extraction cause is not established. An issue number alone does not
establish a root cause. Terminal output makes the remaining untriaged count explicit.
Reasons that only repeat a result do not count as root-cause analysis.

## Propose accounting changes

```bash
just django-facts -m facts_fixtures --facts-update --facts-output target/oracle/proposal
```

This writes `disagreements.proposed.json` and `subjects.proposed.json` separately.
New or changed findings have blank reasons and root causes, explicit null issues,
and untriaged status. They cannot pass. The update option does not turn a failed
check green or modify the maintained ledger. Review the native evidence, identify
the cause where possible, and merge the selected proposal deliberately. Keep
unresolved causes explicitly untriaged. Do not file issues automatically.

## Coverage and deferrals

`targets.json` defines the target configuration, required shards, and partition.
The subject inventory independently retains all ten corpus repository obligations.
Deleting a target from configuration does not remove its coverage obligation.
Each target needs live runs or a reasoned deferral, never both, and one shard.

A whole-target deferral needs stage, exact normalized final diagnostic, reason,
root cause, triage status, and an issue number or null. The attempted command still
runs. Preparation deferrals retry preparation on every session. An unexpected
success, changed failure, or disappeared allowance fails. Partial import/setup
failures remain individual Unchecked findings. No whole-target deferrals are active.

Wagtail and sekizai run in required PR shards. Permission is nightly because its
Python 3.6 interpreter requires a source build. InvenTree's 153-package application
runs nightly in both project and synthetic modes. Bird has both its synthetic
corpus run and a source project using the actual pinned package with builtin
configuration matching upstream's tests. Their different diagnostics are visible.

## Reference limits

Wrong includes definite block contracts contradicted by Django, even when an open
symbol inventory suppresses diagnostics. Only an absent TagSpec is Imprecise.
Missing argument witnesses, unexplored stops, signature failures, native exceptions,
and instrumentation/native differences are Unchecked and need exact allowances.
Instrumented facts are withheld explicitly when native replay disagrees, not dropped.

Template resolution uses Django's public `get_template`, including custom overrides.
For invalid source, a TemplateSyntaxError traceback can expose the actual selected
Template's origin before compilation failed. Only a matching requested template
name is used. If Django exposes no origin, the result remains Unchecked.

Generation is finite. Cached invalid openers, missing candidate vocabulary, bare
intermediates, and manual token-loop blocks remain documented limits. All 194
compilation seeds receive live native verdicts. Old goldens remain until their
coverage is reconciled explicitly. No production extraction fix is
included in this plugin migration.

The Rust `django-oracle project --root ... --venv ... --settings ...` command
continues to support real projects outside this CI target list. Installing the
pytest plugin into users' projects is future work, not part of this change.
