from __future__ import annotations

import pytest
from lsprotocol import types
from pytest_lsp import LanguageClient

from .conftest import TEST_WORKSPACE

TEMPLATE = (
    TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "tags" / "scoping.html"
)
FIRST_PARTY_UNLOADED_TEMPLATE = (
    TEST_WORKSPACE
    / "djls_app"
    / "templates"
    / "djls_app"
    / "tags"
    / "first_party_unloaded.html"
)
NOT_IN_INSTALLED_APPS_TEMPLATE = TEST_WORKSPACE / "templates" / "not_in_installed_apps.html"
EXPECTED_DIAGNOSTICS = {"S108", "S109", "S111", "S112", "S115", "S116"}


@pytest.mark.asyncio
async def test_publish_diagnostics_for_existing_template(client: LanguageClient):
    client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=TEMPLATE.as_uri(),
                language_id="htmldjango",
                version=1,
                text=TEMPLATE.read_text(encoding="utf-8"),
            )
        )
    )

    while not client.diagnostics.get(TEMPLATE.as_uri()):
        await client.wait_for_notification(types.TEXT_DOCUMENT_PUBLISH_DIAGNOSTICS)

    assert {
        str(diagnostic.code)
        for diagnostic in client.diagnostics[TEMPLATE.as_uri()]
        if diagnostic.code
    } == EXPECTED_DIAGNOSTICS


@pytest.mark.asyncio
async def test_publish_diagnostics_for_unloaded_first_party_tag(
    client: LanguageClient,
):
    client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=FIRST_PARTY_UNLOADED_TEMPLATE.as_uri(),
                language_id="htmldjango",
                version=1,
                text=FIRST_PARTY_UNLOADED_TEMPLATE.read_text(encoding="utf-8"),
            )
        )
    )

    while not client.diagnostics.get(FIRST_PARTY_UNLOADED_TEMPLATE.as_uri()):
        await client.wait_for_notification(types.TEXT_DOCUMENT_PUBLISH_DIAGNOSTICS)

    assert {
        str(diagnostic.code)
        for diagnostic in client.diagnostics[FIRST_PARTY_UNLOADED_TEMPLATE.as_uri()]
        if diagnostic.code
    } == {"S109"}


@pytest.mark.asyncio
async def test_publish_diagnostics_for_inactive_django_contrib_library(
    client: LanguageClient,
):
    client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=NOT_IN_INSTALLED_APPS_TEMPLATE.as_uri(),
                language_id="htmldjango",
                version=1,
                text=NOT_IN_INSTALLED_APPS_TEMPLATE.read_text(encoding="utf-8"),
            )
        )
    )

    while not client.diagnostics.get(NOT_IN_INSTALLED_APPS_TEMPLATE.as_uri()):
        await client.wait_for_notification(types.TEXT_DOCUMENT_PUBLISH_DIAGNOSTICS)

    diagnostics = client.diagnostics[NOT_IN_INSTALLED_APPS_TEMPLATE.as_uri()]
    load_diagnostics = [
        diagnostic for diagnostic in diagnostics if diagnostic.range.start.line == 0
    ]
    tag_diagnostics = [
        diagnostic for diagnostic in diagnostics if diagnostic.range.start.line == 2
    ]

    assert [str(diagnostic.code) for diagnostic in load_diagnostics] == ["S121"]
    assert "django.contrib.flatpages" in load_diagnostics[0].message
    assert [str(diagnostic.code) for diagnostic in tag_diagnostics] == ["S118"]
    assert "django.contrib.flatpages" in tag_diagnostics[0].message
    assert "S108" not in {str(diagnostic.code) for diagnostic in diagnostics}


@pytest.mark.asyncio
async def test_pull_diagnostics_for_existing_template(neovim_client: LanguageClient):
    neovim_client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=TEMPLATE.as_uri(),
                language_id="htmldjango",
                version=1,
                text=TEMPLATE.read_text(encoding="utf-8"),
            )
        )
    )

    report = await neovim_client.text_document_diagnostic_async(
        types.DocumentDiagnosticParams(
            text_document=types.TextDocumentIdentifier(uri=TEMPLATE.as_uri()),
        )
    )

    assert report.kind == "full"
    assert {
        str(diagnostic.code) for diagnostic in report.items if diagnostic.code
    } == EXPECTED_DIAGNOSTICS
    assert len(neovim_client.diagnostics) == 0, "Server should not publish diagnostics"
