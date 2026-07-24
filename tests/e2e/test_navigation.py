from __future__ import annotations

from pathlib import Path

import pytest
from django.templatetags import static as django_static
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
FIRST_PARTY_LOAD_TEMPLATE = (
    TEST_WORKSPACE
    / "djls_app"
    / "templates"
    / "djls_app"
    / "tags"
    / "first_party_load.html"
)
FIRST_PARTY_TAG_LIBRARY = (
    TEST_WORKSPACE / "djls_app" / "templatetags" / "djls_app_tags.py"
)
DJANGO_STATIC_LIBRARY = Path(django_static.__file__)


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
    assert len(result) == 1
    link = result[0]
    assert link.origin_selection_range == Range(
        start=Position(line=0, character=12),
        end=Position(line=0, character=30),
    )
    assert link.target_uri == BASE_TEMPLATE.as_uri()
    assert link.target_range == Range(
        start=Position(line=0, character=0),
        end=Position(line=0, character=0),
    )
    assert link.target_selection_range == Range(
        start=Position(line=0, character=0),
        end=Position(line=0, character=0),
    )


@pytest.mark.asyncio
async def test_goto_definition_for_parent_template_block(client: LanguageClient):
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
            position=position_in(HOME_TEMPLATE, "title %}"),
        )
    )

    assert result is not None
    assert len(result) == 1
    link = result[0]
    assert link.origin_selection_range == Range(
        start=Position(line=2, character=9),
        end=Position(line=2, character=14),
    )
    assert link.target_uri == BASE_TEMPLATE.as_uri()
    assert link.target_range.start == position_in(BASE_TEMPLATE, "{% block title")
    parent_name = position_in(BASE_TEMPLATE, "title %}")
    assert link.target_selection_range == Range(
        start=parent_name,
        end=Position(
            line=parent_name.line,
            character=parent_name.character + len("title"),
        ),
    )


@pytest.mark.asyncio
async def test_goto_definition_for_root_template_block(client: LanguageClient):
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
            position=position_in(BASE_TEMPLATE, "title %}"),
        )
    )

    assert result is not None
    assert len(result) == 1
    link = result[0]
    parent_name = position_in(BASE_TEMPLATE, "title %}")
    assert link.origin_selection_range == Range(
        start=parent_name,
        end=Position(
            line=parent_name.line,
            character=parent_name.character + len("title"),
        ),
    )
    assert link.target_uri == BASE_TEMPLATE.as_uri()
    assert link.target_range.start == position_in(BASE_TEMPLATE, "{% block title")
    assert link.target_selection_range == link.origin_selection_range


@pytest.mark.asyncio
async def test_find_references_for_template_block(client: LanguageClient):
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
            position=position_in(HOME_TEMPLATE, "title %}"),
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
        (BASE_TEMPLATE.as_uri(), 7, 15, 7, 20),
        (HOME_TEMPLATE.as_uri(), 2, 9, 2, 14),
    }


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
    assert len(result) == 1
    link = result[0]
    assert link.origin_selection_range == Range(
        start=Position(line=11, character=16),
        end=Position(line=11, character=36),
    )
    assert link.target_uri == HEADER_TEMPLATE.as_uri()
    assert link.target_range == Range(
        start=Position(line=0, character=0),
        end=Position(line=0, character=0),
    )
    assert link.target_selection_range == Range(
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
        (HOME_TEMPLATE.as_uri(), 0, 12, 0, 30),
        (EXTENDS_TAG_TEMPLATE.as_uri(), 2, 12, 2, 30),
        (EXTENDS_TAG_TEMPLATE.as_uri(), 10, 12, 10, 30),
    }


async def goto_first_party_definition(client: LanguageClient, name: str):
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
    return await client.text_document_definition_async(
        DefinitionParams(
            text_document=TextDocumentIdentifier(uri=FIRST_PARTY_LOAD_TEMPLATE.as_uri()),
            position=position_in(FIRST_PARTY_LOAD_TEMPLATE, name),
        )
    )


@pytest.mark.asyncio
async def test_goto_definition_for_template_library(client: LanguageClient):
    result = await goto_first_party_definition(client, "djls_app_tags")

    assert result is not None
    assert len(result) == 1
    link = result[0]
    assert link.origin_selection_range == Range(
        start=Position(line=1, character=8),
        end=Position(line=1, character=21),
    )
    assert link.target_uri == FIRST_PARTY_TAG_LIBRARY.as_uri()
    assert link.target_range == Range(
        start=Position(line=0, character=0),
        end=Position(line=0, character=0),
    )
    assert link.target_selection_range == link.target_range


@pytest.mark.asyncio
async def test_goto_definition_for_template_tag(client: LanguageClient):
    result = await goto_first_party_definition(client, "djls_greeting")

    assert result is not None
    assert len(result) == 1
    link = result[0]
    assert link.origin_selection_range == Range(
        start=Position(line=2, character=3),
        end=Position(line=2, character=16),
    )
    assert link.target_uri == FIRST_PARTY_TAG_LIBRARY.as_uri()
    assert link.target_range.start == Position(line=7, character=0)
    assert link.target_selection_range == Range(
        start=Position(line=8, character=4),
        end=Position(line=8, character=17),
    )


@pytest.mark.asyncio
async def test_goto_definition_for_template_filter(client: LanguageClient):
    result = await goto_first_party_definition(client, "djls_shout")

    assert result is not None
    assert len(result) == 1
    link = result[0]
    assert link.origin_selection_range == Range(
        start=Position(line=3, character=11),
        end=Position(line=3, character=21),
    )
    assert link.target_uri == FIRST_PARTY_TAG_LIBRARY.as_uri()
    assert link.target_range.start == Position(line=12, character=0)
    assert link.target_selection_range == Range(
        start=Position(line=13, character=4),
        end=Position(line=13, character=14),
    )


@pytest.mark.asyncio
async def test_goto_definition_distinguishes_static_library_from_tag(
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

    library_result = await client.text_document_definition_async(
        DefinitionParams(
            text_document=TextDocumentIdentifier(uri=BASE_TEMPLATE.as_uri()),
            position=position_in(BASE_TEMPLATE, "static %}"),
        )
    )
    tag_result = await client.text_document_definition_async(
        DefinitionParams(
            text_document=TextDocumentIdentifier(uri=BASE_TEMPLATE.as_uri()),
            position=position_in(BASE_TEMPLATE, "static 'images/logo.png'"),
        )
    )

    assert library_result is not None
    assert len(library_result) == 1
    library_link = library_result[0]
    assert library_link.target_uri == DJANGO_STATIC_LIBRARY.as_uri()
    assert library_link.target_range == Range(
        start=Position(line=0, character=0),
        end=Position(line=0, character=0),
    )
    assert library_link.target_selection_range == library_link.target_range

    assert tag_result is not None
    assert len(tag_result) == 1
    tag_link = tag_result[0]
    assert tag_link.target_uri == DJANGO_STATIC_LIBRARY.as_uri()
    assert tag_link.target_range.start == position_in(
        DJANGO_STATIC_LIBRARY, '@register.tag("static")'
    )
    do_static = position_in(DJANGO_STATIC_LIBRARY, "do_static(parser, token)")
    assert tag_link.target_selection_range == Range(
        start=do_static,
        end=Position(
            line=do_static.line,
            character=do_static.character + len("do_static"),
        ),
    )


@pytest.mark.asyncio
async def test_find_references_ignores_tag_name(client: LanguageClient):
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

    references = await client.text_document_references_async(
        ReferenceParams(
            text_document=TextDocumentIdentifier(uri=HOME_TEMPLATE.as_uri()),
            position=position_in(HOME_TEMPLATE, "extends"),
            context=ReferenceContext(include_declaration=True),
        )
    )

    assert references is None
