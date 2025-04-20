"""
Basic end-to-end tests for django-language-server.
"""

from __future__ import annotations

import asyncio
import os
from pathlib import Path

import pytest
from pygls.client import JsonRPCClient
from pygls.protocol import LanguageServerProtocol
from pygls.types import (
    CompletionParams,
    DidOpenTextDocumentParams,
    Position,
    TextDocumentIdentifier,
    TextDocumentItem,
)


@pytest.mark.asyncio
async def test_server_initialization(lsp_client: JsonRPCClient):
    """Test that the server initializes correctly."""
    # Server should already be initialized by the fixture
    assert lsp_client.protocol.state == LanguageServerProtocol.STATE.INITIALIZED


@pytest.mark.asyncio
async def test_template_tag_completion(
    lsp_client: JsonRPCClient, django_project: Path
):
    """Test template tag completion."""
    # Find a template file
    template_files = list(django_project.glob("**/templates/**/*.html"))
    assert template_files, "No template files found"
    
    template_file = template_files[0]
    template_uri = f"file://{template_file}"
    
    # Read the template content
    with open(template_file, "r") as f:
        template_content = f.read()
    
    # Open the document in the language server
    await lsp_client.text_document_did_open_async(
        DidOpenTextDocumentParams(
            text_document=TextDocumentItem(
                uri=template_uri,
                language_id="django-html",
                version=1,
                text=template_content,
            )
        )
    )
    
    # Wait a moment for the server to process the document
    await asyncio.sleep(1)
    
    # Find a position after {% to test completion
    lines = template_content.split("\n")
    for i, line in enumerate(lines):
        if "{%" in line:
            position = Position(
                line=i,
                character=line.index("{%") + 2,
            )
            break
    else:
        # If no {% found, add one at the end and update the document
        position = Position(line=len(lines), character=2)
        template_content += "\n{% "
        
        # Update the document
        await lsp_client.text_document_did_change_async(
            {
                "textDocument": {
                    "uri": template_uri,
                    "version": 2,
                },
                "contentChanges": [
                    {
                        "text": template_content,
                    }
                ],
            }
        )
        await asyncio.sleep(1)
    
    # Request completions
    completions = await lsp_client.completion_async(
        CompletionParams(
            text_document=TextDocumentIdentifier(uri=template_uri),
            position=position,
        )
    )
    
    # Check that we got some completions
    assert completions is not None
    assert len(completions.items) > 0
    
    # Check that common Django template tags are included
    tag_labels = [item.label for item in completions.items]
    common_tags = ["for", "if", "block", "extends", "include"]
    
    for tag in common_tags:
        assert any(tag in label for label in tag_labels), f"Tag '{tag}' not found in completions"


@pytest.mark.asyncio
async def test_django_settings_detection(
    lsp_client: JsonRPCClient, django_project: Path
):
    """Test that the server correctly detects Django settings."""
    # This is a basic test to ensure the server can detect Django settings
    # We'll use the workspace/executeCommand API to check this
    
    result = await lsp_client.execute_command_async(
        {
            "command": "djls.debug.projectInfo",
            "arguments": [],
        }
    )
    
    # The result should contain information about the Django project
    assert result is not None
    assert "django" in str(result).lower()
    assert "settings" in str(result).lower()