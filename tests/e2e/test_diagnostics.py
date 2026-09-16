from __future__ import annotations

import asyncio
from pathlib import Path
from textwrap import dedent

import pytest
import pytest_lsp
from lsprotocol import types
from pygls.protocol import default_converter
from pytest_lsp import ClientServerConfig
from pytest_lsp import LanguageClient
from pytest_lsp import client_capabilities
from pytest_lsp.client import DEFAULT_CLIENT_FEATURES
from pytest_lsp.client import create_work_done_progress
from pytest_lsp.client import register_lsp_features

from .conftest import SERVER_COMMAND
from .conftest import TEST_WORKSPACE
from .conftest import wait_for_project_load

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
NOT_IN_INSTALLED_APPS_TEMPLATE = (
    TEST_WORKSPACE / "templates" / "not_in_installed_apps.html"
)
EXPECTED_DIAGNOSTICS = {"S108", "S109", "S111", "S112", "S115", "S116"}
HELPER_SOURCE = dedent(
    """\
    def bits(token):
        return token.split_contents()[1:]
    """
)
UNUSED_SOURCE = "VALUE = 'unused'\n"


class ProgressControlledClient(LanguageClient):
    def __init__(self):
        super().__init__(converter_factory=default_converter)
        self.progress_creates_until_hold = 0
        self.progress_held = asyncio.Event()
        self.release_progress = asyncio.Event()

        async def create_progress(params: types.WorkDoneProgressCreateParams):
            create_work_done_progress(self, params)
            if self.progress_creates_until_hold > 0:
                self.progress_creates_until_hold -= 1
                if self.progress_creates_until_hold == 0:
                    self.progress_held.set()
                    await self.release_progress.wait()

        register_lsp_features(
            self,
            {
                **DEFAULT_CLIENT_FEATURES,
                types.WINDOW_WORK_DONE_PROGRESS_CREATE: create_progress,
            },
        )


@pytest_lsp.fixture(
    config=ClientServerConfig(
        server_command=SERVER_COMMAND, client_factory=ProgressControlledClient
    )
)
async def helper_client(lsp_client: ProgressControlledClient, tmp_path: Path):
    sources = {
        "settings.py": """
            INSTALLED_APPS = []
            TEMPLATES = [{
                'BACKEND': 'django.template.backends.django.DjangoTemplates',
                'OPTIONS': {'builtins': ['app.templatetags.tags']},
            }]
        """,
        "app/__init__.py": "",
        "app/templatetags/__init__.py": "",
        # Rule-only helpers must not be eager templatetags candidates.
        "app/helper.py": HELPER_SOURCE,
        "unused.py": UNUSED_SOURCE,
        "app/templatetags/tags.py": """
            from django import template
            from ..helper import bits

            register = template.Library()

            @register.tag(name='guarded')
            def guarded(parser, token):
                parts = bits(token)
                if len(parts) != 2:
                    raise template.TemplateSyntaxError('wrong count')
                return template.Node()
        """,
        "page.html": "{% guarded one two %}",
    }
    for name, source in sources.items():
        path = tmp_path / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(dedent(source), encoding="utf-8")

    await lsp_client.initialize_session(
        types.InitializeParams(
            capabilities=client_capabilities("visual-studio-code"),
            workspace_folders=[
                types.WorkspaceFolder(uri=tmp_path.as_uri(), name="helper_project")
            ],
            initialization_options={"django_settings_module": "settings"},
        )
    )
    await wait_for_project_load(lsp_client)

    yield lsp_client

    await lsp_client.shutdown_session()


@pytest.mark.asyncio
async def test_concurrent_mutation_does_not_deadlock_during_warmup(
    helper_client: ProgressControlledClient, tmp_path: Path
):
    # The helper's first open starts a full reload. Hold its third progress-create
    # request (warm-up) while it owns a live Salsa snapshot. Create requests have
    # no title; the title only arrives after the client acknowledges creation.
    helper_client.progress_creates_until_hold = 3
    helper_client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=(tmp_path / "app/helper.py").as_uri(),
                language_id="python",
                version=1,
                text=HELPER_SOURCE,
            )
        )
    )
    await asyncio.wait_for(helper_client.progress_held.wait(), timeout=5)
    try:
        # This mutation must not block the event loop: the server's progress
        # timeout needs to run and release the old snapshot before it can finish.
        uri = (tmp_path / "unused.py").as_uri()
        helper_client.text_document_did_open(
            types.DidOpenTextDocumentParams(
                text_document=types.TextDocumentItem(
                    uri=uri, language_id="python", version=1, text=UNUSED_SOURCE
                )
            )
        )
        await asyncio.wait_for(
            helper_client.text_document_document_symbol_async(
                types.DocumentSymbolParams(
                    text_document=types.TextDocumentIdentifier(uri=uri)
                )
            ),
            timeout=5,
        )
    finally:
        helper_client.release_progress.set()


@pytest.mark.asyncio
async def test_helper_edits_republish_diagnostics_without_template_changes(
    helper_client: ProgressControlledClient, tmp_path: Path
):
    for name, source in [
        ("app/helper.py", HELPER_SOURCE),
        ("unused.py", UNUSED_SOURCE),
    ]:
        helper_client.text_document_did_open(
            types.DidOpenTextDocumentParams(
                text_document=types.TextDocumentItem(
                    uri=(tmp_path / name).as_uri(),
                    language_id="python",
                    version=1,
                    text=source,
                )
            )
        )

    # Finish discovery before opening the template, so initial reload publications
    # cannot be mistaken for the diagnostic response to a later helper edit.
    await asyncio.wait_for(
        helper_client.text_document_document_symbol_async(
            types.DocumentSymbolParams(
                text_document=types.TextDocumentIdentifier(
                    uri=(tmp_path / "unused.py").as_uri()
                )
            )
        ),
        timeout=5,
    )

    template_uri = (tmp_path / "page.html").as_uri()
    pending = asyncio.wrap_future(
        helper_client.protocol.wait_for_notification(
            types.TEXT_DOCUMENT_PUBLISH_DIAGNOSTICS
        )
    )
    helper_client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=template_uri,
                language_id="htmldjango",
                version=1,
                text="{% guarded one two %}",
            )
        )
    )
    initial = await asyncio.wait_for(pending, timeout=5)
    assert initial.uri == template_uri
    assert initial.version == 1
    assert len(initial.diagnostics) == 0

    for name, version, source, expected_codes in [
        ("unused.py", 2, "VALUE = 'changed'\n", []),
        ("app/helper.py", 2, HELPER_SOURCE.replace("[1:]", "[2:]"), ["S117"]),
        ("app/helper.py", 3, HELPER_SOURCE, []),
    ]:
        # Register before sending; cached diagnostics cannot prove republishing.
        pending = asyncio.wrap_future(
            helper_client.protocol.wait_for_notification(
                types.TEXT_DOCUMENT_PUBLISH_DIAGNOSTICS
            )
        )
        helper_client.text_document_did_change(
            types.DidChangeTextDocumentParams(
                text_document=types.VersionedTextDocumentIdentifier(
                    uri=(tmp_path / name).as_uri(), version=version
                ),
                content_changes=[
                    types.TextDocumentContentChangeWholeDocument(text=source)
                ],
            )
        )
        publication = await asyncio.wait_for(pending, timeout=5)
        assert publication.uri == template_uri
        assert publication.version == 1
        assert [str(item.code) for item in publication.diagnostics] == expected_codes


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
