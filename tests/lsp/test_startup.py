from __future__ import annotations

import asyncio
from pathlib import Path

import pytest
import pytest_lsp
from lsprotocol.types import ClientCapabilities
from lsprotocol.types import CompletionParams
from lsprotocol.types import DiagnosticClientCapabilities
from lsprotocol.types import DidOpenTextDocumentParams
from lsprotocol.types import DocumentDiagnosticParams
from lsprotocol.types import InitializeParams
from lsprotocol.types import Position
from lsprotocol.types import TextDocumentClientCapabilities
from lsprotocol.types import TextDocumentIdentifier
from lsprotocol.types import TextDocumentItem
from lsprotocol.types import WindowClientCapabilities
from pytest_lsp import ClientServerConfig
from pytest_lsp import LanguageClient

SERVER_COMMAND = ["cargo", "run", "-q", "-p", "djls", "--", "serve", "--connection-type", "stdio"]


def default_capabilities() -> ClientCapabilities:
    return ClientCapabilities(
        text_document=TextDocumentClientCapabilities(
            diagnostic=DiagnosticClientCapabilities()
        )
    )


async def wait_for_progress(client: LanguageClient, *, timeout: float = 5.0):
    deadline = asyncio.get_running_loop().time() + timeout
    while asyncio.get_running_loop().time() < deadline:
        if client.progress_reports:
            return client.progress_reports
        await asyncio.sleep(0.05)
    return client.progress_reports


@pytest_lsp.fixture(config=ClientServerConfig(server_command=SERVER_COMMAND))
async def client(lsp_client: LanguageClient, tmp_path: Path):
    params = InitializeParams(
        capabilities=default_capabilities(),
        root_uri=tmp_path.as_uri(),
    )
    initialize_result = await lsp_client.initialize_session(params)
    lsp_client.djls_initialize_result = initialize_result

    yield lsp_client

    await lsp_client.shutdown_session()


@pytest_lsp.fixture(config=ClientServerConfig(server_command=SERVER_COMMAND))
async def progress_client(lsp_client: LanguageClient, tmp_path: Path):
    params = InitializeParams(
        capabilities=ClientCapabilities(
            text_document=TextDocumentClientCapabilities(
                diagnostic=DiagnosticClientCapabilities()
            ),
            window=WindowClientCapabilities(work_done_progress=True),
        ),
        root_uri=tmp_path.as_uri(),
    )
    initialize_result = await lsp_client.initialize_session(params)
    lsp_client.djls_initialize_result = initialize_result

    yield lsp_client

    await lsp_client.shutdown_session()


@pytest.mark.asyncio
async def test_initialize_returns_capabilities(client: LanguageClient):
    capabilities = client.djls_initialize_result.capabilities

    assert capabilities is not None
    assert capabilities.text_document_sync is not None
    assert capabilities.completion_provider is not None


@pytest.mark.asyncio
async def test_server_stays_responsive_after_initialized(client: LanguageClient, tmp_path: Path):
    template_path = tmp_path / "templates" / "index.html"
    template_path.parent.mkdir()
    template_path.write_text("{% ", encoding="utf-8")
    uri = template_path.as_uri()

    client.text_document_did_open(
        DidOpenTextDocumentParams(
            text_document=TextDocumentItem(
                uri=uri,
                language_id="html",
                version=1,
                text="{% ",
            )
        )
    )

    result = await client.text_document_completion_async(
        CompletionParams(
            text_document=TextDocumentIdentifier(uri=uri),
            position=Position(line=0, character=3),
        )
    )

    # The assertion is the awaited request above: a protocol error would fail the test.
    assert result is None or hasattr(result, "items") or isinstance(result, list)


@pytest.mark.asyncio
async def test_supported_client_receives_startup_progress_begin_report_end(
    progress_client: LanguageClient,
):
    progress_reports = await wait_for_progress(progress_client)

    assert progress_reports
    events = [event for reports in progress_reports.values() for event in reports]
    assert any(getattr(event, "kind", None) == "begin" for event in events)
    assert any(getattr(event, "kind", None) == "report" for event in events)
    assert any(getattr(event, "kind", None) == "end" for event in events)


@pytest.mark.asyncio
async def test_template_request_works_while_loading_in_progress(
    client: LanguageClient, tmp_path: Path
):
    template_path = tmp_path / "templates" / "during-loading.html"
    template_path.parent.mkdir()
    template_path.write_text("{% load missing %}\n", encoding="utf-8")
    uri = template_path.as_uri()

    client.text_document_did_open(
        DidOpenTextDocumentParams(
            text_document=TextDocumentItem(
                uri=uri,
                language_id="html",
                version=1,
                text="{% load missing %}\n",
            )
        )
    )

    result = await client.text_document_diagnostic_async(
        DocumentDiagnosticParams(text_document=TextDocumentIdentifier(uri=uri))
    )

    assert result is not None


@pytest.mark.asyncio
async def test_unsupported_client_receives_log_fallback(client: LanguageClient):
    await asyncio.sleep(0.2)

    assert not client.progress_reports
    assert client.log_messages
