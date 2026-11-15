"""Repository-local pytest collection of live Django facts, not LSP tests.

The inventory records work owed, never expected DJLS answers. Django and the Rust
production queries run once per selected target. Each subject is a pytest item;
the session checks unused allowances and lost work independently of item results.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
from collections import Counter
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / "crates/djls-testing/oracle"
STATE = pytest.StashKey()
CLASSES = {"wrong", "missed", "imprecise", "unchecked"}


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON key: " + key)
        result[key] = value
    return result


def read(path):
    return json.loads(Path(path).read_text(), object_pairs_hook=unique_object)


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def unchecked_cause(finding):
    if finding["family"] == "library_load":
        if finding.get("symbol") == "setup-coverage":
            if finding["django"].startswith(
                "Django could not start with every library"
            ):
                return "setup_reduction"
            if (
                finding["django"]
                == "Template resolution compared only the first 5000 Template Names."
            ):
                return "template_name_budget"
            raise ValueError("unclassified Unchecked setup observation")
        if finding["django"] == "import failed in this environment":
            return "import_failure"
        raise ValueError("unclassified Unchecked library observation")
    return {
        "argument_requiredness": "generator_shapes",
        "block_structure": "unexplored_stop_tags",
        "reference_integrity": "instrumentation_mismatch",
        "validation": "native_exception",
        "filter_arity": "uninspectable_signature",
        "template_resolution": "no_loader_origin",
    }[finding["family"]]


def observations(reports):
    runs, findings = {}, {}
    for report in reports:
        scope = "/".join([report["project"], report["settings"], report["backend"]])
        subjects = report["coverage"]["subjects"]
        if scope in runs or not subjects or len(subjects) != len(set(subjects)):
            raise ValueError("duplicate run, empty or duplicate subjects: " + scope)
        runs[scope] = {
            "subjects": sorted(subjects),
            "counts": {k: v for k, v in report["coverage"].items() if k != "subjects"},
            "django_version": report["django_version"],
            "python_version": report["python_version"],
            "notes": report["notes"],
        }
        for finding in report["findings"]:
            key = scope + "|" + finding["key"]
            if key in findings or finding["class"] not in CLASSES:
                raise ValueError("duplicate finding or unknown class: " + key)
            findings[key] = {
                "class": finding["class"],
                "django": finding["django"],
                "djls": finding["djls"],
                "subject": finding_subject(finding),
                "fingerprint": hashlib.sha256(
                    json.dumps(finding, sort_keys=True).encode()
                ).hexdigest(),
            }
            if finding["class"] == "unchecked":
                findings[key]["cause"] = unchecked_cause(finding)
    return runs, findings


def subject(identity):
    """Group probe/exploration evidence with the tag or filter it tests."""
    kind, _, rest = identity.partition(":")
    if kind == "probe":
        # Registered names already have subjects, including names with colons.
        # Seeds have no registration inventory of their own.
        if rest.startswith("tag:compilation-seed:"):
            return ":".join(rest.split(":", 3)[:3])
        return None
    if kind in {"block-exploration", "argument-witness", "filter-arity"}:
        return ("filter:" if kind == "filter-arity" else "tag:") + rest.rsplit(":", 1)[
            0
        ]
    if kind in {"library", "candidate"}:
        return "library:" + (
            rest.split(":", 1)[0] if kind == "library" else rest.split(":", 1)[1]
        )
    return identity


def finding_subject(finding):
    family, library, name = finding["family"], finding["library"], finding["symbol"]
    if family == "template_resolution":
        return "template:" + name
    if family == "library_load":
        return "library:" + (library or "setup-coverage")
    if family == "symbol_inventory":
        kind, name = name.split(":", 1)
    else:
        kind = (
            "filter"
            if family == "filter_arity" or finding["key"].endswith(":filter")
            else "tag"
        )
    return f"{kind}:{library}:{name}"


def execute(command, env=None):
    result = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, text=True)
    print(result.stdout, end="")
    print(result.stderr, end="")
    result.check_returncode()
    return result


def prepare(name, target, interpreter):
    if target["kind"] == "corpus":
        lock = CONFIG / "requirements" / (name + ".txt")
        execute(
            [
                "cargo",
                "run",
                "-q",
                "-p",
                "djls-testing",
                "--bin",
                "corpus",
                "--",
                "sync",
                "--source-only",
            ]
        )
        # The corpus bootstrap may select an older Python than the pinned oracle
        # environment below, so oracle constraints must not affect its install.
        execute(
            [
                "cargo",
                "run",
                "-q",
                "-p",
                "djls-testing",
                "--bin",
                "corpus",
                "--",
                "environment",
                "sync",
                name,
            ]
        )
        environment = ROOT / "crates/djls-testing/.corpus/environments" / name
        metadata = {
            path.name: path.read_bytes()
            for path in (environment / ".ready", environment / "provenance.json")
            if path.exists()
        }
        version = target.get("python", interpreter)
        if target.get("historical_python"):
            minor = target["historical_python"]
            version = str(
                ROOT
                / "crates/djls-testing/.corpus/interpreters"
                / minor
                / f"bin/python{minor}"
            )
        execute(["uv", "venv", "--clear", "--python", version, str(environment)])
        command = [
            "uv",
            "pip",
            "sync",
            "--python",
            str(environment / "bin/python"),
            str(lock),
        ]
        for package in target.get("no_binary", []):
            command.extend(["--no-binary", package])
        execute(command)
        for filename, value in metadata.items():
            (environment / filename).write_bytes(value)
    elif not target.get("venv"):
        execute(
            [
                "uv",
                "venv",
                "--python",
                interpreter,
                "--allow-existing",
                "target/oracle/venv",
            ]
        )
        execute(
            [
                "uv",
                "pip",
                "sync",
                "--python",
                "target/oracle/venv/bin/python",
                str(CONFIG / "requirements/fixtures.txt"),
            ]
        )


def failure(stage, error):
    lines = re.sub(r"\x1b\[[0-9;]*m", "", error).strip().splitlines()
    if not lines:
        raise ValueError("failure without diagnostic")
    return {"stage": stage, "error": lines[-1].replace(str(ROOT), "${ROOT}")}


def pytest_addoption(parser):
    group = parser.getgroup("django-facts")
    group.addoption(
        "--django-facts",
        action="store_true",
        help="Opt in to live Django facts and their policy tests",
    )
    group.addoption(
        "--facts-prepare",
        action="store_true",
        help="Recreate selected pinned environments before live comparisons",
    )
    group.addoption(
        "--facts-update",
        action="store_true",
        help="Write separate inventory/ledger proposals; new reasons remain blank",
    )
    group.addoption("--facts-ledger", type=Path, default=CONFIG / "disagreements.json")
    group.addoption("--facts-config", type=Path, default=CONFIG / "targets.json")
    group.addoption(
        "--facts-inventory",
        type=Path,
        help="Alternate subject inventory for accounting review",
    )
    group.addoption("--facts-output", type=Path, default=ROOT / "target/oracle/pytest")
    group.addoption(
        "--facts-required-shard",
        help="CI: fail if selection loses any target or subject from this shard",
    )


def pytest_configure(config):
    config.addinivalue_line(
        "markers", "django_facts: live Django facts, independent of LSP/e2e"
    )
    targets = read(config.getoption("facts_config"))
    for shard in targets["shards"]:
        config.addinivalue_line("markers", f"facts_{shard}: live Django facts shard")
    for name in targets["targets"]:
        config.addinivalue_line(
            "markers", "target_" + name.replace("-", "_") + ": live Django target"
        )


def pytest_ignore_collect(collection_path, config):
    if not config.getoption("django_facts") and collection_path == ROOT / "tests/facts":
        return True
    return None


def pytest_collect_file(file_path, parent):
    if (
        parent.config.getoption("django_facts")
        and file_path.name == "django.facts.json"
    ):
        return FactsFile.from_parent(parent, path=file_path)


class FactsFile(pytest.File):
    def collect(self):
        state = FactSession(
            self.config, self.config.getoption("facts_inventory") or self.path
        )
        self.config.stash[STATE] = state
        for name in state.targets["targets"]:
            yield FactsTarget.from_parent(self, name=name, state=state)


class FactsTarget(pytest.Collector):
    def __init__(self, *, state, **kwargs):
        super().__init__(**kwargs)
        self.state = state
        self.add_marker("django_facts")
        self.add_marker("target_" + self.name.replace("-", "_"))
        shard = next(
            shard
            for shard, names in state.targets["shards"].items()
            if self.name in names
        )
        self.add_marker("facts_" + shard)

    def collect(self):
        if self.name in self.state.inventory["deferrals"]:
            yield FactItem.from_parent(self, name="deferred-environment", scope=None)
        else:
            for scope, run in self.state.inventory["runs"].items():
                if scope.split("/")[0] == self.name:
                    names = {
                        name
                        for identity in run["subjects"]
                        if (name := subject(identity)) is not None
                    }
                    names.add("library:setup-coverage")
                    # Static-only registrations and failed imports are subjects too.
                    for key, entry in self.state.ledger.items():
                        if key.split("|", 1)[0] == scope:
                            if "subject" in entry:
                                names.add(entry["subject"])
                    for name in sorted(names):
                        yield FactItem.from_parent(
                            self,
                            name=scope.split("/", 1)[1] + "::" + name,
                            scope=scope,
                            subject=name,
                        )


class FactItem(pytest.Item):
    def __init__(self, *, scope, subject=None, **kwargs):
        super().__init__(**kwargs)
        self.scope, self.subject = scope, subject
        # pytest-lsp's autoloaded call wrapper requires a fixture mapping even
        # on custom items; these items intentionally request no LSP fixtures.
        self.funcargs = {}

    def runtest(self):
        state = self.parent.state
        name = self.parent.name
        state.run(name)
        if name in state.failures:
            pytest.fail(state.failures[name], pytrace=False)
        if self.scope is None:
            expected = state.inventory["deferrals"][name]
            actual = state.deferrals.get(name)
            if actual != {key: expected[key] for key in ("stage", "error")}:
                pytest.fail("STALE OR CHANGED DEFERRAL: " + name, pytrace=False)
            pytest.xfail(expected["reason"])
        entries = state.by_subject.get((self.scope, self.subject), [])
        errors, reasons = [], []
        for key, finding in entries:
            observed = state.findings[key]
            known = state.ledger.get(key)
            self.user_properties.append(
                (
                    key,
                    json.dumps(
                        {"finding": finding, "accounting": known}, sort_keys=True
                    ),
                )
            )
            if known is None:
                errors.append("NEW: " + key + "\n" + json.dumps(finding, indent=2))
            elif any(known.get(field) != value for field, value in observed.items()):
                errors.append("CHANGED: " + key + "\n" + json.dumps(finding, indent=2))
            elif metadata_errors(key, known):
                errors.extend(metadata_errors(key, known))
            else:
                issue = f"#{known['issue']}" if known["issue"] else "NO ISSUE FILED"
                reasons.append(
                    f"{known['class']} [{known['triage']}, {issue}]: {known['reason']}"
                )
        if errors:
            pytest.fail("\n".join(errors), pytrace=False)
        if reasons:
            pytest.xfail("\n".join(reasons))

    def repr_failure(self, excinfo, style=None):
        return str(excinfo.value)

    def reportinfo(self):
        return self.path, 0, self.name


def metadata_errors(key, entry):
    errors = []
    for field in ("reason", "root_cause"):
        if not isinstance(entry.get(field), str) or not entry[field].strip():
            errors.append(f"MISSING {field.upper()}: {key}")
    if entry.get("triage") not in {"triaged", "untriaged"}:
        errors.append("MISSING TRIAGE STATUS: " + key)
    if "issue" not in entry or (
        entry["issue"] is not None
        and (type(entry["issue"]) is not int or entry["issue"] <= 0)
    ):
        errors.append("INVALID ISSUE: " + key)
    return errors


class FactSession:
    def __init__(self, config, inventory):
        self.config = config
        self.targets = read(config.getoption("facts_config"))
        self.inventory = read(inventory)
        self.ledger = read(config.getoption("facts_ledger"))
        self.output = config.getoption("facts_output")
        self.output.mkdir(parents=True, exist_ok=True)
        self.reports, self.runs, self.findings, self.by_subject = [], {}, {}, {}
        self.attempted, self.failures, self.deferrals = set(), {}, {}
        self.prepared = set()
        self.selected = []
        scheduled = [
            name for names in self.targets["shards"].values() for name in names
        ]
        if len(scheduled) != len(set(scheduled)) or set(scheduled) != set(
            self.targets["targets"]
        ):
            raise pytest.UsageError(
                "COVERAGE: each target must belong to exactly one shard"
            )
        corpus = {
            name
            for name, target in self.targets["targets"].items()
            if target["kind"] == "corpus"
        }
        if corpus != set(self.inventory["corpus_roster"]):
            raise pytest.UsageError("COVERAGE: corpus roster changed")
        runs = {scope.split("/")[0] for scope in self.inventory["runs"]}
        deferred = set(self.inventory["deferrals"])
        if runs & deferred or runs | deferred != set(scheduled):
            raise pytest.UsageError("COVERAGE: every target needs runs OR a deferral")
        for key in self.ledger:
            if key.split("|", 1)[0] not in self.inventory["runs"]:
                raise pytest.UsageError("STALE LEDGER SCOPE: " + key)
        if config.getoption("facts_required_shard"):
            workflow = (ROOT / ".github/workflows/test.yml").read_text()
            matrix = re.findall(r"^\s+shard: \[([^\]]+)\]$", workflow, re.MULTILINE)
            required = set(self.targets["required_shards"])
            if (
                len(matrix) != 1
                or [name.strip() for name in matrix[0].split(",")]
                != self.targets["required_shards"]
            ):
                raise pytest.UsageError(
                    "COVERAGE: PR matrix differs from required shards"
                )
            members = [
                name for shard in required for name in self.targets["shards"][shard]
            ]
            if len(members) != len(set(members)) or set(members) != {
                name
                for name, target in self.targets["targets"].items()
                if target.get("required")
            }:
                raise pytest.UsageError("COVERAGE: required target partition changed")
        for name, entry in self.inventory["deferrals"].items():
            if (
                metadata_errors(name, entry)
                or entry.get("stage") not in {"run", "prepare"}
                or not entry.get("error", "").strip()
            ):
                raise pytest.UsageError("INVALID DEFERRAL: " + name)

    def run(self, name):
        if name in self.attempted:
            return
        if not self.attempted:
            try:
                execute(
                    [
                        "cargo",
                        "build",
                        "-q",
                        "-p",
                        "djls-testing",
                        "--bin",
                        "django-oracle",
                    ]
                )
            except (OSError, subprocess.CalledProcessError) as error:
                pytest.exit(
                    "Cannot build live Django oracle: " + str(error), returncode=1
                )
        self.attempted.add(name)
        target = self.targets["targets"][name]
        stage = "prepare"
        try:
            if (
                self.config.getoption("facts_prepare")
                or self.inventory["deferrals"].get(name, {}).get("stage") == "prepare"
            ):
                environment_target = target.get("environment_target", name)
                if environment_target not in self.prepared:
                    prepare(
                        environment_target,
                        self.targets["targets"][environment_target],
                        self.targets["python"],
                    )
                    self.prepared.add(environment_target)
            stage = "run"
            path = self.output / (name + ".json")
            path.unlink(missing_ok=True)
            command = [
                str(ROOT / "target/debug/django-oracle"),
                "--report-only",
                "--json",
                str(path),
                "--markdown",
                str(self.output / (name + ".md")),
                "--settings-mode",
                target["mode"],
            ]
            for flag in ("include_django",):
                if target.get(flag):
                    command.append("--" + flag.replace("_", "-"))
            for flag in ("seeds", "backend"):
                if flag in target:
                    command.extend(
                        [
                            "--" + flag,
                            str(ROOT / target[flag])
                            if flag == "seeds"
                            else target[flag],
                        ]
                    )
            if target["kind"] == "corpus":
                command.extend(["corpus", name])
            else:
                command.extend(
                    [
                        "project",
                        "--root",
                        str(ROOT / target["root"]),
                        "--venv",
                        str(ROOT / target.get("venv", "target/oracle/venv")),
                        "--settings",
                        target["settings"],
                    ]
                )
                for entry in target.get("pythonpath", []):
                    command.extend(["--pythonpath", str(ROOT / entry)])
            result = execute(command)
            (self.output / (name + ".log")).write_text(result.stderr)
            reports = read(path)
            # A real project CLI names its root; CI identities are explicit targets.
            for report in reports:
                report["project"] = name
            if not reports:
                raise ValueError("no completed live scopes: " + name)
            runs, findings = observations(reports)
            self.reports.extend(reports)
            self.runs.update(runs)
            self.findings.update(findings)
            for report in reports:
                scope = "/".join([name, report["settings"], report["backend"]])
                for finding in report["findings"]:
                    self.by_subject.setdefault(
                        (scope, finding_subject(finding)), []
                    ).append((scope + "|" + finding["key"], finding))
        except subprocess.CalledProcessError as error:
            (self.output / (name + ".log")).write_text(error.stderr or str(error))
            self.deferrals[name] = failure(stage, error.stderr or str(error))
            if name not in self.inventory["deferrals"]:
                self.failures[name] = (
                    "UNDEFERRED FAILURE: " + name + "\n" + (error.stderr or str(error))
                )
        except (ValueError, OSError, KeyError) as error:
            self.failures[name] = str(error)

    def finish(self):
        errors = []
        selected = {item.parent.name for item in self.selected}
        required = self.config.getoption("facts_required_shard")
        if required:
            expected = set(self.targets["shards"][required])
            if selected != expected:
                errors.append(
                    "LOST SUBJECTS: required shard selection changed: "
                    + str(sorted(expected - selected))
                )
        prefixes = {name + "/" for name in selected}
        ledger = {
            key: value
            for key, value in self.ledger.items()
            if any(key.startswith(prefix) for prefix in prefixes)
        }
        expected_runs = {
            key: value
            for key, value in self.inventory["runs"].items()
            if any(key.startswith(prefix) for prefix in prefixes)
        }
        for scope in expected_runs.keys() - self.runs.keys():
            errors.append("LOST SUBJECTS: missing live scope " + scope)
        for scope in self.runs.keys() - expected_runs.keys():
            errors.append("NEW SCOPE: " + scope)
        for scope in self.runs.keys() & expected_runs.keys():
            actual, expected = self.runs[scope], expected_runs[scope]
            missing = set(expected["subjects"]) - set(actual["subjects"])
            added = set(actual["subjects"]) - set(expected["subjects"])
            if missing:
                errors.append(
                    f"LOST SUBJECTS: {scope}: {len(missing)}\n"
                    + "\n".join(sorted(missing))
                )
            if added:
                errors.append(
                    f"NEW SUBJECTS: {scope}: {len(added)}\n" + "\n".join(sorted(added))
                )
            if {k: v for k, v in actual.items() if k != "subjects"} != {
                k: v for k, v in expected.items() if k != "subjects"
            }:
                errors.append("ENVIRONMENT OR EXPLORATION CHANGED: " + scope)
        for key in ledger.keys() - self.findings.keys():
            errors.append("STALE LEDGER ENTRY: " + key)
        for key, entry in ledger.items():
            errors.extend(metadata_errors(key, entry))
        for key in self.findings.keys() - ledger.keys():
            errors.append("NEW: " + key)
        for key in self.findings.keys() & ledger.keys():
            if any(
                ledger[key].get(field) != value
                for field, value in self.findings[key].items()
            ):
                errors.append("CHANGED: " + key)
        for name in selected & self.inventory["deferrals"].keys():
            expected = self.inventory["deferrals"][name]
            if self.deferrals.get(name) != {
                key: expected[key] for key in ("stage", "error")
            }:
                errors.append("STALE OR CHANGED DEFERRAL: " + name)
        errors.extend(self.failures.values())
        if required:
            collected = {(item.scope, item.subject) for item in self.selected}
            for scope, run in expected_runs.items():
                if any(
                    (scope, name) not in collected
                    for identity in run["subjects"]
                    if (name := subject(identity)) is not None
                ):
                    errors.append(
                        "LOST SUBJECTS: pytest deselected required items in " + scope
                    )
        write(self.output / "report.json", self.reports)
        write(self.output / "deferrals.json", self.deferrals)
        if self.config.getoption("facts_update"):
            proposed = {
                key: {
                    **value,
                    **{
                        field: ledger.get(key, {}).get(field)
                        if ledger.get(key, {}).get("fingerprint")
                        == value["fingerprint"]
                        else (
                            None
                            if field == "issue"
                            else "untriaged"
                            if field == "triage"
                            else ""
                        )
                        for field in ("reason", "root_cause", "issue", "triage")
                    },
                }
                for key, value in self.findings.items()
            }
            write(self.output / "disagreements.proposed.json", proposed)
            write(
                self.output / "subjects.proposed.json",
                {**self.inventory, "runs": {**self.inventory["runs"], **self.runs}},
            )
        return sorted(set(errors))


@pytest.hookimpl(trylast=True)
def pytest_collection_modifyitems(config, items):
    if not config.getoption("django_facts"):
        # Explicit Python test paths bypass directory ignore hooks as well.
        excluded = [
            item for item in items if item.path.is_relative_to(ROOT / "tests/facts")
        ]
        if excluded:
            items[:] = [item for item in items if item not in excluded]
            config.hook.pytest_deselected(items=excluded)
    if STATE in config.stash:
        config.stash[STATE].selected = [
            item for item in items if isinstance(item, FactItem)
        ]


@pytest.hookimpl(tryfirst=True)
def pytest_sessionfinish(session, exitstatus):
    if session.config.option.collectonly:
        return
    if STATE in session.config.stash:
        state = session.config.stash[STATE]
        state.errors = errors = state.finish()
    elif session.config.getoption("facts_required_shard"):
        errors = ["LOST SUBJECTS: no Django facts inventory was collected"]
    else:
        return
    if errors:
        session.config.hook.pytest_runtest_logreport(
            report=pytest.TestReport(
                nodeid="django-facts::session-accounting",
                location=("tests/facts/django.facts.json", 0, "session-accounting"),
                keywords={},
                outcome="failed",
                longrepr="\n".join(errors),
                when="call",
                sections=[],
                duration=0,
            )
        )
        session.exitstatus = pytest.ExitCode.TESTS_FAILED


def pytest_terminal_summary(terminalreporter, exitstatus, config):
    if STATE not in config.stash:
        return
    state = config.stash[STATE]
    terminalreporter.section("Live Django facts")
    terminalreporter.write_line("target / mode: Wrong / Missed / Imprecise / Unchecked")
    for report in state.reports:
        counts = Counter(finding["class"] for finding in report["findings"])
        terminalreporter.write_line(
            report["project"]
            + "/"
            + report["settings"]
            + ": "
            + " / ".join(
                str(counts[key])
                for key in ("wrong", "missed", "imprecise", "unchecked")
            )
        )
    untriaged = sum(
        entry.get("triage") == "untriaged"
        for key, entry in state.ledger.items()
        if key in state.findings
    )
    terminalreporter.write_line(
        f"Ledger is not a reviewed truth set: {untriaged} observed entries remain untriaged."
    )
    terminalreporter.write_line(
        "Native evidence: "
        + str(state.output)
        + "; pytest results: terminal and --junitxml"
    )
    if getattr(state, "errors", []):
        terminalreporter.write_line(
            "Session accounting FAILED; see django-facts::session-accounting", red=True
        )
