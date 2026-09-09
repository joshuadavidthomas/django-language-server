from __future__ import annotations

import subprocess
import sys
from unittest.mock import Mock

import demo_check
import pytest


@pytest.fixture
def corpus(tmp_path, monkeypatch):
    manifest = tmp_path / "manifest.toml"
    manifest.write_text(
        '[corpus]\nroot_dir = ".corpus"\n'
        '[[repo]]\nname = "nested"\nproject_root = "src"\n'
        'django_settings_module = "app.settings"\n'
        '[[repo]]\nname = "auto"\n',
        encoding="utf-8",
    )
    repos = tmp_path / ".corpus/repos"
    (repos / "nested/src").mkdir(parents=True)
    (repos / "auto").mkdir()
    for name in ("nested", "auto"):
        (repos / name / ".complete.json").write_text("{}", encoding="utf-8")
    logs = tmp_path / "logs"
    logs.mkdir()
    monkeypatch.setattr(demo_check, "MANIFEST", manifest)
    monkeypatch.setattr(demo_check.tempfile, "mkdtemp", lambda **kwargs: str(logs))
    monkeypatch.setenv("DJANGO_SETTINGS_MODULE", "caller.settings")
    monkeypatch.setenv("VIRTUAL_ENV", "/caller/venv")
    return repos, logs


def test_selected_projects_preserve_order_roots_and_settings(
    corpus, monkeypatch, capsys
):
    repos, logs = corpus
    run = Mock(
        side_effect=[
            subprocess.CompletedProcess([], 0, stdout="djls test\n"),
            subprocess.CompletedProcess([], 1, stderr="Found 2 errors in 1 file.\n"),
            subprocess.CompletedProcess([], 0, stderr=""),
        ]
    )
    monkeypatch.setattr(demo_check.subprocess, "run", run)

    assert demo_check.main(["--binary", sys.executable, "nested", "auto"]) == 0

    nested = run.call_args_list[1]
    auto = run.call_args_list[2]
    assert nested.args[0][1:] == ["check", "--color", "never", str(repos / "nested")]
    assert nested.kwargs["cwd"] == repos / "nested/src"
    assert nested.kwargs["env"]["DJANGO_SETTINGS_MODULE"] == "app.settings"
    assert auto.kwargs["cwd"] == repos / "auto"
    assert "DJANGO_SETTINGS_MODULE" not in auto.kwargs["env"]
    assert auto.kwargs["env"]["VIRTUAL_ENV"] == "/caller/venv"
    assert "Found 2 errors in 1 file." in (logs / "nested.log").read_text()
    assert "settings: (auto-detect)" in (logs / "auto.log").read_text()
    output = capsys.readouterr().out
    assert "2 diagnostics" in output
    assert "no diagnostics" in output
    assert "2/2 checks completed" in output


@pytest.mark.parametrize(
    "returncode, stderr",
    [
        (1, "Failed to discover Django environment\n"),
        (1, ""),
        (1, "Something failed\nFound 2 errors in 1 file.\n"),
        (-9, ""),
        (2, "invalid arguments\n"),
    ],
)
def test_failed_check_is_reported_and_next_repository_runs(
    corpus,
    monkeypatch,
    capsys,
    returncode,
    stderr,
):
    run = Mock(
        side_effect=[
            subprocess.CompletedProcess([], 0, stdout="djls test\n"),
            subprocess.CompletedProcess([], returncode, stderr=stderr),
            subprocess.CompletedProcess([], 0, stderr=""),
        ]
    )
    monkeypatch.setattr(demo_check.subprocess, "run", run)

    assert demo_check.main(["--binary", sys.executable, "--all"]) == 1

    assert run.call_count == 3
    output = capsys.readouterr().out
    assert "FAILED" in output
    assert "1/2 checks completed" in output
    assert stderr in (corpus[1] / "nested.log").read_text()


@pytest.mark.parametrize(
    "projects", [[], ["unknown"], ["auto", "auto"], ["auto", "--all"]]
)
def test_invalid_selection_fails_before_starting_checks(corpus, monkeypatch, projects):
    run = Mock()
    monkeypatch.setattr(demo_check.subprocess, "run", run)

    with pytest.raises(SystemExit) as error:
        demo_check.main(["--binary", sys.executable, *projects])

    assert error.value.code == 2
    run.assert_not_called()


def test_missing_checkout_fails_before_any_checks(corpus, monkeypatch):
    (corpus[0] / "auto/.complete.json").unlink()
    run = Mock()
    monkeypatch.setattr(demo_check.subprocess, "run", run)

    with pytest.raises(SystemExit) as error:
        demo_check.main(["--binary", sys.executable, "--all"])

    assert error.value.code == 2
    run.assert_not_called()


def test_list_does_not_need_binary_or_synced_corpus(corpus, monkeypatch, capsys):
    (corpus[0] / "auto/.complete.json").unlink()
    run = Mock()
    monkeypatch.setattr(demo_check.subprocess, "run", run)

    assert demo_check.main(["--binary", "/missing/djls", "--list"]) == 0

    output = capsys.readouterr().out
    assert "nested" in output
    assert "app.settings" in output
    assert "(auto-detect)" in output
    run.assert_not_called()
