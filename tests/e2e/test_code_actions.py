from __future__ import annotations

import ast
from urllib.parse import parse_qs
from urllib.parse import urlparse

import pytest
from lsprotocol import types
from pytest_lsp import LanguageClient

from .conftest import TEST_WORKSPACE
from .conftest import UNREADABLE_WORKSPACE

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
UNREADABLE_LIBRARY_TEMPLATE = (
    UNREADABLE_WORKSPACE
    / "unreadable_app"
    / "templates"
    / "unreadable_app"
    / "unreadable.html"
)
UNREADABLE_LIBRARY_SOURCE = (
    UNREADABLE_WORKSPACE
    / "unreadable_app"
    / "templatetags"
    / "unreadable_tags.py"
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


@pytest.mark.asyncio
async def test_reports_unreadable_registration_in_external_issue_url(
    unreadable_client: LanguageClient,
):
    client = unreadable_client
    uri = UNREADABLE_LIBRARY_TEMPLATE.as_uri()
    client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=uri,
                language_id="htmldjango",
                version=1,
                text=UNREADABLE_LIBRARY_TEMPLATE.read_text(encoding="utf-8"),
            )
        )
    )

    while not client.diagnostics.get(uri):
        await client.wait_for_notification(types.TEXT_DOCUMENT_PUBLISH_DIAGNOSTICS)

    diagnostic = next(
        diagnostic for diagnostic in client.diagnostics[uri] if str(diagnostic.code) == "S124"
    )
    assert diagnostic.severity == types.DiagnosticSeverity.Hint
    assert diagnostic.range == types.Range(
        start=types.Position(line=0, character=8),
        end=types.Position(line=0, character=23),
    )
    assert diagnostic.related_information is not None
    assert len(diagnostic.related_information) == 1
    related = diagnostic.related_information[0]
    assert related.location.uri == UNREADABLE_LIBRARY_SOURCE.as_uri()
    assert related.location.range.start == types.Position(line=11, character=0)
    assert related.message == "the registered name cannot be resolved"

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
    action = next(
        action
        for action in actions
        if action.title == "Report unreadable registration to django-language-server"
    )
    # lsprotocol 2025.0.0 produces a bare Command whose `command` field is the
    # Python repr of the nested command dict, rather than a command-bearing CodeAction.
    assert isinstance(action, types.Command)
    command = ast.literal_eval(action.command)
    assert isinstance(command, dict)
    assert command["command"] == "djls.reportUnreadableRegistration"
    assert command["arguments"] == [
        {
            "module": "unreadable_app.templatetags.unreadable_tags",
            "file": UNREADABLE_LIBRARY_SOURCE.as_uri(),
            "line": 12,
            "shape": "the registered name cannot be resolved",
            "count": 1,
        }
    ]

    shown_before = len(client.shown_documents)
    await client.workspace_execute_command_async(
        types.ExecuteCommandParams(
            command=command["command"],
            arguments=command["arguments"],
        )
    )
    assert len(client.shown_documents) == shown_before + 1
    shown = client.shown_documents[-1]
    assert shown.external is True
    issue_url = urlparse(shown.uri)
    assert issue_url.scheme == "https"
    assert issue_url.netloc == "github.com"
    assert issue_url.path == (
        "/joshuadavidthomas/django-language-server/issues/new"
    )
    query = parse_qs(issue_url.query)
    assert query["title"][0].startswith("Unreadable tag registration")
    assert "register.simple_tag(takes_context=True)" in query["body"][0]
    assert (
        "unrecognized tags and filters from that library are not reported"
        in query["body"][0]
    )
