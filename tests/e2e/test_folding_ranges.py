from __future__ import annotations

import pytest
from lsprotocol.types import DidOpenTextDocumentParams
from lsprotocol.types import FoldingRangeKind
from lsprotocol.types import FoldingRangeParams
from lsprotocol.types import TextDocumentIdentifier
from lsprotocol.types import TextDocumentItem
from pytest_lsp import LanguageClient

from .conftest import TEST_WORKSPACE

BASE_TEMPLATE = TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "base.html"
COMMENT_TEMPLATE = (
    TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "tags" / "comment.html"
)
HEADER_TEMPLATE = TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "header.html"


@pytest.mark.asyncio
async def test_folding_ranges_include_nested_template_regions(
    client: LanguageClient,
):
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

    result = await client.text_document_folding_range_async(
        FoldingRangeParams(
            text_document=TextDocumentIdentifier(uri=BASE_TEMPLATE.as_uri()),
        )
    )

    assert result is not None
    assert {(range.start_line, range.end_line, range.kind) for range in result} == {
        (13, 25, FoldingRangeKind.Region),
        (16, 22, FoldingRangeKind.Region),
    }


@pytest.mark.asyncio
async def test_folding_ranges_include_comment_blocks(client: LanguageClient):
    client.text_document_did_open(
        DidOpenTextDocumentParams(
            text_document=TextDocumentItem(
                uri=COMMENT_TEMPLATE.as_uri(),
                language_id="htmldjango",
                version=1,
                text=COMMENT_TEMPLATE.read_text(encoding="utf-8"),
            )
        )
    )

    result = await client.text_document_folding_range_async(
        FoldingRangeParams(
            text_document=TextDocumentIdentifier(uri=COMMENT_TEMPLATE.as_uri()),
        )
    )

    assert result is not None
    assert {(range.start_line, range.end_line, range.kind) for range in result} == {
        (3, 6, FoldingRangeKind.Comment),
        (9, 11, FoldingRangeKind.Comment),
    }


@pytest.mark.asyncio
async def test_folding_ranges_are_empty_for_plain_html_template(
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

    result = await client.text_document_folding_range_async(
        FoldingRangeParams(
            text_document=TextDocumentIdentifier(uri=HEADER_TEMPLATE.as_uri()),
        )
    )

    assert result is not None
    assert len(result) == 0
