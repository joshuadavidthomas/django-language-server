from __future__ import annotations

import pytest
from lsprotocol.types import DidOpenTextDocumentParams
from lsprotocol.types import DocumentLinkParams
from lsprotocol.types import Position
from lsprotocol.types import Range
from lsprotocol.types import TextDocumentIdentifier
from lsprotocol.types import TextDocumentItem
from pytest_lsp import LanguageClient

from .conftest import TEST_WORKSPACE

BASE_TEMPLATE = TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "base.html"
FIRST_PARTY_LOAD_TEMPLATE = (
    TEST_WORKSPACE
    / "djls_app"
    / "templates"
    / "djls_app"
    / "tags"
    / "first_party_load.html"
)
FIRST_PARTY_TAG_LIBRARY = TEST_WORKSPACE / "djls_app" / "templatetags" / "djls_app_tags.py"
HOME_TEMPLATE = TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "home.html"


@pytest.mark.asyncio
async def test_document_links_for_template_references(client: LanguageClient):
    client.text_document_did_open(
        DidOpenTextDocumentParams(
            text_document=TextDocumentItem(
                uri=HOME_TEMPLATE.as_uri(),
                language_id="htmldjango",
                version=1,
                text=HOME_TEMPLATE.read_text(encoding="utf-8"),
            )
        )
    )

    result = await client.text_document_document_link_async(
        DocumentLinkParams(
            text_document=TextDocumentIdentifier(uri=HOME_TEMPLATE.as_uri()),
        )
    )

    assert result is not None
    assert len(result) == 1
    link = result[0]
    assert link.range == Range(
        start=Position(line=0, character=12),
        end=Position(line=0, character=30),
    )
    assert link.target == BASE_TEMPLATE.as_uri()


@pytest.mark.asyncio
async def test_document_links_for_load_libraries(client: LanguageClient):
    client.text_document_did_open(
        DidOpenTextDocumentParams(
            text_document=TextDocumentItem(
                uri=FIRST_PARTY_LOAD_TEMPLATE.as_uri(),
                language_id="htmldjango",
                version=1,
                text=FIRST_PARTY_LOAD_TEMPLATE.read_text(encoding="utf-8"),
            )
        )
    )

    result = await client.text_document_document_link_async(
        DocumentLinkParams(
            text_document=TextDocumentIdentifier(uri=FIRST_PARTY_LOAD_TEMPLATE.as_uri()),
        )
    )

    assert result is not None
    assert len(result) == 1
    link = result[0]
    assert link.range == Range(
        start=Position(line=1, character=8),
        end=Position(line=1, character=21),
    )
    assert link.target == FIRST_PARTY_TAG_LIBRARY.as_uri()
