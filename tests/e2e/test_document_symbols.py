from __future__ import annotations

import pytest
from lsprotocol.types import DidOpenTextDocumentParams
from lsprotocol.types import DocumentSymbolParams
from lsprotocol.types import SymbolKind
from lsprotocol.types import TextDocumentIdentifier
from lsprotocol.types import TextDocumentItem
from pytest_lsp import LanguageClient

from .conftest import TEST_WORKSPACE

BASE_TEMPLATE = TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "base.html"
HEADER_TEMPLATE = TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "header.html"


@pytest.mark.asyncio
async def test_document_symbols_include_template_outline(client: LanguageClient):
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

    result = await client.text_document_document_symbol_async(
        DocumentSymbolParams(
            text_document=TextDocumentIdentifier(uri=BASE_TEMPLATE.as_uri()),
        )
    )

    assert result is not None
    assert [(symbol.name, symbol.detail, symbol.kind) for symbol in result] == [
        ("static", "load", SymbolKind.Module),
        ("title", "block", SymbolKind.Namespace),
        ("djls_app/header.html", "include", SymbolKind.File),
        ("content", "block", SymbolKind.Namespace),
    ]

    content = result[3]
    assert content.range.start.line == 13
    assert content.range.end.line == 25
    assert content.selection_range.start.line == 13
    assert content.selection_range.start.character == 15
    assert [
        (symbol.name, symbol.detail, symbol.kind) for symbol in content.children
    ] == [
        ("user.username", None, SymbolKind.Variable),
        ("if items", "if", SymbolKind.Operator),
        ("images/logo.png", "static", SymbolKind.File),
    ]

    user = content.children[0]
    assert [(symbol.name, symbol.detail, symbol.kind) for symbol in user.children] == [
        ("lower", None, SymbolKind.Function),
    ]


@pytest.mark.asyncio
async def test_document_symbols_are_empty_for_plain_html_template(
    client: LanguageClient,
):
    client.text_document_did_open(
        DidOpenTextDocumentParams(
            text_document=TextDocumentItem(
                uri=HEADER_TEMPLATE.as_uri(),
                language_id="htmldjango",
                version=1,
                text=HEADER_TEMPLATE.read_text(encoding="utf-8"),
            )
        )
    )

    result = await client.text_document_document_symbol_async(
        DocumentSymbolParams(
            text_document=TextDocumentIdentifier(uri=HEADER_TEMPLATE.as_uri()),
        )
    )

    assert result is not None
    assert len(result) == 0
