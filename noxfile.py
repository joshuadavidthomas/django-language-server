from __future__ import annotations

import json
import os
import re
from pathlib import Path

import nox

nox.options.default_venv_backend = "uv|virtualenv"
nox.options.reuse_existing_virtualenvs = True

PY39 = "3.9"
PY310 = "3.10"
PY311 = "3.11"
PY312 = "3.12"
PY313 = "3.13"
PY_VERSIONS = [PY39, PY310, PY311, PY312, PY313]
PY_DEFAULT = PY_VERSIONS[0]
PY_LATEST = PY_VERSIONS[-1]

DJ42 = "4.2"
DJ51 = "5.1"
DJ52 = "5.2"
DJ60 = "6.0a0"
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
    session.run(
        "uv",
        "run",
        "--no-project",
        "--with",
        "pre-commit-uv",
        "--python",
        PY_LATEST,
        "pre-commit",
        "run",
        "--all-files",
        "--show-diff-on-failure",
        "--color",
        "always",
    )


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
    git_status = session.run("git", "status", "--porcelain", external=True, silent=True)
    if not any(cog_file in git_status for cog_file in COG_FILES):
        session.log("No changes to documentation files, skipping commit")
        return
    session.run("git", "add", *COG_FILES, external=True)
    session.run(
        "git",
        "commit",
        "-m",
        "auto-regenerate docs using cog",
        external=True,
        silent=True,
    )


@nox.session
def process_docs(session):
    session.run("uv", "run", "docs/processor.py")
    session.run("git", "add", "docs/", external=True)
    session.run(
        "git",
        "commit",
        "-m",
        "process docs from GHFM to mkdocs-style",
        external=True,
        silent=True,
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


@nox.session(requires=["cog", "process_docs", "update_changelog", "update_uvlock"])
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
