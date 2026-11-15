"""Exercise pytest's public results using real projects and native subprocesses.

Policy mutations edit the allowances or required work, never Django's answers.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import xml.etree.ElementTree as ET

import pytest

from tools import pytest_django_facts as facts

pytestmark = pytest.mark.facts_fixtures
INVENTORY = facts.ROOT / "tests/facts/django.facts.json"
TOOLCHAIN_HOME = os.environ["HOME"]


@pytest.fixture
def pytester(pytester, monkeypatch):
    # Pytester isolates HOME. These subprocesses use the real Cargo/mise toolchain
    # and its existing trust/configuration, not a fresh simulated user account.
    monkeypatch.setenv("HOME", TOOLCHAIN_HOME)
    return pytester


@pytest.mark.parametrize("selection", ["directory", "policy_file"])
def test_facts_collection_requires_opt_in(pytester, selection):
    paths = {
        "directory": facts.ROOT / "tests/facts",
        "policy_file": facts.ROOT / "tests/facts/test_plugin.py",
    }
    output = pytester.path / "live"
    result = pytester.runpytest_subprocess(
        str(paths[selection]), "--collect-only", "-q", "--facts-output", str(output)
    )
    assert result.ret == pytest.ExitCode.NO_TESTS_COLLECTED
    assert "django.facts.json::" not in result.stdout.str()
    assert (
        "::test_known_disagreements_are_xfails_with_evidence" not in result.stdout.str()
    )
    assert not output.exists()


@pytest.mark.parametrize("enabled", [False, True])
def test_required_shard_fails_without_facts_collection(pytester, enabled):
    junit = pytester.path / "junit.xml"
    result = pytester.runpytest_subprocess(
        str(facts.ROOT / "tests/facts/test_plugin.py"),
        *(["--django-facts"] if enabled else []),
        "-k",
        "no_such_test",
        "--facts-required-shard",
        "fixtures",
        "-q",
        "--junitxml",
        str(junit),
    )
    assert result.ret == pytest.ExitCode.TESTS_FAILED
    assert (
        "LOST SUBJECTS: no Django facts inventory was collected" in result.stdout.str()
    )
    assert any(
        "LOST SUBJECTS" in (failure.text or "")
        for failure in ET.parse(junit).findall(".//testcase/failure")
    )


def test_known_disagreements_are_xfails_with_evidence(pytester):
    junit = pytester.path / "junit.xml"
    result = pytester.runpytest_subprocess(
        str(INVENTORY),
        "--django-facts",
        "-m",
        "target_facts",
        "-q",
        "--facts-output",
        str(pytester.path / "live"),
        "--junitxml",
        str(junit),
    )
    assert result.ret == pytest.ExitCode.OK
    cases = ET.parse(junit).findall(".//testcase")
    assert any(
        case.find("skipped") is not None
        and "#1092" in case.find("skipped").get("message", "")
        for case in cases
    )
    assert any(
        "self_filter" in prop.get("value", "")
        for prop in ET.parse(junit).findall(".//property")
    )
    assert not ET.parse(junit).findall(".//failure")


@pytest.mark.parametrize("finding_class", ["wrong", "missed", "imprecise", "unchecked"])
def test_unlisted_disagreement_fails_and_proposes_blank_reason(pytester, finding_class):
    ledger = facts.read(facts.CONFIG / "disagreements.json")
    # Prefer the fast real fixtures; Imprecise may need a real corpus package.
    key = min(
        (key for key, entry in ledger.items() if entry["class"] == finding_class),
        key=lambda key: (
            not key.startswith(("facts/", "compilation/", "project/")),
            key,
        ),
    )
    target = key.split("/")[0]
    del ledger[key]
    path = pytester.path / "ledger.json"
    facts.write(path, ledger)
    output = pytester.path / "live"
    result = pytester.runpytest_subprocess(
        str(INVENTORY),
        "--django-facts",
        "-m",
        "target_" + target.replace("-", "_"),
        "-q",
        "--tb=short",
        "--facts-ledger",
        str(path),
        "--facts-update",
        "--facts-output",
        str(output),
        "--junitxml",
        str(output / "junit.xml"),
    )
    assert result.ret == pytest.ExitCode.TESTS_FAILED
    assert "NEW: " + key in result.stdout.str()
    proposed = facts.read(output / "disagreements.proposed.json")[key]
    assert proposed["reason"] == ""
    assert proposed["root_cause"] == ""
    assert proposed["issue"] is None
    assert ET.parse(output / "junit.xml").findall(".//failure")


def test_stale_allowance_fails_at_session_end(pytester):
    ledger = facts.read(facts.CONFIG / "disagreements.json")
    key = next(key for key in ledger if key.startswith("facts/"))
    stale = key + ":previous-observation"
    ledger[stale] = ledger[key]
    path = pytester.path / "ledger.json"
    facts.write(path, ledger)
    junit = pytester.path / "junit.xml"
    result = pytester.runpytest_subprocess(
        str(INVENTORY),
        "--django-facts",
        "-m",
        "target_facts",
        "-q",
        "--tb=short",
        "--facts-ledger",
        str(path),
        "--facts-output",
        str(pytester.path / "live"),
        "--junitxml",
        str(junit),
    )
    assert result.ret == pytest.ExitCode.TESTS_FAILED
    assert "STALE LEDGER ENTRY: " + stale in result.stdout.str()
    failures = ET.parse(junit).findall(".//testcase/failure")
    assert any("STALE LEDGER ENTRY" in (failure.text or "") for failure in failures)


def test_blank_accounting_cannot_pass(pytester):
    ledger = facts.read(facts.CONFIG / "disagreements.json")
    key = next(key for key in ledger if key.startswith("facts/"))
    ledger[key]["reason"] = " "
    del ledger[key]["issue"]
    path = pytester.path / "ledger.json"
    facts.write(path, ledger)
    result = pytester.runpytest_subprocess(
        str(INVENTORY),
        "--django-facts",
        "-m",
        "target_facts",
        "-q",
        "--tb=short",
        "--facts-ledger",
        str(path),
        "--facts-output",
        str(pytester.path / "live"),
    )
    assert result.ret == pytest.ExitCode.TESTS_FAILED
    assert "MISSING REASON: " + key in result.stdout.str()
    assert "INVALID ISSUE: " + key in result.stdout.str()


def test_real_failed_environment_deferral_is_retried_and_can_go_stale(pytester):
    broken = facts.ROOT / "tests/oracle-projects/broken"
    result = subprocess.run(
        [
            str(facts.ROOT / "target/debug/django-oracle"),
            "--report-only",
            "--settings-mode",
            "project",
            "project",
            "--root",
            str(broken),
            "--venv",
            str(facts.ROOT / "target/oracle/venv"),
            "--settings",
            "settings",
        ],
        cwd=facts.ROOT,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 1
    assert "module_does_not_exist" in result.stderr
    config = facts.read(facts.CONFIG / "targets.json")
    config["targets"]["facts"]["root"] = str(broken)
    inventory = facts.read(INVENTORY)
    del inventory["runs"]["facts/project/django"]
    inventory["deferrals"]["facts"] = {
        "stage": "run",
        "error": result.stderr.strip().splitlines()[-1],
        "reason": "This real fixture deliberately configures an unimportable library.",
        "root_cause": "Invalid configured library module",
        "issue": None,
        "triage": "triaged",
    }
    ledger = {
        key: value
        for key, value in facts.read(facts.CONFIG / "disagreements.json").items()
        if not key.startswith("facts/")
    }
    paths = {
        name: pytester.path / (name + ".json")
        for name in ("targets", "inventory", "ledger")
    }
    for name, value in (
        ("targets", config),
        ("inventory", inventory),
        ("ledger", ledger),
    ):
        facts.write(paths[name], value)
    args = [
        str(INVENTORY),
        "--django-facts",
        "-m",
        "target_facts",
        "-q",
        "--tb=short",
        "--facts-config",
        str(paths["targets"]),
        "--facts-inventory",
        str(paths["inventory"]),
        "--facts-ledger",
        str(paths["ledger"]),
        "--facts-output",
        str(pytester.path / "live"),
    ]
    run = pytester.runpytest_subprocess(*args)
    run.assert_outcomes(xfailed=1)
    assert run.ret == pytest.ExitCode.OK
    config["targets"]["facts"]["root"] = "tests/oracle-projects/facts"
    facts.write(paths["targets"], config)
    run = pytester.runpytest_subprocess(*args)
    assert run.ret == pytest.ExitCode.TESTS_FAILED
    assert "STALE OR CHANGED DEFERRAL: facts" in run.stdout.str()


def test_removing_a_real_compilation_seed_fails_coverage(pytester):
    project = pytester.path / "compilation"
    shutil.copytree(facts.ROOT / "tests/compilation", project)
    seeds = project / "templates"
    (seeds / "simple_block_valid.html").unlink()
    config = facts.read(facts.CONFIG / "targets.json")
    config["targets"]["compilation"]["root"] = str(project)
    config["targets"]["compilation"]["seeds"] = str(seeds)
    path = pytester.path / "targets.json"
    facts.write(path, config)
    result = pytester.runpytest_subprocess(
        str(INVENTORY),
        "--django-facts",
        "-m",
        "target_compilation",
        "-q",
        "--tb=short",
        "--facts-config",
        str(path),
        "--facts-output",
        str(pytester.path / "live"),
    )
    assert result.ret == pytest.ExitCode.TESTS_FAILED
    # The real file supplied both a compilation probe and a loader-name subject.
    assert "LOST SUBJECTS: compilation/project/django: 2" in result.stdout.str()
    assert "simple_block_valid.html" in result.stdout.str()


def native(project, **request):
    root = facts.ROOT / project
    result = subprocess.run(
        [
            str(facts.ROOT / "target/oracle/venv/bin/python"),
            "-I",
            str(facts.ROOT / "tools/django_oracle.py"),
        ],
        cwd=root,
        input=json.dumps(
            {"settings_module": "settings", "sys_path": [str(root)], **request}
        ),
        capture_output=True,
        text=True,
        check=True,
    )
    return json.loads(result.stdout)


def test_public_loader_origin_survives_invalid_source_and_custom_override():
    name = "templatetag_invalid_choice.html"
    result = native("tests/compilation", template_names=[name])
    assert result["templates"]["resolved"][name] == str(
        facts.ROOT / "tests/compilation/templates" / name
    )
    result = native("tests/oracle-projects/loaders", template_names=["shadow.html"])
    assert result["templates"]["resolved"]["shadow.html"] == "virtual://shadow.html"
    result = native("tests/oracle-projects/broken")
    assert "module_does_not_exist" in result["setup_error"]
    assert "libraries" not in result


def test_all_compilation_templates_receive_native_verdicts():
    seeds = sorted((facts.ROOT / "tests/compilation/templates").iterdir())
    assert len(seeds) == 194
    result = native(
        "tests/compilation", replay_templates=[path.read_text() for path in seeds]
    )
    assert result["setup_error"] is None
    assert set(result["native_probes"]) == {path.read_text() for path in seeds}
    assert all(
        verdict["django"] in {"compiled", "rejected"}
        for verdict in result["native_probes"].values()
    )
    assert (
        result["native_probes"][
            (
                facts.ROOT / "tests/compilation/templates/simple_block_valid.html"
            ).read_text()
        ]["django"]
        == "compiled"
    )
    assert (
        result["native_probes"][
            (
                facts.ROOT
                / "tests/compilation/templates/templatetag_invalid_choice.html"
            ).read_text()
        ]["django"]
        == "rejected"
    )
