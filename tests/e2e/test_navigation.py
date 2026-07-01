from __future__ import annotations

import pytest
from lsprotocol.types import DefinitionParams
from lsprotocol.types import DidOpenTextDocumentParams
from lsprotocol.types import Position
from lsprotocol.types import Range
from lsprotocol.types import ReferenceContext
from lsprotocol.types import ReferenceParams
from lsprotocol.types import TextDocumentIdentifier
from lsprotocol.types import TextDocumentItem
from pytest_lsp import LanguageClient

from .conftest import TEST_WORKSPACE
from .utils import position_in

BASE_TEMPLATE = TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "base.html"
HEADER_TEMPLATE = TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "header.html"
HOME_TEMPLATE = TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "home.html"
EXTENDS_TAG_TEMPLATE = (
    TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "tags" / "extends.html"
)


@pytest.mark.asyncio
async def test_goto_definition_for_extends_template_reference(client: LanguageClient):
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

    result = await client.text_document_definition_async(
        DefinitionParams(
            text_document=TextDocumentIdentifier(uri=HOME_TEMPLATE.as_uri()),
            position=position_in(HOME_TEMPLATE, "djls_app/base.html"),
        )
    )

    assert result is not None
    assert result.uri == BASE_TEMPLATE.as_uri()
    assert result.range == Range(
        start=Position(line=0, character=0),
        end=Position(line=0, character=0),
    )


@pytest.mark.asyncio
async def test_goto_definition_for_include_template_reference(client: LanguageClient):
    client.text_document_did_open(
        DidOpenTextDocumentParams(
            text_document=TextDocumentItem(
                uri=BASE_TEMPLATE.as_uri(),
                language_id="htmldjango",
                version=1,
                text=BASE_TEMPLATE.read_text(encoding="utf-8"),
            )
        )
    )

    result = await client.text_document_definition_async(
        DefinitionParams(
            text_document=TextDocumentIdentifier(uri=BASE_TEMPLATE.as_uri()),
            position=position_in(BASE_TEMPLATE, "djls_app/header.html"),
        )
    )

    assert result is not None
    assert result.uri == HEADER_TEMPLATE.as_uri()
    assert result.range == Range(
        start=Position(line=0, character=0),
        end=Position(line=0, character=0),
    )


@pytest.mark.asyncio
async def test_find_references_for_template_reference(client: LanguageClient):
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

    result = await client.text_document_references_async(
        ReferenceParams(
            text_document=TextDocumentIdentifier(uri=HOME_TEMPLATE.as_uri()),
            position=position_in(HOME_TEMPLATE, "djls_app/base.html"),
            context=ReferenceContext(include_declaration=True),
        )
    )

    assert result is not None
    assert {
        (
            location.uri,
            location.range.start.line,
            location.range.start.character,
            location.range.end.line,
            location.range.end.character,
        )
        for location in result
    } == {
        (HOME_TEMPLATE.as_uri(), 0, 2, 0, 32),
        (EXTENDS_TAG_TEMPLATE.as_uri(), 2, 2, 2, 32),
        (EXTENDS_TAG_TEMPLATE.as_uri(), 10, 2, 10, 32),
    }


@pytest.mark.asyncio
async def test_navigation_ignores_tag_name(client: LanguageClient):
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

    definition = await client.text_document_definition_async(
        DefinitionParams(
            text_document=TextDocumentIdentifier(uri=HOME_TEMPLATE.as_uri()),
            position=position_in(HOME_TEMPLATE, "extends"),
        )
    )
    references = await client.text_document_references_async(
        ReferenceParams(
            text_document=TextDocumentIdentifier(uri=HOME_TEMPLATE.as_uri()),
            position=position_in(HOME_TEMPLATE, "extends"),
            context=ReferenceContext(include_declaration=True),
        )
    )

    assert definition is None
    assert references is None
