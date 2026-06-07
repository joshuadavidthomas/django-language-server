from __future__ import annotations

import pytest
from lsprotocol.types import CompletionItemKind
from lsprotocol.types import CompletionParams
from lsprotocol.types import DidOpenTextDocumentParams
from lsprotocol.types import InsertTextFormat
from lsprotocol.types import TextDocumentIdentifier
from lsprotocol.types import TextDocumentItem
from pytest_lsp import LanguageClient

from .conftest import TEST_WORKSPACE
from .utils import position_after

BASE_TEMPLATE = TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "base.html"
LOAD_TEMPLATE = (
    TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "tags" / "load.html"
)


@pytest.mark.asyncio
async def test_completes_available_template_tags(client: LanguageClient):
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

    result = await client.text_document_completion_async(
        CompletionParams(
            text_document=TextDocumentIdentifier(uri=BASE_TEMPLATE.as_uri()),
            position=position_after(BASE_TEMPLATE, "{% sta"),
        )
    )

    assert result is not None
    static = next(item for item in result if item.label == "static")
    assert static.detail == "{% load static %}"
    assert static.filter_text == "static"


@pytest.mark.asyncio
async def test_completes_load_library_names(client: LanguageClient):
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

    result = await client.text_document_completion_async(
        CompletionParams(
            text_document=TextDocumentIdentifier(uri=BASE_TEMPLATE.as_uri()),
            position=position_after(BASE_TEMPLATE, "{% load sta"),
        )
    )

    assert result is not None
    static = next(item for item in result if item.label == "static")
    assert static.kind == CompletionItemKind.Module
    assert static.insert_text_format == InsertTextFormat.PlainText
    assert static.detail.startswith("Django template library")


@pytest.mark.asyncio
async def test_completes_selective_load_symbols(client: LanguageClient):
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

    result = await client.text_document_completion_async(
        CompletionParams(
            text_document=TextDocumentIdentifier(uri=LOAD_TEMPLATE.as_uri()),
            position=position_after(LOAD_TEMPLATE, "{% load tra"),
        )
    )

    assert result is not None
    trans = next(item for item in result if item.label == "trans")
    assert trans.kind == CompletionItemKind.Function
    assert trans.insert_text_format == InsertTextFormat.PlainText


@pytest.mark.asyncio
async def test_completes_filters_in_template_variables(client: LanguageClient):
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

    result = await client.text_document_completion_async(
        CompletionParams(
            text_document=TextDocumentIdentifier(uri=BASE_TEMPLATE.as_uri()),
            position=position_after(BASE_TEMPLATE, "|lo"),
        )
    )

    assert result is not None
    lower = next(item for item in result if item.label == "lower")
    assert lower.kind == CompletionItemKind.Function
    assert lower.detail == "builtin filter"
    assert lower.filter_text == "lower"


@pytest.mark.asyncio
async def test_completes_structural_end_tags(client: LanguageClient):
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

    result = await client.text_document_completion_async(
        CompletionParams(
            text_document=TextDocumentIdentifier(uri=BASE_TEMPLATE.as_uri()),
            position=position_after(BASE_TEMPLATE, "Django Test App{% end"),
        )
    )

    assert result is not None
    endblock = next(item for item in result if item.label == "endblock")
    assert endblock.kind == CompletionItemKind.Keyword
    assert endblock.detail == "End tag for block"
    assert endblock.sort_text == "00_endblock"
