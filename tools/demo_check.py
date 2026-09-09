# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Time fresh djls check processes against selected corpus repositories.

    just corpus sync
    cargo build --release -p djls
    uv run tools/demo_check.py healthchecks netbox pretix
    uv run tools/demo_check.py --list
    uv run tools/demo_check.py --all

Uses manifest project roots and settings modules, with normal DJLS config and
Python environment discovery. No dependencies are installed for the repositories.
Each check passes the repository directory as its input, even when the project
root is nested. DJLS applies its normal extension and ignore rules; test templates
may be included. Missing settings metadata leaves settings auto-detection to DJLS;
this is not a claim of complete project configuration or LSP readiness.

Times include process startup, discovery, analysis, diagnostic rendering and log
I/O. Building and corpus sync are separate. Each process has a fresh analysis
cache, but the OS filesystem cache is not cleared. DJLS does not report the total
files checked, so this script reports repository counts rather than guessing a
template count. Findings do not fail the demo; command failures do.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import tomllib

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "crates/djls-testing/manifest.toml"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "projects", nargs="*", help="repository names from the corpus manifest"
    )
    parser.add_argument(
        "--all", action="store_true", help="check every corpus repository"
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="list repository names and settings metadata",
    )
    parser.add_argument(
        "--binary",
        type=Path,
        default=ROOT / "target/release/djls",
        help="djls executable (default: target/release/djls)",
    )
    args = parser.parse_args(argv)
    if sum([bool(args.projects), args.all, args.list]) != 1:
        parser.error("choose project names, --all, or --list")

    with MANIFEST.open("rb") as source:
        manifest = tomllib.load(source)
    repos = {repo["name"]: repo for repo in manifest["repo"]}
    if args.list:
        for name, repo in repos.items():
            print(f"{name:28} {repo.get('django_settings_module', '(auto-detect)')}")
        return 0

    names = list(repos) if args.all else args.projects
    for name in names:
        if name not in repos:
            parser.error(f"unknown corpus repository {name!r}; use --list")
    if len(set(names)) != len(names):
        parser.error("specify each repository only once")

    binary = args.binary.resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error(
            f"executable not found: {binary}; run cargo build --release -p djls"
        )

    corpus = MANIFEST.parent / manifest["corpus"]["root_dir"] / "repos"
    for name in names:
        checkout = corpus / name
        project = checkout / repos[name].get("project_root", ".")
        if not (checkout / ".complete.json").is_file() or not project.is_dir():
            parser.error(
                f"{name}: corpus checkout or project root missing; run just corpus sync"
            )

    version = subprocess.run(
        [str(binary), "--version"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    logs = Path(tempfile.mkdtemp(prefix="djls-check-demo-"))
    width = max(len(name) for name in names)
    print(f"{version} | {len(names)} corpus repositories | sequential, fresh processes")
    print(f"Binary: {binary}")
    print("Timing includes discovery, analysis, and diagnostic output to logs.")
    print(
        "Settings come from corpus metadata or auto-detection; dependencies are not installed."
    )
    print(f"Logs: {logs}\n", flush=True)

    failed = 0
    started = time.perf_counter()
    for name in names:
        repo = repos[name]
        checkout = corpus / name
        project = checkout / repo.get("project_root", ".")
        env = os.environ.copy()
        # A settings module inherited from the caller must not leak between repos.
        env.pop("DJANGO_SETTINGS_MODULE", None)
        if settings := repo.get("django_settings_module"):
            env["DJANGO_SETTINGS_MODULE"] = settings
        command = [str(binary), "check", "--color", "never", str(checkout)]
        print(f"  {name:<{width}}  ", end="", flush=True)
        with (logs / f"{name}.log").open("w", encoding="utf-8") as log:
            log.write(f"cwd: {project}\nsettings: {settings or '(auto-detect)'}\n\n")
            log.flush()
            before = time.perf_counter()
            result = subprocess.run(
                command,
                cwd=project,
                env=env,
                stdout=log,
                stderr=subprocess.PIPE,
                text=True,
                check=False,
            )
            elapsed = time.perf_counter() - before
            log.write(result.stderr)
        # Exit 1 also covers infrastructure failures. Only the CLI's full summary
        # identifies a completed check with findings; anything else stays a failure.
        findings = re.fullmatch(
            r"Found (\d+) errors? in \d+ files?\.", result.stderr.strip()
        )
        if result.returncode == 0:
            status = "no diagnostics"
        elif result.returncode == 1 and findings:
            status = f"{int(findings[1]):,} diagnostics"
        else:
            failed += 1
            status = f"FAILED (exit {result.returncode}; see log)"
        print(f"{elapsed:8.3f}s  {status}", flush=True)

    elapsed = time.perf_counter() - started
    print(
        f"\n{len(names) - failed}/{len(names)} checks completed in {elapsed:.3f}s wall time."
    )
    if failed:
        print(f"{failed} failed; inspect logs in {logs} before using these timings.")
    return int(failed > 0)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
    except (OSError, subprocess.CalledProcessError) as error:
        print(f"Demo failed: {error}", file=sys.stderr)
        sys.exit(1)
