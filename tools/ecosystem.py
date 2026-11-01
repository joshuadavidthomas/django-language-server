"""Run `djls check` across the corpus and diff two runs.

`run` records every diagnostic one build reports for each corpus repository.
`diff` compares two recordings, usually the base branch and a pull request,
so a reviewer sees each diagnostic a change adds or removes on real projects.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import tomllib

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST = REPO_ROOT / "crates/djls-testing/manifest.toml"
CORPUS_REPOS = REPO_ROOT / "crates/djls-testing/.corpus/repos"

HEADER = re.compile(r"^(error|warning|info|hint)\[(\w+)\]: (.*)$")
LOCATION = re.compile(r"^\s*--> (.*):(\d+):(\d+)$")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)

    run_parser = commands.add_parser("run", help="record diagnostics for one build")
    run_parser.add_argument("--djls", type=Path, required=True)
    run_parser.add_argument("--output", type=Path, required=True)
    run_parser.add_argument("--repo", action="append", default=[])
    run_parser.add_argument("--timeout", type=float, default=120)
    run_parser.add_argument("--jobs", type=int, default=os.cpu_count() or 1)

    diff_parser = commands.add_parser("diff", help="compare two recordings")
    diff_parser.add_argument("base", type=Path)
    diff_parser.add_argument("head", type=Path)
    diff_parser.add_argument("--max-lines", type=int, default=20)

    args = parser.parse_args()
    if args.command == "run":
        return run(args)
    return diff(args)


def run(args: argparse.Namespace) -> int:
    djls = args.djls.resolve()
    repos = selected_repos(args.repo)

    def check(repo: dict) -> dict:
        return check_repo(djls, repo, args.timeout)

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        results = list(pool.map(check, repos))

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps({"repos": results}, indent=1, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    for result in results:
        print(
            f"{result['repo']}: {result['status']}, "
            f"{len(result['diagnostics'])} diagnostics, {result['seconds']}s",
            file=sys.stderr,
        )
    return 0


def selected_repos(names: list[str]) -> list[dict]:
    manifest = tomllib.loads(MANIFEST.read_text(encoding="utf-8"))
    repos = [repo for repo in manifest["repo"] if not names or repo["name"] in names]
    missing = sorted(set(names) - {repo["name"] for repo in repos})
    if missing:
        sys.exit(f"unknown corpus repos: {', '.join(missing)}")
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


def diff(args: argparse.Namespace) -> int:
    base = load(args.base)
    head = load(args.head)
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
            lines.extend(changes[: args.max_lines])
            if len(changes) > args.max_lines:
                lines.append(f"... {len(changes) - args.max_lines} more")
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
        return 0
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
    return 0


def load(path: Path) -> dict[str, dict]:
    data = json.loads(path.read_text(encoding="utf-8"))
    return {repo["repo"]: repo for repo in data["repos"]}


def code_of(line: str) -> str:
    return line.split(": ", 1)[1].split(" ", 1)[0]


if __name__ == "__main__":
    sys.exit(main())
