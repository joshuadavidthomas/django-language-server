from __future__ import annotations

import json
import os
import re
import shutil
import tempfile
from pathlib import Path

import nox
from nox.command import CommandFailed

nox.options.default_venv_backend = "uv|virtualenv"
nox.options.reuse_existing_virtualenvs = True

PY310 = "3.10"
PY311 = "3.11"
PY312 = "3.12"
PY313 = "3.13"
PY314 = "3.14"
PY_VERSIONS = [PY310, PY311, PY312, PY313, PY314]
PY_DEFAULT = PY_VERSIONS[0]
PY_LATEST = PY_VERSIONS[-1]

DJ42 = "4.2"
DJ51 = "5.1"
DJ52 = "5.2"
DJ60 = "6.0"
DJMAIN = "main"
DJMAIN_MIN_PY = PY312
DJ_VERSIONS = [DJ42, DJ51, DJ52, DJ60, DJMAIN]
DJ_LTS = [
    version for version in DJ_VERSIONS if version.endswith(".2") and version != DJMAIN
]
DJ_DEFAULT = DJ_LTS[0]
DJ_LATEST = DJ_VERSIONS[-2]


def version(ver: str) -> tuple[int, ...]:
    """Convert a string version to a tuple of ints, e.g. "3.10" -> (3, 10)"""
    return tuple(map(int, ver.split(".")))


def display_version(raw: str) -> str:
    match = re.match(r"\d+(?:\.\d+)?", raw)
    return match.group(0) if match else raw


def should_skip(python: str, django: str) -> bool:
    """Return True if the test should be skipped"""

    if django == DJMAIN and version(python) < version(DJMAIN_MIN_PY):
        # Django main requires Python 3.10+
        return True

    if django == DJ60 and version(python) < version(PY312):
        # Django main requires Python 3.12+
        return True

    if django == DJ52 and version(python) < version(PY310):
        # Django 5.2 requires Python 3.10+
        return True

    if django == DJ51 and version(python) < version(PY310):
        # Django 5.1 requires Python 3.10+
        return True

    return False


@nox.session
def test(session):
    session.notify(f"tests(python='{PY_DEFAULT}', django='{DJ_DEFAULT}')")


@nox.session
@nox.parametrize(
    "python,django",
    [
        (python, django)
        for python in PY_VERSIONS
        for django in DJ_VERSIONS
        if not should_skip(python, django)
    ],
)
def tests(session, django):
    session.run_install(
        "uv",
        "sync",
        "--frozen",
        "--inexact",
        "--no-install-package",
        "django",
        "--python",
        session.python,
        env={"UV_PROJECT_ENVIRONMENT": session.virtualenv.location},
    )

    if django == DJMAIN:
        session.install(
            "django @ https://github.com/django/django/archive/refs/heads/main.zip"
        )
    else:
        session.install(f"django=={django}")

    command = ["cargo", "test"]

    if session.posargs:
        args = []
        for arg in session.posargs:
            if arg:
                args.extend(arg.split(" "))
        command.extend(args)
    session.run(*command, external=True)


@nox.session
def lint(session):
    for python_version in reversed(PY_VERSIONS):
        with tempfile.TemporaryFile(mode="w+") as output_file:
            try:
                session.run(
                    "uvx",
                    "--python",
                    python_version,
                    "prek",
                    "run",
                    "--all-files",
                    "--show-diff-on-failure",
                    "--color",
                    "always",
                    stdout=output_file,
                    stderr=output_file,
                )
                output_file.seek(0)
                output = output_file.read().rstrip("\n")
                if output:
                    print(output)
                break
            except CommandFailed as e:
                # Parse exit code from exception reason: "Returned code X"
                match = re.search(r"Returned code (\d+)", e.reason or "")
                exit_code = int(match.group(1)) if match else None

                # Only retry on exit code 3 (infrastructure error)
                if exit_code == 3:
                    session.log(
                        f"Linting with Python {python_version} failed due to pre-commit infrastructure issue (exit code 3), trying next version"
                    )
                    continue
                else:
                    # Real lint failure (exit code 1) or unknown error - re-raise
                    output_file.seek(0)
                    error_output = output_file.read().rstrip("\n")
                    if error_output:
                        print(error_output)
                    raise
    else:
        session.error("Linting failed with all Python versions")


@nox.session
@nox.parametrize("django", [DJ42, DJ51, DJ52, DJ60, DJMAIN])
def analyze_tags(session, django):
    """Analyze Django template tags and generate TagSpec suggestions."""
    if django == DJMAIN:
        session.install(
            "django @ https://github.com/django/django/archive/refs/heads/main.zip"
        )
    else:
        session.install(f"django=={django}")

    session.run(
        "python",
        "scripts/analyze_django_tags.py",
        "--version",
        django,
        "--output-dir",
        "analysis",
    )


@nox.session
def copy_bench_fixtures(session):
    django_version = (
        session.posargs[0] if session.posargs and session.posargs[0] else DJ_LTS[-1]
    )

    if django_version == DJMAIN:
        session.install(
            "django @ https://github.com/django/django/archive/refs/heads/main.zip"
        )
    else:
        session.install(f"django=={django_version}")

    django_path = session.run(
        "python",
        "-c",
        "import django, os; print(os.path.dirname(django.__file__))",
        silent=True,
    ).strip()

    dest_base = Path("crates/djls-bench/fixtures/django")

    if dest_base.exists():
        shutil.rmtree(dest_base)

    templates = {
        "small/forms_widgets_input.html": "forms/templates/django/forms/widgets/input.html",
        "medium/admin_login.html": "contrib/admin/templates/admin/login.html",
        "large/views_technical_500.html": "views/templates/technical_500.html",
    }

    for dest, src in templates.items():
        dest_path = dest_base / dest
        dest_path.parent.mkdir(parents=True, exist_ok=True)
        src_path = Path(django_path) / src
        shutil.copy2(src_path, dest_path)


@nox.session
def gha_matrix(session):
    os_args = session.posargs[0] if session.posargs else ""
    os_list = [os.strip() for os in os_args.split(",") if os_args.strip()] or [
        "ubuntu-latest"
    ]

    sessions = session.run("nox", "-l", "--json", external=True, silent=True)
    versions_list = [
        {
            "django-version": session["call_spec"]["django"],
            "python-version": session["python"],
        }
        for session in json.loads(sessions)
        if session["name"] == "tests"
    ]

    # Build the matrix, excluding Python 3.9 on macOS (PyO3 linking issues)
    include_list = []
    for os_name in os_list:
        for combo in versions_list:
            include_list.append({**combo, "os": os_name})

    matrix = {"include": include_list}

    if os.environ.get("GITHUB_OUTPUT"):
        with Path(os.environ["GITHUB_OUTPUT"]).open("a") as fh:
            print(f"matrix={matrix}", file=fh)
    else:
        print(matrix)


@nox.session
def cog(session):
    COG_FILES = [
        "CONTRIBUTING.md",
        "README.md",
        "pyproject.toml",
    ]
    session.run(
        "uv",
        "run",
        "--with",
        "bumpver",
        "--with",
        "cogapp",
        "--with",
        "nox",
        "cog",
        "-r",
        *COG_FILES,
    )


@nox.session
def update_changelog(session):
    version = get_version(session)

    with open("CHANGELOG.md", "r") as f:
        changelog = f.read()

    changelog = changelog.replace("## [Unreleased]", f"## [{version}]", 1)
    changelog = changelog.replace(
        f"## [{version}]", f"## [Unreleased]\n\n## [{version}]"
    )

    repo_url = session.run("git", "remote", "get-url", "origin", silent=True).strip()
    repo_url = repo_url.replace(".git", "")

    changelog += f"\n[{version}]: {repo_url}/releases/tag/v{version}"
    changelog = re.sub(
        r"\[unreleased\]: .+",
        f"[unreleased]: {repo_url}/compare/v{version}...HEAD",
        changelog,
    )

    with open("CHANGELOG.md", "w") as f:
        f.write(changelog)

    session.run("git", "add", "CHANGELOG.md", external=True)
    session.run(
        "git",
        "commit",
        "-m",
        f"update CHANGELOG for version {version}",
        external=True,
        silent=True,
    )


@nox.session
def update_uvlock(session):
    version = get_version(session)

    session.run("uv", "lock")

    git_status = session.run("git", "status", "--porcelain", external=True, silent=True)
    if "uv.lock" not in git_status:
        session.log("No changes to uv.lock, skipping commit")
        return

    session.run("git", "add", "uv.lock", external=True)
    session.run(
        "git",
        "commit",
        "-m",
        f"update uv.lock for version {version}",
        external=True,
        silent=True,
    )


@nox.session(requires=["cog", "update_changelog", "update_uvlock"])
def release(session):
    version = get_version(session)
    session.run("git", "checkout", "-b", f"release/v{version}")
    command = ["uv", "run", "bumpver", "update"]
    if session.posargs:
        args = []
        for arg in session.posargs:
            if arg:
                args.extend(arg.split(" "))
        command.extend(args)
    session.run(*command)
    session.run("gh", "pr", "create", "--fill", "--head")


def get_version(session):
    from bumpver.version import to_pep440

    command = ["uv", "run", "bumpver", "update", "--dry", "--no-fetch"]
    if session.posargs:
        args = []
        for arg in session.posargs:
            if arg and arg not in ["--dry", "--no-fetch"]:
                args.extend(arg.split(" "))
        command.extend(args)
    output = session.run(*command, silent=True)
    match = re.search(r"New Version: (.+)", output)
    return to_pep440(match.group(1)) if match else None
