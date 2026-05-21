from __future__ import annotations

from pathlib import Path

import pytest
import pytest_lsp
from lsprotocol.types import ClientCapabilities
from lsprotocol.types import CompletionParams
from lsprotocol.types import DidOpenTextDocumentParams
from lsprotocol.types import InitializeParams
from lsprotocol.types import Position
from lsprotocol.types import TextDocumentIdentifier
from lsprotocol.types import TextDocumentItem
from pytest_lsp import ClientServerConfig
from pytest_lsp import LanguageClient

SERVER_COMMAND = ["cargo", "run", "-q", "-p", "djls", "--", "serve", "--connection-type", "stdio"]


@pytest_lsp.fixture(config=ClientServerConfig(server_command=SERVER_COMMAND))
async def client(lsp_client: LanguageClient, tmp_path: Path):
    params = InitializeParams(
        capabilities=ClientCapabilities(),
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
