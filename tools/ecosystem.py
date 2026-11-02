# /// script
# requires-python = ">=3.11"
# dependencies = ["typer>=0.27.0"]
# ///
"""Run `djls check` across the corpus and diff two runs.

`run` records every diagnostic one build reports for each corpus repository.
`diff` compares two recordings, usually the base branch and a pull request,
so a reviewer sees each diagnostic a change adds or removes on real projects.

    uv run tools/ecosystem.py run --djls target/release/djls --output head.json
    uv run tools/ecosystem.py diff base.json head.json
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import time
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Annotated

import tomllib
import typer

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST = REPO_ROOT / "crates/djls-testing/manifest.toml"
CORPUS_REPOS = REPO_ROOT / "crates/djls-testing/.corpus/repos"

HEADER = re.compile(r"^(error|warning|info|hint)\[(\w+)\]: (.*)$")
LOCATION = re.compile(r"^\s*--> (.*):(\d+):(\d+)$")


app = typer.Typer(help=__doc__, add_completion=False)


@app.command()
def run(
    djls: Annotated[Path, typer.Option(help="DJLS executable.")],
    output: Annotated[Path, typer.Option(help="Where to write the recording.")],
    repo: Annotated[
        list[str] | None, typer.Option(help="Manifest repository name; repeatable.")
    ] = None,
    timeout: Annotated[float, typer.Option(help="Seconds per repository.")] = 120,
    jobs: Annotated[int, typer.Option(help="Parallel checks.")] = os.cpu_count() or 1,
) -> None:
    """Record diagnostics for one build."""
    djls = djls.resolve()
    repos = selected_repos(repo or [])

    def check(entry: dict) -> dict:
        return check_repo(djls, entry, timeout)

    with ThreadPoolExecutor(max_workers=jobs) as pool:
        results = list(pool.map(check, repos))

    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        json.dumps({"repos": results}, indent=1, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    for result in results:
        typer.echo(
            f"{result['repo']}: {result['status']}, "
            f"{len(result['diagnostics'])} diagnostics, {result['seconds']}s",
            err=True,
        )


def selected_repos(names: list[str]) -> list[dict]:
    manifest = tomllib.loads(MANIFEST.read_text(encoding="utf-8"))
    repos = [repo for repo in manifest["repo"] if not names or repo["name"] in names]
    missing = sorted(set(names) - {repo["name"] for repo in repos})
    if missing:
        raise typer.BadParameter(f"unknown corpus repos: {', '.join(missing)}")
    return repos


def check_repo(djls: Path, repo: dict, timeout: float) -> dict:
    checkout = CORPUS_REPOS / repo["name"]
    project_root = checkout / repo.get("project_root", ".")
    result = {"repo": repo["name"], "diagnostics": [], "seconds": 0.0}
    if not project_root.is_dir():
        return result | {
            "status": "missing checkout; run `just corpus sync --source-only`"
        }

    env = os.environ.copy()
    env.pop("VIRTUAL_ENV", None)
    env.pop("DJANGO_SETTINGS_MODULE", None)
    settings_module = repo.get("django_settings_module") or next(
        iter(repo.get("django_settings_modules", [])), None
    )
    if settings_module:
        env["DJANGO_SETTINGS_MODULE"] = settings_module

    started = time.monotonic()
    try:
        process = subprocess.run(
            [str(djls), "check", "--color", "never"],
            cwd=project_root,
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return result | {
            "status": "timeout",
            "seconds": round(time.monotonic() - started, 1),
        }
    result["seconds"] = round(time.monotonic() - started, 1)

    result["diagnostics"] = parse_diagnostics(process.stdout, project_root)
    if "panicked" in process.stderr:
        result["status"] = "panic"
        result["stderr"] = process.stderr[-2000:]
    elif process.returncode in (0, 1):
        result["status"] = "ok"
    else:
        result["status"] = f"exit {process.returncode}"
        result["stderr"] = process.stderr[-2000:]
    return result


def parse_diagnostics(stdout: str, project_root: Path) -> list[str]:
    """Return one `path:line:col: CODE message` line per rendered diagnostic."""
    diagnostics = []
    header = None
    for line in stdout.splitlines():
        if match := HEADER.match(line):
            header = match
            continue
        if header and (match := LOCATION.match(line)):
            path, line_number, column = match.groups()
            path = Path(path)
            if path.is_absolute() and path.is_relative_to(project_root):
                path = path.relative_to(project_root)
            diagnostics.append(
                f"{path.as_posix()}:{line_number}:{column}: {header[2]} {header[3]}"
            )
            header = None
    return sorted(diagnostics)


@app.command()
def diff(
    base_path: Annotated[Path, typer.Argument(metavar="BASE", help="Base recording.")],
    head_path: Annotated[Path, typer.Argument(metavar="HEAD", help="Head recording.")],
    max_lines: Annotated[
        int, typer.Option(help="Diagnostic lines shown per repository.")
    ] = 20,
) -> None:
    """Compare two recordings as Markdown."""
    base = load(base_path)
    head = load(head_path)
    sections = []
    added_total = removed_total = 0
    added_codes: Counter[str] = Counter()
    removed_codes: Counter[str] = Counter()

    for name in sorted(base.keys() | head.keys()):
        before = base.get(name, {"status": "absent", "diagnostics": []})
        after = head.get(name, {"status": "absent", "diagnostics": []})
        added = sorted(Counter(after["diagnostics"]) - Counter(before["diagnostics"]))
        removed = sorted(Counter(before["diagnostics"]) - Counter(after["diagnostics"]))
        status_changed = before["status"] != after["status"]
        if not added and not removed and not status_changed:
            continue

        added_total += len(added)
        removed_total += len(removed)
        added_codes.update(code_of(line) for line in added)
        removed_codes.update(code_of(line) for line in removed)

        lines = [f"### {name} (+{len(added)} -{len(removed)})", ""]
        if status_changed:
            lines.append(f"Status: `{before['status']}` → `{after['status']}`")
            lines.append("")
        changes = [f"+ {line}" for line in added] + [f"- {line}" for line in removed]
        if changes:
            lines.append("```diff")
            lines.extend(changes[: max_lines])
            if len(changes) > max_lines:
                lines.append(f"... {len(changes) - max_lines} more")
            lines.append("```")
            lines.append("")
        sections.append("\n".join(lines))

    print("## Ecosystem diff")
    print()
    failing = sorted(name for name, repo in head.items() if repo["status"] != "ok")
    if failing:
        print(
            "Not checked on head: "
            + ", ".join(f"{n} ({head[n]['status']})" for n in failing)
        )
        print()
    if not sections:
        print(f"No changes across {len(head)} corpus repositories.")
        return
    print(
        f"{added_total} added and {removed_total} removed diagnostics "
        f"in {len(sections)} of {len(head)} corpus repositories."
    )
    print()
    codes = sorted(added_codes.keys() | removed_codes.keys())
    print("| Code | Added | Removed |")
    print("|---|---|---|")
    for code in codes:
        print(f"| {code} | {added_codes[code]} | {removed_codes[code]} |")
    print()
    print("\n".join(sections))


def load(path: Path) -> dict[str, dict]:
    data = json.loads(path.read_text(encoding="utf-8"))
    return {repo["repo"]: repo for repo in data["repos"]}


def code_of(line: str) -> str:
    return line.split(": ", 1)[1].split(" ", 1)[0]


if __name__ == "__main__":
    app()
