# /// script
# requires-python = ">=3.11"
# dependencies = ["typer>=0.27.0", "rich>=14.0.0"]
# ///
"""Time fresh djls check processes against corpus repositories.

    just corpus sync
    cargo build --release -p djls
    uv run tools/demo_check.py check healthchecks netbox pretix

Timing includes startup, discovery, analysis, diagnostic rendering, and log I/O;
building and corpus sync are separate. Each process has a fresh analysis cache,
but the OS filesystem cache is not cleared. Repository dependencies are not
installed; settings come from the manifest or auto-detection, not a guarantee of
complete project configuration. Findings do not fail the demo; command failures do.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Annotated

import tomllib
import typer
from rich.console import Console
from rich.table import Table

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "crates/djls-testing/manifest.toml"
DEFAULT_BINARY = ROOT / "target/release/djls"
console = Console(markup=False, highlight=False, soft_wrap=True)
app = typer.Typer(help=__doc__, add_completion=False)


@dataclass(frozen=True)
class Repository:
    name: str
    checkout: Path
    project: Path
    settings: str | None


@dataclass(frozen=True)
class Clean:
    pass


@dataclass(frozen=True)
class Findings:
    count: int


@dataclass(frozen=True)
class Failure:
    exit_code: int


def load_repositories() -> dict[str, Repository]:
    with MANIFEST.open("rb") as source:
        manifest = tomllib.load(source)
    corpus = MANIFEST.parent / manifest["corpus"]["root_dir"] / "repos"
    return {
        entry["name"]: Repository(
            entry["name"],
            corpus / entry["name"],
            corpus / entry["name"] / entry.get("project_root", "."),
            entry.get("django_settings_module"),
        )
        for entry in manifest["repo"]
    }


@app.command("list")
def list_repositories() -> None:
    """List manifest repositories, project roots, and settings modules."""
    table = Table("Repository", "Project root", "Settings module")
    for column in table.columns:
        column.overflow = "fold"
    for repo in load_repositories().values():
        table.add_row(
            repo.name,
            str(repo.project.relative_to(repo.checkout)),
            repo.settings or "auto-detect",
        )
    console.print(table)


@app.command()
def check(
    names: Annotated[
        list[str] | None, typer.Argument(help="Manifest repository names.")
    ] = None,
    all_repos: Annotated[
        bool, typer.Option("--all", help="Check every repository.")
    ] = False,
    binary: Annotated[Path, typer.Option(help="DJLS executable.")] = DEFAULT_BINARY,
) -> None:
    """Check selected repositories, or use --all.

    Timing includes startup, discovery, analysis, diagnostic rendering, and log
    I/O. Building and corpus sync are separate. Processes have fresh analysis
    caches; the OS filesystem cache is not cleared. Repository dependencies are
    not installed, and settings may be auto-detected. This does not establish
    complete project configuration. Findings do not fail the demo; failures do.
    """
    if bool(names) == all_repos:
        raise typer.BadParameter("choose repository names or --all, but not both")
    repos = load_repositories()
    selected = list(repos) if all_repos else names or []
    for name in selected:
        if name not in repos:
            raise typer.BadParameter(
                f"unknown corpus repository {name!r}; run the list command"
            )
    if len(set(selected)) != len(selected):
        raise typer.BadParameter("specify each repository only once")
    binary = binary.resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise typer.BadParameter(
            f"executable not found: {binary}; run cargo build --release -p djls"
        )
    for name in selected:
        repo = repos[name]
        if (
            not (repo.checkout / ".complete.json").is_file()
            or not repo.project.is_dir()
        ):
            raise typer.BadParameter(
                f"{name}: corpus checkout or project root missing; run just corpus sync"
            )

    version = subprocess.run(
        [str(binary), "--version"], capture_output=True, text=True, check=True
    ).stdout.strip()
    logs = Path(tempfile.mkdtemp(prefix="djls-check-demo-"))
    console.print(
        f"{version} | {len(selected)} repositories | sequential, fresh processes"
    )
    console.print(f"Binary: {binary}\nLogs: {logs}")
    console.print("Timing includes discovery, analysis, and diagnostic output to logs.")
    console.print(
        "Settings come from metadata or auto-detection; dependencies are not installed."
    )
    width = max(map(len, selected))
    failed = 0
    started = time.perf_counter()
    for name in selected:
        repo = repos[name]
        env = os.environ.copy()
        # A settings module inherited from the caller must not leak between repos.
        env.pop("DJANGO_SETTINGS_MODULE", None)
        if repo.settings:
            env["DJANGO_SETTINGS_MODULE"] = repo.settings
        with (logs / f"{name}.log").open("w", encoding="utf-8") as log:
            log.write(
                f"cwd: {repo.project}\nsettings: {repo.settings or '(auto-detect)'}\n\n"
            )
            log.flush()
            before = time.perf_counter()
            process = subprocess.run(
                [str(binary), "check", "--color", "never", str(repo.checkout)],
                cwd=repo.project,
                env=env,
                stdout=log,
                stderr=subprocess.PIPE,
                text=True,
                check=False,
            )
            elapsed = time.perf_counter() - before
            log.write(process.stderr)
        # Exit 1 also covers infrastructure failures. Only the CLI's full summary
        # identifies a completed check with findings; anything else stays a failure.
        findings = re.fullmatch(
            r"Found (\d+) errors? in \d+ files?\.", process.stderr.strip()
        )
        outcome: Clean | Findings | Failure
        if process.returncode == 0:
            outcome = Clean()
        elif process.returncode == 1 and findings:
            outcome = Findings(int(findings[1]))
        else:
            outcome = Failure(process.returncode)
        match outcome:
            case Clean():
                status, style = "no diagnostics", "green"
            case Findings(count):
                status, style = f"{count:,} diagnostics", "yellow"
            case Failure(exit_code):
                failed += 1
                status, style = f"FAILED (exit {exit_code}; see log)", "red"
        console.print(f"  {name:<{width}}  {elapsed:8.3f}s  {status}", style=style)

    wall_time = time.perf_counter() - started
    console.print(
        f"\n{len(selected) - failed}/{len(selected)} checks completed in {wall_time:.3f}s wall time."
    )
    if failed:
        console.print(
            f"{failed} failed; inspect logs in {logs} before using these timings.",
            style="yellow",
        )
    raise typer.Exit(int(failed > 0))


if __name__ == "__main__":
    try:
        app()
    except (OSError, subprocess.CalledProcessError) as error:
        console.print(f"Demo failed: {error}", style="red")
        sys.exit(1)
