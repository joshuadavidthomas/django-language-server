from __future__ import annotations

import argparse
import json
import re
import subprocess
import tempfile
from dataclasses import dataclass
from datetime import UTC
from datetime import datetime
from datetime import timedelta
from pathlib import Path
from urllib import request


@dataclass(frozen=True)
class RepoMeta:
    full_name: str
    stars: int
    pushed_at: datetime
    default_branch: str


@dataclass(frozen=True)
class RepoCounts:
    commit: str
    templatetags_py_files: int
    templates_files: int
    tag_sites: int
    filter_sites: int


TAG_PATTERNS: tuple[re.Pattern[str], ...] = (
    re.compile(r"@register\.simple_tag\b"),
    re.compile(r"@register\.inclusion_tag\b"),
    re.compile(r"@register\.tag\b"),
    re.compile(r"\bregister\.simple_tag\("),
    re.compile(r"\bregister\.inclusion_tag\("),
    re.compile(r"\bregister\.tag\("),
)
FILTER_PATTERNS: tuple[re.Pattern[str], ...] = (
    re.compile(r"@register\.filter\b"),
    re.compile(r"\bregister\.filter\("),
)


def main() -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Discover high-signal Django projects by counting custom template tags/filters "
            "under templatetags/ and ranking by freshness + stars."
        )
    )
    parser.add_argument(
        "repos",
        nargs="*",
        help="GitHub repos in owner/name form (e.g. getsentry/sentry).",
    )
    parser.add_argument(
        "--min-stars",
        type=int,
        default=1000,
        help="Minimum GitHub stars required to include a repo in results.",
    )
    parser.add_argument(
        "--max-age-days",
        type=int,
        default=365,
        help="Maximum age (days since last push) required to include a repo in results.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit JSON instead of a human-readable table.",
    )
    args = parser.parse_args()

    if not args.repos:
        raise SystemExit("Provide one or more repos like getsentry/sentry")

    now = datetime.now(tz=UTC)
    cutoff = now - timedelta(days=args.max_age_days)

    rows: list[dict[str, object]] = []
    for repo in args.repos:
        meta = _github_repo_meta(repo)
        if meta is None:
            continue
        if meta.stars < args.min_stars:
            continue
        if meta.pushed_at < cutoff:
            continue

        counts = _count_repo(repo)
        rows.append(
            {
                "repo": meta.full_name,
                "stars": meta.stars,
                "pushed_at": meta.pushed_at.isoformat(),
                "commit": counts.commit,
                "templatetags_py_files": counts.templatetags_py_files,
                "templates_files": counts.templates_files,
                "tag_sites": counts.tag_sites,
                "filter_sites": counts.filter_sites,
            }
        )

    rows.sort(
        key=lambda r: (
            int(r["tag_sites"]),
            int(r["filter_sites"]),
            int(r["stars"]),
        ),
        reverse=True,
    )

    if args.json:
        print(json.dumps(rows, indent=2, sort_keys=True))
        return 0

    _print_table(rows)
    return 0


def _github_repo_meta(full_name: str) -> RepoMeta | None:
    url = f"https://api.github.com/repos/{full_name}"
    req = request.Request(
        url,
        headers={
            "Accept": "application/vnd.github+json",
            "User-Agent": "template-linter-corpus-discovery",
        },
    )
    try:
        with request.urlopen(req, timeout=30) as resp:
            data = json.load(resp)
    except Exception:
        return None

    pushed_at = data.get("pushed_at")
    if not pushed_at:
        return None

    return RepoMeta(
        full_name=data["full_name"],
        stars=int(data.get("stargazers_count") or 0),
        pushed_at=datetime.fromisoformat(pushed_at.replace("Z", "+00:00")),
        default_branch=str(data.get("default_branch") or "HEAD"),
    )


def _count_repo(full_name: str) -> RepoCounts:
    with tempfile.TemporaryDirectory() as tmp:
        tmpdir = Path(tmp)
        repo_dir = tmpdir / "repo"
        _clone_repo(full_name, repo_dir)
        commit = subprocess.check_output(
            ["git", "-C", str(repo_dir), "rev-parse", "HEAD"], text=True
        ).strip()

        py_files = list(repo_dir.rglob("templatetags/**/*.py"))
        template_files = [
            p
            for p in repo_dir.rglob("templates/**/*")
            if p.is_file() and p.suffix.lower() in {".html", ".txt"}
        ]

        tag_sites = 0
        filter_sites = 0
        for p in py_files:
            try:
                text = p.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            tag_sites += _count_patterns(TAG_PATTERNS, text)
            filter_sites += _count_patterns(FILTER_PATTERNS, text)

        return RepoCounts(
            commit=commit,
            templatetags_py_files=len(py_files),
            templates_files=len(template_files),
            tag_sites=tag_sites,
            filter_sites=filter_sites,
        )


def _clone_repo(full_name: str, dest: Path) -> None:
    url = f"https://github.com/{full_name}.git"
    subprocess.run(
        ["git", "clone", "--depth", "1", url, str(dest)],
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def _count_patterns(patterns: tuple[re.Pattern[str], ...], text: str) -> int:
    return sum(len(p.findall(text)) for p in patterns)


def _print_table(rows: list[dict[str, object]]) -> None:
    cols = [
        ("repo", "repo"),
        ("stars", "stars"),
        ("pushed_at", "pushed_at"),
        ("commit", "commit"),
        ("tag_sites", "tag_sites"),
        ("filter_sites", "filter_sites"),
        ("tt_py", "templatetags_py_files"),
        ("tmpl_files", "templates_files"),
    ]
    widths = {h: len(h) for h, _ in cols}
    for row in rows:
        for h, k in cols:
            widths[h] = max(widths[h], len(str(row[k])))

    header = "  ".join(h.ljust(widths[h]) for h, _ in cols)
    print(header)
    print("  ".join("-" * widths[h] for h, _ in cols))
    for row in rows:
        print("  ".join(str(row[k]).ljust(widths[h]) for h, k in cols))


if __name__ == "__main__":
    raise SystemExit(main())
