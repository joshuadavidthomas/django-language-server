#!/usr/bin/env python3
"""Time real Django projects nightly and render the history page.

This never publishes. See docs/performance.md for the measurement boundaries and
the workflow's separate, permission-scoped publisher.
"""

from __future__ import annotations

import argparse
import hashlib
import html
import json
import platform
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

import tomllib

ROOT = Path(__file__).resolve().parents[1]
# The real applications from the performance survey, not the corpus's package examples.
APPS = (
    "archivebox",
    "babybuddy",
    "django-crm",
    "djangopackages.org",
    "djangoproject.com",
    "healthchecks",
    "horizon",
    "hyperkitty",
    "inventree",
    "linkding",
    "misago",
    "netbox",
    "nomnom",
    "pretix",
    "pythonic-news",
    "readthedocs.org",
    "sentry",
    "simonwillison.net",
)
NATIVE_METRICS = ("cold_start_check_templates", "keystroke_diagnostics")
LSP_METRICS = ("lsp_cold_start", "lsp_keystroke")
METRICS = NATIVE_METRICS + LSP_METRICS
CORPUS = ROOT / "crates/djls-testing/.corpus"


def command(args, *, cwd=ROOT, timeout=900, log=None):
    """Keep tool noise out of JSON and retain complete setup/build logs as artifacts."""
    try:
        result = subprocess.run(
            args,
            cwd=cwd,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        if log:
            log.write_bytes(error.output or b"")
        raise
    if log:
        log.write_text(result.stdout)
    if result.returncode:
        raise RuntimeError(
            f"command failed ({result.returncode}): {' '.join(map(str, args))}"
        )
    return result.stdout.strip()


def fingerprint(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def build(root, output, label, *, server=False):
    target = (
        ["-p", "djls"] if server else ["-p", "djls-bench", "--example", "performance"]
    )
    # A shared ambient CARGO_TARGET_DIR would overwrite main with the release binary.
    target_dir = root / "target"
    command(
        [
            "cargo",
            "build",
            "--locked",
            "--release",
            "--target-dir",
            str(target_dir),
            *target,
        ],
        cwd=root,
        timeout=1800,
        log=output / f"{label}-{'server' if server else 'native'}-build.log",
    )
    return target_dir / ("release/djls" if server else "release/examples/performance")


def prepare_release(root, output):
    """Use exactly the same measurement source on each side, never backport production code."""
    harness = root / "crates/djls-bench/src/project.rs"
    if not harness.exists():
        return {
            "native_status": "unsupported",
            "native_reason": "Release predates the in-process real-project harness. Compare the separate external LSP series instead.",
        }, None
    for relative in (
        "crates/djls-bench/src/project.rs",
        "crates/djls-bench/examples/performance.rs",
        "crates/djls-testing/manifest.toml",
        "crates/djls-testing/manifest.lock",
    ):
        destination = root / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / relative, destination)
    corpus = root / "crates/djls-testing/.corpus"
    if not corpus.exists():
        corpus.symlink_to(CORPUS, target_is_directory=True)
    try:
        return {"native_status": "ready"}, build(root, output, "release")
    except (RuntimeError, subprocess.TimeoutExpired) as error:
        return {
            "native_status": "unsupported",
            "native_reason": f"Frozen measurement harness cannot build against release APIs: {error}. See release-native-build.log.",
        }, None


def sample(binary, app, cold_samples, edit_samples, output, label):
    raw = command(
        [str(binary), app, str(cold_samples), str(edit_samples)],
        timeout=600,
        log=output / f"{app}-{label}.log",
    )
    # The runner emits just one JSON document; unrelated stdout must fail, not be ignored.
    result = json.loads(raw)
    if result["schema_version"] != 1 or result["app"] != app or result["templates"] < 1:
        raise ValueError("measurement identity or template count is invalid")
    for metric, count in zip(NATIVE_METRICS, (cold_samples, edit_samples), strict=True):
        values = result[metric]["samples_ns"]
        if len(values) != count or any(
            not isinstance(v, int) or v <= 0 for v in values
        ):
            raise ValueError(f"invalid sample set for {metric}")
        result[metric]["median_ns"] = statistics.median(values)
    return result


def lsp_sample(driver, binary, descriptor, samples, edits, output, label, app):
    templates = []
    for path in sorted(map(Path, descriptor["templates"])):
        text = path.read_text()
        if any(
            line.lstrip().startswith("<")
            and not line.lstrip().startswith(("<!", "<script", "<style"))
            and "{" not in line
            for line in text.splitlines()
        ):
            templates.append((len(path.read_bytes()), path))
    if not templates:
        raise ValueError("no real template has an editable markup-only line")
    template = max(templates, key=lambda item: item[0])[1]
    raw = command(
        [
            sys.executable,
            str(driver),
            "--binary",
            str(binary),
            "--project-root",
            descriptor["root"],
            "--template",
            str(template),
            "--initialization-options",
            json.dumps(descriptor["initialization_options"]),
            "--samples",
            str(samples),
            "--edits",
            str(edits),
        ],
        timeout=600,
        log=output / f"{app}-{label}-lsp.log",
    )
    result = json.loads(raw)
    if (
        result["schema_version"] != 1
        or result["workload"] != "real_template"
        or result["position_encoding"] != "utf-16"
        or result["changes_per_notification"] != 1
        or result["source_sha256"] != fingerprint(template)
        or len(result["samples"]) != samples
        or any(len(trial["did_change_ns"]) != edits for trial in result["samples"])
    ):
        raise ValueError("unexpected external LSP workload or sample count")
    cold = [trial["cold_start_ns"] for trial in result["samples"]]
    changes = [value for trial in result["samples"] for value in trial["did_change_ns"]]
    if len(changes) != samples * edits or any(
        type(value) is not int or value <= 0 for value in cold + changes
    ):
        raise ValueError("invalid external LSP timings")
    return {
        LSP_METRICS[0]: {"samples_ns": cold, "median_ns": statistics.median(cold)},
        LSP_METRICS[1]: {
            "samples_ns": changes,
            "median_ns": statistics.median(changes),
        },
        "lsp_trial": result,
        "lsp_template": str(template),
        "lsp_template_sha256": fingerprint(template),
    }


# What the page shows, in reading order. The native keystroke series stays in latest.json
# for investigation, but the LSP keystroke already answers "how fast does typing feel".
PAGE_METRICS = (
    (
        "lsp_cold_start",
        "Open a template",
        "From starting the language server and opening one of the project's templates "
        "until that template's diagnostics reach the editor. Median of {lsp_samples} fresh server launches.",
    ),
    (
        "lsp_keystroke",
        "Type a character",
        "From typing one character in that open template until the updated diagnostics reach the editor. "
        "Median of {lsp_keystrokes} keystrokes.",
    ),
    (
        "cold_start_check_templates",
        "Check the whole project",
        "Loading the project and checking every one of its templates, timed inside the language server "
        "with no editor connection. Median of {cold_samples} runs.",
    ),
)
HISTORY_LIMIT = 365


def summarize(report):
    """Reduce one run to the medians the page charts; raw samples stay in latest.json."""
    apps = {}
    for app in report["apps"]:
        if app["status"] == "deferred":
            continue
        entry = {"status": app["status"], "reason": app.get("reason")}
        for label in ("main", "release"):
            measurement = app.get(label, {})
            medians = {
                metric: measurement[metric]["median_ns"]
                for metric in METRICS
                if metric in measurement
            }
            if medians:
                entry[label] = medians
            if "templates" in measurement:
                entry["templates"] = measurement["templates"]
            if "lsp_template" in measurement and "project" in app.get("main", {}):
                entry["template"] = str(
                    Path(measurement["lsp_template"]).relative_to(
                        app["main"]["project"]["root"]
                    )
                )
        apps[app["app"]] = entry
    return {
        "recorded_at": report["recorded_at"],
        "main_commit": report["main_commit"],
        "release": report["release"]["ref"],
        "release_commit": report["release"]["commit"],
        "release_status": report["release"]["status"],
        "cpu": report["cpu"],
        "samples": {
            "cold_samples": report["cold_samples"],
            "lsp_samples": report["lsp_samples"],
            "lsp_keystrokes": report["lsp_samples"] * report["lsp_edits"],
        },
        "apps": apps,
    }


def add_run(history, run):
    """Rerunning the publisher for the same measurement must not add a duplicate point."""
    runs = [
        old
        for old in history
        if (old["recorded_at"], old["main_commit"])
        != (run["recorded_at"], run["main_commit"])
    ]
    runs.append(run)
    runs.sort(key=lambda item: item["recorded_at"])
    return runs[-HISTORY_LIMIT:]


def duration(ns):
    if ns >= 1_000_000_000:
        return f"{ns / 1_000_000_000:.2f} s"
    if ns >= 10_000_000:
        return f"{ns / 1_000_000:.0f} ms"
    if ns >= 1_000_000:
        return f"{ns / 1_000_000:.1f} ms"
    return f"{ns / 1_000_000:.2f} ms"


def day(timestamp):
    return time.strftime("%b %-d, %Y", time.strptime(timestamp, "%Y-%m-%dT%H:%M:%SZ"))


def chart(history, app, metric):
    width, height, left, right, top, bottom = 320, 130, 46, 10, 10, 22
    stamps = [
        time.mktime(time.strptime(run["recorded_at"], "%Y-%m-%dT%H:%M:%SZ"))
        for run in history
    ]
    first, last = stamps[0], stamps[-1]
    series = {}
    for stamp, run in zip(stamps, history, strict=True):
        entry = run["apps"].get(app, {})
        for label in ("main", "release"):
            value = entry.get(label, {}).get(metric)
            if value is not None:
                series.setdefault(label, []).append((stamp, value, run))
    peak = max(
        (value for points in series.values() for _, value, _ in points), default=1
    )

    def x(stamp):
        if last == first:
            return left + (width - left - right) / 2
        return left + (stamp - first) / (last - first) * (width - left - right)

    def y(value):
        return top + (1 - value / (peak * 1.1)) * (height - top - bottom)

    parts = [
        f"<line x1='{left}' y1='{y(0):.1f}' x2='{width - right}' y2='{y(0):.1f}' class='axis'/>",
        f"<line x1='{left}' y1='{y(peak):.1f}' x2='{width - right}' y2='{y(peak):.1f}' class='grid'/>",
        f"<text x='{left - 4}' y='{y(peak) + 4:.1f}' class='tick' text-anchor='end'>{duration(peak)}</text>",
        f"<text x='{left - 4}' y='{y(0) + 4:.1f}' class='tick' text-anchor='end'>0</text>",
        f"<text x='{left}' y='{height - 4}' class='tick'>{day(history[0]['recorded_at'])}</text>",
    ]
    if last != first:
        parts.append(
            f"<text x='{width - right}' y='{height - 4}' class='tick' text-anchor='end'>{day(history[-1]['recorded_at'])}</text>"
        )
    for label in ("release", "main"):
        points = series.get(label, [])
        if len(points) > 1:
            path = " ".join(f"{x(s):.1f},{y(v):.1f}" for s, v, _ in points)
            parts.append(f"<polyline points='{path}' class='{label}'/>")
        for stamp, value, run in points:
            name = (
                f"main {run['main_commit'][:7]}" if label == "main" else run["release"]
            )
            parts.append(
                f"<circle cx='{x(stamp):.1f}' cy='{y(value):.1f}' r='3' class='{label}'>"
                f"<title>{html.escape(day(run['recorded_at']))} · {html.escape(name)}: {duration(value)}</title></circle>"
            )
    return f"<svg viewBox='0 0 {width} {height}' role='img' aria-label='{html.escape(app)} over time'>{''.join(parts)}</svg>"


def render(history):
    latest = history[-1]
    release = html.escape(latest["release"])
    sections = []
    for metric, title, description in PAGE_METRICS:
        cards = []
        compared = False
        for app in sorted(latest["apps"]):
            entry = latest["apps"][app]
            main = entry.get("main", {}).get(metric)
            base = entry.get("release", {}).get(metric)
            compared |= base is not None
            if main is None:
                headline = "<p class='value missing'>Not measured in the latest run</p>"
            else:
                headline = f"<p class='value'>{duration(main)}</p>"
            if main is not None and base is not None:
                change = (main - base) / base
                verdict = (
                    "about the same as"
                    if abs(change) < 0.03
                    else f"{abs(change):.0%} {'faster' if change < 0 else 'slower'} than"
                )
                headline += (
                    f"<p class='compare'>{verdict} {release} ({duration(base)})</p>"
                )
            detail = []
            if "templates" in entry:
                detail.append(f"{entry['templates']} templates")
            if metric != "cold_start_check_templates" and "template" in entry:
                detail.append(f"opens <code>{html.escape(entry['template'])}</code>")
            cards.append(
                f"<article><h3>{html.escape(app)}</h3><p class='detail'>{' · '.join(detail)}</p>"
                f"{headline}{chart(history, app, metric)}</article>"
            )
        note = (
            ""
            if compared
            else f" {release} cannot run this measurement, so there is no release line."
        )
        sections.append(
            f"<section id='{metric}'><h2>{title}</h2><p>{html.escape(description.format(**latest['samples']))}{note}</p>"
            f"<div class='cards'>{''.join(cards)}</div></section>"
        )
    missing = [
        f"<li><strong>{html.escape(app)}</strong>: {html.escape(entry.get('reason') or entry['status'])}</li>"
        for app, entry in sorted(latest["apps"].items())
        if entry["status"] != "ok"
    ]
    problems = (
        f"<div class='problems'><p>Some measurements failed in the latest run. Their points are missing, not zero.</p><ul>{''.join(missing)}</ul></div>"
        if missing
        else ""
    )
    measured = sum(entry["status"] == "ok" for entry in latest["apps"].values())
    return f"""<!doctype html>
<html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Django Language Server performance on real projects</title>
<style>
body{{font:16px/1.5 system-ui,sans-serif;color:#1f2933;background:#f7f8fa;margin:0;padding:32px}}
main{{max-width:1400px;margin:auto}}h1{{font-size:28px;margin:0 0 8px}}h2{{margin:40px 0 4px}}a{{color:#1257a6}}
.lede{{max-width:52rem}}.legend span{{display:inline-flex;align-items:center;gap:6px;margin-right:20px}}
.swatch{{width:28px;height:0;border-top:3px solid #1f6feb}}.swatch.release{{border-top:2px dashed #8a94a3}}
.problems{{background:#fff4e5;border-left:4px solid #d97706;padding:8px 16px;margin:16px 0}}
.cards{{display:grid;grid-template-columns:repeat(auto-fill,minmax(300px,1fr));gap:16px;margin-top:16px}}
article{{background:white;border:1px solid #dde2e8;border-radius:8px;padding:14px 16px}}
h3{{margin:0;font-size:17px}}.detail{{margin:0;color:#5b6675;font-size:13px}}code{{font-size:12px;overflow-wrap:anywhere}}
.value{{font-size:26px;font-weight:600;margin:6px 0 0;font-variant-numeric:tabular-nums}}.value.missing{{font-size:15px;color:#b45309}}
.compare{{margin:0;color:#3d4753;font-size:14px}}svg{{width:100%;height:auto;margin-top:8px}}
.axis{{stroke:#9aa4b1}}.grid{{stroke:#e6e9ee;stroke-dasharray:2 3}}.tick{{font-size:10px;fill:#6b7684}}
polyline{{fill:none;stroke-width:2}}polyline.main{{stroke:#1f6feb}}polyline.release{{stroke:#8a94a3;stroke-dasharray:5 4}}
circle.main{{fill:#1f6feb}}circle.release{{fill:white;stroke:#8a94a3;stroke-width:1.5}}
footer{{margin-top:48px;color:#5b6675;font-size:14px;max-width:52rem}}
@media(max-width:600px){{body{{padding:16px}}}}
</style><main>
<h1>Django Language Server performance on real projects</h1>
<p class="lede">Every night, the language server built from <code>main</code> is timed on {len(latest["apps"])} open-source
Django projects, doing what an editor asks of it. The latest release is timed in the same run on the same
machine for comparison. Lower is better. Hover a point to see its date and value.</p>
<p class="legend"><span><i class="swatch"></i><code>main</code></span><span><i class="swatch release"></i>Latest release ({release} in the latest run)</span></p>
<p>Latest run: {day(latest["recorded_at"])} · <code>main</code> at <code>{html.escape(latest["main_commit"][:7])}</code> ·
{measured} of {len(latest["apps"])} projects measured · {len(history)} night{"s" if len(history) != 1 else ""} of history</p>
<p>Jump to: {" · ".join(f"<a href='#{metric}'>{title}</a>" for metric, title, _ in PAGE_METRICS)}</p>
{problems}
{"".join(sections)}
<footer><p>Times are wall-clock times on a shared GitHub-hosted runner ({html.escape(latest["cpu"])}), so they move from night to night
with the hardware. Both lines on a chart come from the same machine and the same run, so the gap between them is more
reliable than a jump in one line. Pull requests are checked separately by CodSpeed instruction counts.</p>
<p><a href="latest.json">Raw samples from the latest run</a> · <a href="history.json">All charted values</a></p></footer>
</main></html>
"""


def page(args):
    history = json.loads(args.history.read_text()) if args.history.exists() else []
    history = add_run(history, summarize(json.loads(args.latest.read_text())))
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "history.json").write_text(json.dumps(history, indent=1) + "\n")
    (args.output / "index.html").write_text(render(history))


def collect(args):
    start = time.monotonic()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    manifest = ROOT / "crates/djls-testing/manifest.toml"
    recipes = {
        repo["name"]: repo for repo in tomllib.loads(manifest.read_text())["repo"]
    }
    cpu = next(
        (
            line.split(":", 1)[1].strip()
            for line in Path("/proc/cpuinfo").read_text().splitlines()
            if line.startswith("model name")
        ),
        platform.processor(),
    )
    report = {
        "schema_version": 1,
        "recorded_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "cpu": cpu,
        "platform": platform.platform(),
        "main_commit": command(["git", "rev-parse", "HEAD"]),
        "manifest_sha256": fingerprint(manifest),
        "lock_sha256": fingerprint(ROOT / "crates/djls-testing/manifest.lock"),
        "cold_samples": args.cold_samples,
        "edit_samples": args.edit_samples,
        "lsp_samples": args.lsp_samples,
        "lsp_edits": args.lsp_edits,
        "cohort": list(APPS),
        "requested_apps": list(args.apps),
        "complete_cohort": set(args.apps) == set(APPS),
        "measurement_sha256": fingerprint(
            ROOT / "crates/djls-bench/examples/performance.rs"
        ),
        "harness_sha256": fingerprint(ROOT / "crates/djls-bench/src/project.rs"),
        "lsp_driver_sha256": fingerprint(args.lsp_driver),
        "release": {
            "ref": args.release_ref,
            "commit": command(["git", "rev-parse", "HEAD"], cwd=args.release_dir),
        },
        "apps": [],
    }
    binary = build(ROOT, output, "main")
    status, release_binary = prepare_release(args.release_dir, output)
    report["release"].update(status)
    main_server = build(ROOT, output, "main", server=True)
    try:
        release_server = build(args.release_dir, output, "release", server=True)
        report["release"]["status"] = "ready"
    except (RuntimeError, subprocess.TimeoutExpired) as error:
        release_server = None
        report["release"].update(status="build-failed", reason=str(error))
    report["server_binary_sha256"] = {
        "main": fingerprint(main_server),
        "release": fingerprint(release_server) if release_server else None,
    }
    for index, name in enumerate(args.apps):
        app = {"app": name}
        report["apps"].append(app)
        dependencies = recipes[name]["environment"]["dependencies"]
        if dependencies["kind"] == "deferred":
            app.update(status="deferred", reason=dependencies["reason"])
            continue
        try:
            if not args.skip_setup:
                command(
                    [
                        "cargo",
                        "run",
                        "--locked",
                        "-q",
                        "-p",
                        "djls-testing",
                        "--bin",
                        "corpus",
                        "--",
                        "environment",
                        "sync",
                        name,
                    ],
                    log=output / f"{name}-setup.log",
                )
            provenance = CORPUS / "environments" / name / "provenance.json"
            if provenance.exists():
                app["environment_sha256"] = fingerprint(provenance)
                app["environment"] = json.loads(provenance.read_text())
            # Alternating the first version limits systematic ordering bias; these are
            # sequential, never concurrent jobs on different runners.
            versions = [("main", binary), ("release", release_binary)]
            if index % 2:
                versions.reverse()
            for label, executable in versions:
                if executable is None:
                    continue
                try:
                    app[label] = {
                        "status": "ok",
                        **sample(
                            executable,
                            name,
                            args.cold_samples,
                            args.edit_samples,
                            output,
                            label,
                        ),
                    }
                except (
                    RuntimeError,
                    subprocess.TimeoutExpired,
                    ValueError,
                    KeyError,
                ) as error:
                    app[label] = {"status": "failed", "errors": [str(error)]}
            descriptor = app["main"].get("project")
            if descriptor:
                servers = [("main", main_server), ("release", release_server)]
                if index % 2:
                    servers.reverse()
                for label, executable in servers:
                    measurement = app.setdefault(label, {"status": "ok"})
                    if executable is None:
                        measurement["status"] = "failed"
                        measurement.setdefault("errors", []).append(
                            "Release server build failed; see release-server-build.log."
                        )
                        continue
                    try:
                        measurement.update(
                            lsp_sample(
                                args.lsp_driver,
                                executable,
                                descriptor,
                                args.lsp_samples,
                                args.lsp_edits,
                                output,
                                label,
                                name,
                            )
                        )
                    except (
                        RuntimeError,
                        subprocess.TimeoutExpired,
                        ValueError,
                        KeyError,
                    ) as error:
                        measurement["status"] = "failed"
                        measurement.setdefault("errors", []).append(str(error))
            app["status"] = app["main"]["status"]
            if app.get("release", {}).get("status") == "failed":
                app["status"] = "failed"
            if app["status"] != "ok":
                app["reason"] = (
                    "One or more measurements failed; available samples are retained. See run artifacts."
                )
        except (RuntimeError, subprocess.TimeoutExpired) as error:
            app.update(status="setup-failed", reason=str(error))
        print(f"{name}: {app['status']}", flush=True)
    report["completed"] = sum(app["status"] == "ok" for app in report["apps"])
    report["elapsed_seconds"] = time.monotonic() - start
    (output / "latest.json").write_text(json.dumps(report, indent=2) + "\n")
    summary = f"Measured {report['completed']}/{len(report['apps'])} apps. Release {args.release_ref}: {report['release']['status']}. Elapsed {report['elapsed_seconds']:.1f}s.\n"
    (output / "summary.md").write_text(summary)
    print(summary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    measure = commands.add_parser(
        "collect", help="Measure main and a release; write latest.json"
    )
    measure.add_argument(
        "--release-dir",
        type=Path,
        required=True,
        help="Disposable checkout, modified only for measurement source and shared corpus",
    )
    measure.add_argument("--release-ref", required=True)
    measure.add_argument("--output", type=Path, required=True)
    measure.add_argument("--apps", nargs="+", choices=APPS, default=APPS)
    measure.add_argument("--cold-samples", type=int, default=5)
    measure.add_argument("--edit-samples", type=int, default=100)
    measure.add_argument("--lsp-samples", type=int, default=3)
    measure.add_argument("--lsp-edits", type=int, default=20)
    measure.add_argument("--lsp-driver", type=Path, default=ROOT / "tools/bench_lsp.py")
    measure.add_argument(
        "--skip-setup",
        action="store_true",
        help="Use already synchronized environments for local validation",
    )
    publish = commands.add_parser(
        "page", help="Add latest.json to the history and render the page"
    )
    publish.add_argument("--latest", type=Path, required=True)
    publish.add_argument(
        "--history",
        type=Path,
        required=True,
        help="Previous history.json; may not exist yet",
    )
    publish.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "page":
        page(args)
        return
    if min(args.cold_samples, args.edit_samples, args.lsp_samples, args.lsp_edits) < 1:
        measure.error("sample counts must be positive")
    args.release_dir = args.release_dir.resolve()
    if args.release_dir == ROOT:
        measure.error("release-dir must be a separate disposable checkout")
    collect(args)


if __name__ == "__main__":
    main()
