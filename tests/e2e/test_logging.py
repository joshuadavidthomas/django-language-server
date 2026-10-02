from __future__ import annotations

import os
import sys
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

# Mirrors `LOG_FILE_LIMIT` in crates/djls-server/src/logging.rs.
LOG_FILE_LIMIT = 7
TEMPLATE = TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "base.html"
# Salsa logs each query execution at INFO; none of it belongs in the editor.
SALSA_QUERY_MESSAGE = "executing query"


async def start_isolated_server(cache: Path, rust_log: str | None) -> LanguageClient:
    if sys.platform not in {"linux", "darwin"}:
        pytest.skip("isolated logging coverage needs a supported cache directory")

    server_env = os.environ.copy()
    if sys.platform == "darwin":
        # directories uses HOME on macOS. Preserve Cargo/Rustup locations while
        # isolating the server's Library/Caches directory.
        server_env["CARGO_HOME"] = os.environ.get("CARGO_HOME", str(Path.home() / ".cargo"))
        server_env["RUSTUP_HOME"] = os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup"))
        server_env["PATH"] = str(Path(server_env["CARGO_HOME"]) / "bin") + os.pathsep + server_env["PATH"]
        server_env["HOME"] = str(cache)
    else:
        server_env["XDG_CACHE_HOME"] = str(cache)
    if rust_log is None:
        server_env.pop("RUST_LOG", None)
    else:
        server_env["RUST_LOG"] = rust_log

    client = await ClientServerConfig(
        server_command=SERVER_COMMAND,
        server_env=server_env,
    ).start()
    try:
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
    except BaseException:
        await client.stop()
        raise
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


def daily_logs(log_dir: Path) -> list[Path]:
    return sorted(log_dir.glob("djls.log.*"))


def seed_old_logs(log_dir: Path) -> list[Path]:
    old = [log_dir / f"djls.log.2000-01-{day:02}" for day in range(1, 11)]
    for path in old:
        path.write_bytes(b"old\n")
    return old


@pytest.mark.asyncio
async def test_default_logging_keeps_editor_visibility_without_dependency_info(
    tmp_path: Path,
):
    cache = tmp_path / "cache"
    log_dir = (cache / "Library" / "Caches" if sys.platform == "darwin" else cache) / "djls"
    log_dir.mkdir(parents=True)
    old = seed_old_logs(log_dir)
    unrelated = log_dir / "keep.txt"
    unrelated.write_bytes(b"kept\n")

    client = await start_isolated_server(cache, rust_log=None)
    try:
        await wait_for_log_message(client, "Project reload completed")
        await exercise_edits(client, 40)
        messages = [message.message for message in client.log_messages]
        assert "Initializing server..." in messages
        assert not any(SALSA_QUERY_MESSAGE in message for message in messages)
    finally:
        await client.shutdown_session()

    logs = daily_logs(log_dir)
    assert len(logs) == LOG_FILE_LIMIT
    current = [path for path in logs if path not in old]
    assert len(current) == 1
    records = current[0].read_bytes()
    assert b"Initializing server" in records
    assert b"salsa::" not in records.lower()
    assert unrelated.read_bytes() == b"kept\n"


@pytest.mark.asyncio
async def test_verbose_logging_reaches_files_but_not_the_editor(tmp_path: Path):
    cache = tmp_path / "cache"
    log_dir = (cache / "Library" / "Caches" if sys.platform == "darwin" else cache) / "djls"
    log_dir.mkdir(parents=True)
    old = seed_old_logs(log_dir)

    client = await start_isolated_server(cache, rust_log="trace")
    try:
        await exercise_edits(client, 100)
        messages = [message.message for message in client.log_messages]
        assert "Initializing server..." in messages
        assert not any(SALSA_QUERY_MESSAGE in message for message in messages)
    finally:
        await client.shutdown_session()

    assert len(daily_logs(log_dir)) == LOG_FILE_LIMIT
    records = b"".join(
        path.read_bytes() for path in daily_logs(log_dir) if path not in old
    )
    # Salsa's per-query INFO reached the file, so its absence from the editor
    # above is the editor filter at work.
    assert b" TRACE " in records
    assert SALSA_QUERY_MESSAGE.encode() in records
