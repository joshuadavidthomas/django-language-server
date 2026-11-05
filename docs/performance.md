# Nightly performance statistics

The `nightly performance` workflow times the language server on real open-source
Django projects every day at 08:43 UTC and publishes a page of charts over time.
It builds current `main` and the latest stable GitHub release and times both in the
same run on the same GitHub-hosted Ubuntu 22.04 runner. It never runs on pull
requests. CodSpeed instruction counts in `bench.yml` remain the regression gate.

## What the page shows

Each section has one chart per project. The solid line is `main`, and the dashed line
is the release that was latest on that night. Lower is better.

| Section | Timed operation | Series in `latest.json` |
| --- | --- | --- |
| Open a template | Start a server process, initialize it, open the project's largest template that has an editable markup-only line, and wait until that template's diagnostics arrive. Median of 3 fresh processes. | `lsp_cold_start` |
| Type a character | Insert or delete one character in that open template and wait until the diagnostics for that version arrive. Median of 60 keystrokes: 20 in each process, after 2 untimed edits. | `lsp_keystroke` |
| Check the whole project | In-process: open the project with its real environment and check every project template. Median of 5 runs. | `cold_start_check_templates` |

"Open a template" and "Type a character" drive the server binary over stdio with
`tools/bench_lsp.py`, the same way an editor does. "Check the whole project" runs in
process, so it has a release line only when the release contains the in-process
measurement harness. v6.1.1 does not. The collector also records in-process keystroke
timings (`keystroke_diagnostics`) in `latest.json` but does not chart them.

Wall-clock times on shared runners change with the hardware from night to night. The
two lines on one chart come from the same machine in the same run, so the gap between
them is more reliable than one jump in a single line. A failed measurement leaves a gap
in the chart, never a zero, and the page lists each failed project with its reason.
The workflow then fails after it publishes the page.

The page covers the 18 runnable applications from the performance survey. The page
does not list GeoNode because the corpus manifest defers it: GeoNode needs native GDAL
headers.

## Outputs and publication

`tools/performance.py collect` writes `latest.json`, `summary.md`, and complete build,
setup, and run logs. `latest.json` holds raw nanosecond samples, diagnostics, the CPU
model, commit identities, measurement source and binary hashes, and per-project
environment fingerprints.

`tools/performance.py page` adds the medians from `latest.json` to `history.json`,
keeps the last 365 runs, and renders `index.html`. Republishing the same run replaces
its point. The page is plain HTML with inline SVG charts and does not load any scripts.

The read-only measurement job uploads the collector output as a 30-day artifact. The
publish job checks out `gh-pages` in a separate worktree, renders the page into
`performance/`, pushes `gh-pages` with `GITHUB_TOKEN`, and deploys it with the Pages
deployment actions. On the first run, the job creates `gh-pages` as an orphan branch.
Logs are kept only as Actions artifacts and are not published.

Repository setup:

1. Set **Settings → Pages → Source** to **GitHub Actions**.
2. Let Actions write repository contents.
3. Configure the `github-pages` environment to allow deployments from main.
4. Make sure branch protections allow the workflow to update `gh-pages`.

Local validation does not change any repository setting.

## Run locally

On Linux, use Python 3.11 or newer and the repository's Rust and uv tools. Create a
disposable release checkout and synchronize the corpus source:

```bash
git worktree add --detach /tmp/djls-performance-release v6.1.1
just corpus sync --source-only
uv run --no-project --python 3.12 python tools/performance.py collect \
  --release-dir /tmp/djls-performance-release --release-ref v6.1.1 \
  --apps djangoproject.com netbox --output /tmp/djls-performance
python3 tools/performance.py page --latest /tmp/djls-performance/latest.json \
  --history /tmp/djls-performance/history.json --output /tmp/djls-performance
```

Omit `--apps` to measure every project. `--skip-setup` reuses ready environments. The
collector never pushes, deploys, or dispatches a workflow.
