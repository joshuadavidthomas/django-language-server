from __future__ import annotations

import copy

from tools import performance

REPORT = {
    "recorded_at": "2026-11-04T08:55:00Z",
    "cpu": "CPU <model>",
    "main_commit": "79c48287aaaa",
    "cold_samples": 5,
    "lsp_samples": 3,
    "lsp_edits": 20,
    "release": {"ref": "v6.1.1", "commit": "5c0cf691", "status": "ready"},
    "apps": [
        {
            "app": "djangoproject.com",
            "status": "ok",
            "main": {
                "templates": 108,
                "project": {"root": "/corpus/djangoproject.com"},
                "lsp_template": "/corpus/djangoproject.com/templates/base.html",
                "cold_start_check_templates": {"median_ns": 650_000_000},
                "lsp_cold_start": {"median_ns": 300_000_000},
                "lsp_keystroke": {"median_ns": 500_000},
            },
            # An old release supports the external LSP contract but not the native API.
            "release": {
                "lsp_cold_start": {"median_ns": 400_000_000},
                "lsp_keystroke": {"median_ns": 490_000},
            },
        },
        {"app": "geonode", "status": "deferred", "reason": "native GDAL headers"},
        {
            "app": "netbox",
            "status": "setup-failed",
            "reason": "<script>uv sync failed</script>",
        },
    ],
}


def test_history_keeps_measured_medians_and_one_point_per_run():
    run = performance.summarize(REPORT)
    assert run["apps"] == {
        "djangoproject.com": {
            "status": "ok",
            "reason": None,
            "templates": 108,
            "template": "templates/base.html",
            "main": {
                "cold_start_check_templates": 650_000_000,
                "lsp_cold_start": 300_000_000,
                "lsp_keystroke": 500_000,
            },
            "release": {"lsp_cold_start": 400_000_000, "lsp_keystroke": 490_000},
        },
        "netbox": {
            "status": "setup-failed",
            "reason": "<script>uv sync failed</script>",
        },
    }
    earlier = copy.deepcopy(run) | {"recorded_at": "2026-11-03T08:55:00Z"}
    # Republishing the same measurement replaces its point instead of adding another.
    history = performance.add_run(performance.add_run([run], earlier), run)
    assert [item["recorded_at"] for item in history] == [
        "2026-11-03T08:55:00Z",
        "2026-11-04T08:55:00Z",
    ]


def test_page_explains_comparisons_and_failures_without_deferred_projects():
    page = performance.render([performance.summarize(REPORT)])
    assert "geonode" not in page
    assert "25% faster than v6.1.1 (400 ms)" in page
    assert "about the same as v6.1.1 (0.49 ms)" in page
    assert "v6.1.1 cannot run this measurement, so there is no release line." in page
    assert page.count("Not measured in the latest run") == 3
    assert "&lt;script&gt;uv sync failed&lt;/script&gt;" in page
    assert "<script>" not in page
    assert "1 of 2 projects measured" in page
