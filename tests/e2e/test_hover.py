from __future__ import annotations

from pathlib import Path

import pytest
from lsprotocol.types import DidOpenTextDocumentParams
from lsprotocol.types import HoverParams
from lsprotocol.types import MarkupKind
from lsprotocol.types import Position
from lsprotocol.types import TextDocumentIdentifier
from lsprotocol.types import TextDocumentItem
from pytest_lsp import LanguageClient

from .conftest import TEST_WORKSPACE

BASE_TEMPLATE = (
    TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "base.html"
)
HEADER_TEMPLATE = (
    TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "header.html"
)
LOAD_TEMPLATE = (
    TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "tags" / "load.html"
)


def position_in(path: Path, needle: str) -> Position:
    text = path.read_text(encoding="utf-8")
    offset = text.index(needle)
    before = text[:offset]
    line = before.count("\n")
    line_start = before.rfind("\n") + 1
    return Position(line=line, character=offset - line_start)


@pytest.mark.asyncio
async def test_hover_resolves_template_references(client: LanguageClient):
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

    result = await client.text_document_hover_async(
        HoverParams(
            text_document=TextDocumentIdentifier(uri=BASE_TEMPLATE.as_uri()),
            position=position_in(BASE_TEMPLATE, "djls_app/header.html"),
        )
    )

    assert result is not None
    assert result.contents.kind == MarkupKind.Markdown
    assert '```text\n(template) "djls_app/header.html"\n```' in result.contents.value
    assert f"Resolved to `{HEADER_TEMPLATE}`" in result.contents.value


@pytest.mark.asyncio
async def test_hover_describes_load_libraries(client: LanguageClient):
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

    result = await client.text_document_hover_async(
        HoverParams(
            text_document=TextDocumentIdentifier(uri=BASE_TEMPLATE.as_uri()),
            position=position_in(BASE_TEMPLATE, "static"),
        )
    )

    assert result is not None
    assert result.contents.kind == MarkupKind.Markdown
    assert "```text\n(library) static\n```" in result.contents.value
    assert "```python\ndjango.templatetags.static\n```" in result.contents.value


@pytest.mark.asyncio
async def test_hover_describes_load_symbols(client: LanguageClient):
    client.text_document_did_open(
        DidOpenTextDocumentParams(
            text_document=TextDocumentItem(
                uri=LOAD_TEMPLATE.as_uri(),
                language_id="htmldjango",
                version=1,
                text=LOAD_TEMPLATE.read_text(encoding="utf-8"),
            )
        )
    )

    result = await client.text_document_hover_async(
        HoverParams(
            text_document=TextDocumentIdentifier(uri=LOAD_TEMPLATE.as_uri()),
            position=position_in(LOAD_TEMPLATE, "trans"),
        )
    )

    assert result is not None
    assert result.contents.kind == MarkupKind.Markdown
    assert "```text\n(tag) trans\n```" in result.contents.value
    assert "Requires `{% load i18n %}`." in result.contents.value


@pytest.mark.asyncio
async def test_hover_describes_loaded_tags(client: LanguageClient):
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

    result = await client.text_document_hover_async(
        HoverParams(
            text_document=TextDocumentIdentifier(uri=BASE_TEMPLATE.as_uri()),
            position=position_in(BASE_TEMPLATE, "static 'images/logo.png'"),
        )
    )

    assert result is not None
    assert result.contents.kind == MarkupKind.Markdown
    assert "```text\n(tag) static\n```" in result.contents.value
    assert "Requires `{% load static %}`." in result.contents.value


@pytest.mark.asyncio
async def test_hover_describes_filters(client: LanguageClient):
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

    result = await client.text_document_hover_async(
        HoverParams(
            text_document=TextDocumentIdentifier(uri=BASE_TEMPLATE.as_uri()),
            position=position_in(BASE_TEMPLATE, "lower"),
        )
    )

    assert result is not None
    assert result.contents.kind == MarkupKind.Markdown
    assert "```text\n(filter) lower\n```" in result.contents.value


@pytest.mark.asyncio
async def test_hover_ignores_template_variables(client: LanguageClient):
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

    result = await client.text_document_hover_async(
        HoverParams(
            text_document=TextDocumentIdentifier(uri=BASE_TEMPLATE.as_uri()),
            position=position_in(BASE_TEMPLATE, "user.username"),
        )
    )

    assert result is None
