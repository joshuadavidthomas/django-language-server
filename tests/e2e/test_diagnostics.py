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
        self.hold_refresh = False
        self.refresh_held = asyncio.Event()
        self.release_refresh = asyncio.Event()
        self.refresh_count = 0
        self.publications: list[types.PublishDiagnosticsParams] = []

        async def create_progress(params: types.WorkDoneProgressCreateParams):
            create_work_done_progress(self, params)
            if self.progress_creates_until_hold > 0:
                self.progress_creates_until_hold -= 1
                if self.progress_creates_until_hold == 0:
                    self.progress_held.set()
                    await self.release_progress.wait()

        async def refresh_diagnostics(_params):
            self.refresh_count += 1
            if self.hold_refresh:
                self.refresh_held.set()
                await self.release_refresh.wait()

        def publish_diagnostics(params: types.PublishDiagnosticsParams):
            self.publications.append(params)
            DEFAULT_CLIENT_FEATURES[types.TEXT_DOCUMENT_PUBLISH_DIAGNOSTICS](
                self, params
            )

        register_lsp_features(
            self,
            {
                **DEFAULT_CLIENT_FEATURES,
                types.WINDOW_WORK_DONE_PROGRESS_CREATE: create_progress,
                types.WORKSPACE_DIAGNOSTIC_REFRESH: refresh_diagnostics,
                types.TEXT_DOCUMENT_PUBLISH_DIAGNOSTICS: publish_diagnostics,
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
        # Neither the session nor the reload worker may wait for optional progress.
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
            timeout=1,
        )
    finally:
        helper_client.release_progress.set()


@pytest_lsp.fixture(
    config=ClientServerConfig(
        server_command=SERVER_COMMAND, client_factory=ProgressControlledClient
    )
)
async def held_refresh_client(lsp_client: ProgressControlledClient, tmp_path: Path):
    (tmp_path / "settings.py").write_text("INSTALLED_APPS = []\nTEMPLATES = []\n")
    capabilities = client_capabilities("visual-studio-code")
    capabilities.text_document.diagnostic = types.DiagnosticClientCapabilities()
    capabilities.workspace.diagnostics = types.DiagnosticWorkspaceClientCapabilities(
        refresh_support=True
    )
    lsp_client.hold_refresh = True
    await lsp_client.initialize_session(
        types.InitializeParams(
            capabilities=capabilities,
            workspace_folders=[
                types.WorkspaceFolder(uri=tmp_path.as_uri(), name="audit")
            ],
            initialization_options={"django_settings_module": "settings"},
        )
    )
    await asyncio.wait_for(lsp_client.refresh_held.wait(), timeout=5)
    yield lsp_client
    lsp_client.release_refresh.set()
    await lsp_client.shutdown_session()


@pytest.mark.asyncio
async def test_refresh_response_does_not_block_mutation_or_next_generation(
    held_refresh_client: ProgressControlledClient, tmp_path: Path
):
    uri = (tmp_path / "new.py").as_uri()
    held_refresh_client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=uri, language_id="python", version=1, text="VALUE = 1\n"
            )
        )
    )
    # Opening a new Python file also requires a new discovery generation.
    await asyncio.wait_for(
        held_refresh_client.text_document_document_symbol_async(
            types.DocumentSymbolParams(
                text_document=types.TextDocumentIdentifier(uri=uri)
            )
        ),
        timeout=1,
    )
    assert not held_refresh_client.release_refresh.is_set()


@pytest.mark.asyncio
async def test_slow_refresh_keeps_one_rpc_and_one_coalesced_followup(
    held_refresh_client: ProgressControlledClient, tmp_path: Path
):
    for index in range(3):
        uri = (tmp_path / f"new_{index}.py").as_uri()
        held_refresh_client.text_document_did_open(
            types.DidOpenTextDocumentParams(
                text_document=types.TextDocumentItem(
                    uri=uri, language_id="python", version=1, text="VALUE = 1\n"
                )
            )
        )
        # Each new file requires a generation to finish while refresh is held.
        await asyncio.wait_for(
            held_refresh_client.text_document_document_symbol_async(
                types.DocumentSymbolParams(
                    text_document=types.TextDocumentIdentifier(uri=uri)
                )
            ),
            timeout=5,
        )

    # Exceed the former timeout: abandoning its future left the RPC outstanding.
    await asyncio.sleep(2.2)
    assert held_refresh_client.refresh_count == 1
    held_refresh_client.release_refresh.set()
    for _ in range(100):
        if held_refresh_client.refresh_count >= 2:
            break
        await asyncio.sleep(0.01)
    await asyncio.sleep(0.1)
    assert held_refresh_client.refresh_count == 2


@pytest.mark.asyncio
@pytest.mark.parametrize("reopen", [False, True])
@pytest.mark.parametrize("language", ["html", "htmldjango"])
async def test_push_diagnostics_coalesce_versions_waiting_for_readiness(
    helper_client: ProgressControlledClient, tmp_path: Path, reopen: bool, language: str
):
    # Hold discovery before intrinsic readiness. All edits finish before release,
    # making stale-version publication a deterministic failure, not a timing race.
    helper_client.progress_creates_until_hold = 1
    helper_client.workspace_did_change_configuration(
        types.DidChangeConfigurationParams(settings={})
    )
    await asyncio.wait_for(helper_client.progress_held.wait(), timeout=5)
    uri = (tmp_path / "page.html").as_uri()
    try:
        helper_client.text_document_did_open(
            types.DidOpenTextDocumentParams(
                text_document=types.TextDocumentItem(
                    uri=uri, language_id=language, version=0, text="{{ value"
                )
            )
        )
        for version in range(1, 41):
            helper_client.text_document_did_change(
                types.DidChangeTextDocumentParams(
                    text_document=types.VersionedTextDocumentIdentifier(
                        uri=uri, version=version
                    ),
                    content_changes=[
                        types.TextDocumentContentChangeWholeDocument(
                            text="{{ value" if version % 2 else "<p>ok</p>"
                        )
                    ],
                )
            )
        if reopen:
            helper_client.text_document_did_close(
                types.DidCloseTextDocumentParams(
                    text_document=types.TextDocumentIdentifier(uri=uri)
                )
            )
            helper_client.text_document_did_open(
                types.DidOpenTextDocumentParams(
                    text_document=types.TextDocumentItem(
                        uri=uri, language_id=language, version=0, text="{{ value"
                    )
                )
            )
        # Formatting bypasses readiness and is ordered after the source mutations.
        await helper_client.text_document_formatting_async(
            types.DocumentFormattingParams(
                text_document=types.TextDocumentIdentifier(uri=uri),
                options=types.FormattingOptions(tab_size=4, insert_spaces=True),
            )
        )
    finally:
        helper_client.release_progress.set()
    await asyncio.wait_for(
        helper_client.text_document_document_symbol_async(
            types.DocumentSymbolParams(
                text_document=types.TextDocumentIdentifier(uri=uri)
            )
        ),
        timeout=5,
    )
    expected_version = 0 if reopen else 40
    for _ in range(100):
        publications = [p for p in helper_client.publications if p.uri == uri]
        if any(p.version == expected_version for p in publications):
            break
        await asyncio.sleep(0.01)
    assert publications
    assert all(
        p.version == expected_version and len(p.diagnostics) == int(reopen)
        for p in publications
    )
    assert len(publications) <= 2  # Document scheduling and the completed reload.


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
