from __future__ import annotations

import os
import sys
import time
from pathlib import Path

import pytest
from lsprotocol import types
from pytest_lsp import ClientServerConfig
from pytest_lsp import LanguageClient
from pytest_lsp import client_capabilities

from .conftest import SERVER_COMMAND
from .conftest import TEST_WORKSPACE
from .conftest import wait_for_log_message
from .conftest import wait_for_project_load

FILE_LIMIT = 16 * 1024 * 1024
MANAGED_LOG_NAMES = [
    "djls-bounded.log",
    "djls-bounded.log.1",
    "djls-bounded.log.2",
    "djls-bounded.log.3",
]
TEMPLATE = TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "base.html"
# Salsa logs each query execution at INFO; none of it belongs in the editor.
SALSA_QUERY_MESSAGE = "executing query"


async def start_isolated_server(cache: Path, rust_log: str | None) -> LanguageClient:
    if sys.platform != "linux":
        pytest.skip("isolated logging coverage currently configures XDG_CACHE_HOME")

    server_env = os.environ.copy()
    server_env["XDG_CACHE_HOME"] = str(cache)
    if rust_log is None:
        server_env.pop("RUST_LOG", None)
    else:
        server_env["RUST_LOG"] = rust_log

    client = await ClientServerConfig(
        server_command=SERVER_COMMAND,
        server_env=server_env,
    ).start()
    await client.initialize_session(
        types.InitializeParams(
            capabilities=client_capabilities("visual-studio-code"),
            workspace_folders=[
                types.WorkspaceFolder(
                    uri=TEST_WORKSPACE.as_uri(),
                    name="test_project",
                )
            ],
        )
    )
    await wait_for_project_load(client)
    return client


async def exercise_edits(client: LanguageClient, count: int) -> None:
    uri = TEMPLATE.as_uri()
    client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=uri,
                language_id="htmldjango",
                version=0,
                text=TEMPLATE.read_text(encoding="utf-8"),
            )
        )
    )
    for version in range(1, count + 1):
        client.text_document_did_change(
            types.DidChangeTextDocumentParams(
                text_document=types.VersionedTextDocumentIdentifier(
                    uri=uri,
                    version=version,
                ),
                content_changes=[
                    types.TextDocumentContentChangeWholeDocument(
                        text="{{ value" if version % 2 else "<p>{{ value }}</p>"
                    )
                ],
            )
        )

    # This request is ordered after the notifications and gives their background
    # work a chance to emit before shutdown closes and drains the logging queue.
    await client.text_document_formatting_async(
        types.DocumentFormattingParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            options=types.FormattingOptions(tab_size=4, insert_spaces=True),
        )
    )


def managed_logs(log_dir: Path) -> list[Path]:
    return [log_dir / name for name in MANAGED_LOG_NAMES if (log_dir / name).exists()]


@pytest.mark.asyncio
async def test_default_logging_keeps_editor_visibility_without_dependency_info(
    tmp_path: Path,
):
    cache = tmp_path / "cache"
    log_dir = cache / "djls"
    log_dir.mkdir(parents=True)
    stale_legacy = log_dir / "djls.log.2026-01-01"
    recent_legacy = log_dir / "djls.log.2026-01-02"
    unrelated = log_dir / "keep.txt"
    for path in (stale_legacy, recent_legacy, unrelated):
        path.write_bytes(b"kept\n")
    two_days_ago = time.time() - 2 * 24 * 60 * 60
    os.utime(stale_legacy, (two_days_ago, two_days_ago))

    client = await start_isolated_server(cache, rust_log=None)
    try:
        await wait_for_log_message(client, "Project reload completed")
        await exercise_edits(client, 40)
        messages = [message.message for message in client.log_messages]
        assert "Initializing server..." in messages
        assert not any(SALSA_QUERY_MESSAGE in message for message in messages)
    finally:
        await client.shutdown_session()

    records = b"".join(path.read_bytes() for path in managed_logs(log_dir))
    assert records
    assert b"salsa::" not in records.lower()
    assert not stale_legacy.exists()
    assert recent_legacy.read_bytes() == b"kept\n"
    assert unrelated.read_bytes() == b"kept\n"


@pytest.mark.asyncio
async def test_verbose_logging_repairs_and_stays_within_managed_budget(
    tmp_path: Path,
):
    cache = tmp_path / "cache"
    log_dir = cache / "djls"
    log_dir.mkdir(parents=True)
    for name in MANAGED_LOG_NAMES:
        with (log_dir / name).open("wb") as log:
            log.truncate(FILE_LIMIT + 1)

    client = await start_isolated_server(cache, rust_log="trace")
    try:
        await exercise_edits(client, 100)
        messages = [message.message for message in client.log_messages]
        assert "Initializing server..." in messages
        assert not any(SALSA_QUERY_MESSAGE in message for message in messages)
    finally:
        await client.shutdown_session()

    logs = managed_logs(log_dir)
    assert len(logs) == len(MANAGED_LOG_NAMES)
    assert (log_dir / MANAGED_LOG_NAMES[0]).stat().st_size > 0
    assert all(path.stat().st_size <= FILE_LIMIT for path in logs)
    assert sum(path.stat().st_size for path in logs) <= FILE_LIMIT * len(
        MANAGED_LOG_NAMES
    )
    # RUST_LOG=trace reached the file, so the editor's silence above is filtering.
    active = (log_dir / MANAGED_LOG_NAMES[0]).read_bytes()
    assert b" TRACE " in active
    assert b"salsa::" in active
    assert not list(log_dir.glob("djls-bounded.log.[4-9]*"))
