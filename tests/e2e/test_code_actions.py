from __future__ import annotations

import pytest
from lsprotocol import types
from pytest_lsp import LanguageClient

from .conftest import TEST_WORKSPACE

FIRST_PARTY_UNLOADED_TEMPLATE = (
    TEST_WORKSPACE
    / "djls_app"
    / "templates"
    / "djls_app"
    / "tags"
    / "first_party_unloaded.html"
)
AMBIGUOUS_UNLOADED_TEMPLATE = (
    TEST_WORKSPACE
    / "djls_app"
    / "templates"
    / "djls_app"
    / "tags"
    / "ambiguous_unloaded.html"
)
BLOCK_MISMATCH_TEMPLATE = (
    TEST_WORKSPACE
    / "djls_app"
    / "templates"
    / "djls_app"
    / "tags"
    / "block_mismatch.html"
)


@pytest.mark.asyncio
async def test_offers_load_quick_fix_for_unloaded_tag(client: LanguageClient):
    uri = FIRST_PARTY_UNLOADED_TEMPLATE.as_uri()
    client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=uri,
                language_id="htmldjango",
                version=1,
                text=FIRST_PARTY_UNLOADED_TEMPLATE.read_text(encoding="utf-8"),
            )
        )
    )

    while not client.diagnostics.get(uri):
        await client.wait_for_notification(types.TEXT_DOCUMENT_PUBLISH_DIAGNOSTICS)

    diagnostic = next(
        diagnostic for diagnostic in client.diagnostics[uri] if str(diagnostic.code) == "S109"
    )
    actions = await client.text_document_code_action_async(
        types.CodeActionParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            range=diagnostic.range,
            context=types.CodeActionContext(
                diagnostics=[diagnostic],
                only=[types.CodeActionKind.QuickFix],
            ),
        )
    )

    assert actions is not None
    action = next(action for action in actions if action.title == "Add '{% load djls_app_tags %}'")
    assert action.kind == types.CodeActionKind.QuickFix
    assert action.is_preferred is True
    assert action.edit is not None
    assert action.edit.changes is not None
    edits = action.edit.changes[uri]
    assert len(edits) == 1
    assert edits[0].range.start == types.Position(line=0, character=0)
    assert edits[0].range.end == types.Position(line=0, character=0)
    assert edits[0].new_text == "{% load djls_app_tags %}\n"


@pytest.mark.asyncio
async def test_offers_load_quick_fixes_for_ambiguous_unloaded_tag(client: LanguageClient):
    uri = AMBIGUOUS_UNLOADED_TEMPLATE.as_uri()
    client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=uri,
                language_id="htmldjango",
                version=1,
                text=AMBIGUOUS_UNLOADED_TEMPLATE.read_text(encoding="utf-8"),
            )
        )
    )

    while not client.diagnostics.get(uri):
        await client.wait_for_notification(types.TEXT_DOCUMENT_PUBLISH_DIAGNOSTICS)

    diagnostic = next(
        diagnostic for diagnostic in client.diagnostics[uri] if str(diagnostic.code) == "S110"
    )
    actions = await client.text_document_code_action_async(
        types.CodeActionParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            range=diagnostic.range,
            context=types.CodeActionContext(
                diagnostics=[diagnostic],
                only=[types.CodeActionKind.QuickFix],
            ),
        )
    )

    assert actions is not None
    assert [action.title for action in actions] == [
        "Add '{% load alpha_tags %}'",
        "Add '{% load beta_tags %}'",
    ]
    assert [action.is_preferred for action in actions] == [None, None]
    for action, library in zip(actions, ["alpha_tags", "beta_tags"], strict=True):
        assert action.kind == types.CodeActionKind.QuickFix
        assert action.edit is not None
        assert action.edit.changes is not None
        edits = action.edit.changes[uri]
        assert len(edits) == 1
        assert edits[0].range.start == types.Position(line=0, character=0)
        assert edits[0].range.end == types.Position(line=0, character=0)
        assert edits[0].new_text == f"{{% load {library} %}}\n"


@pytest.mark.asyncio
async def test_offers_rename_quick_fix_for_unmatched_block_name(client: LanguageClient):
    uri = BLOCK_MISMATCH_TEMPLATE.as_uri()
    client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=uri,
                language_id="htmldjango",
                version=1,
                text=BLOCK_MISMATCH_TEMPLATE.read_text(encoding="utf-8"),
            )
        )
    )

    while not client.diagnostics.get(uri):
        await client.wait_for_notification(types.TEXT_DOCUMENT_PUBLISH_DIAGNOSTICS)

    diagnostic = next(
        diagnostic for diagnostic in client.diagnostics[uri] if str(diagnostic.code) == "S103"
    )
    actions = await client.text_document_code_action_async(
        types.CodeActionParams(
            text_document=types.TextDocumentIdentifier(uri=uri),
            range=diagnostic.range,
            context=types.CodeActionContext(
                diagnostics=[diagnostic],
                only=[types.CodeActionKind.QuickFix],
            ),
        )
    )

    assert actions is not None
    action = next(action for action in actions if action.title == "Rename closing block to 'content'")
    assert action.kind == types.CodeActionKind.QuickFix
    assert action.is_preferred is True
    assert action.edit is not None
    assert action.edit.changes is not None
    edits = action.edit.changes[uri]
    assert len(edits) == 1
    assert edits[0].range.start == types.Position(line=1, character=12)
    assert edits[0].range.end == types.Position(line=1, character=17)
    assert edits[0].new_text == "content"
