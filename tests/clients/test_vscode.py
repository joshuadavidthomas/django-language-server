"""
Tests for VS Code integration with django-language-server.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

from fixtures.create_django_project import create_django_project, cleanup_django_project


@pytest.fixture(scope="module")
def vscode_extension_dir():
    """Create a temporary directory for a VS Code extension."""
    temp_dir = tempfile.mkdtemp()
    extension_dir = Path(temp_dir) / "django-language-server-vscode"
    extension_dir.mkdir(exist_ok=True)
    
    # Create package.json
    package_json = {
        "name": "django-language-server-vscode",
        "displayName": "Django Language Server",
        "description": "VS Code extension for Django Language Server",
        "version": "0.0.1",
        "engines": {
            "vscode": "^1.60.0"
        },
        "categories": [
            "Programming Languages"
        ],
        "activationEvents": [
            "onLanguage:django-html",
            "onLanguage:python"
        ],
        "main": "./extension.js",
        "contributes": {
            "languages": [
                {
                    "id": "django-html",
                    "aliases": ["Django HTML", "django-html"],
                    "extensions": [".html", ".djhtml"],
                    "configuration": "./language-configuration.json"
                }
            ],
            "configuration": {
                "title": "Django Language Server",
                "properties": {
                    "djangoLanguageServer.path": {
                        "type": "string",
                        "default": "djls",
                        "description": "Path to the Django Language Server executable"
                    }
                }
            }
        },
        "scripts": {
            "test": "echo \"Error: no test specified\" && exit 1"
        }
    }
    
    with open(extension_dir / "package.json", "w") as f:
        json.dump(package_json, f, indent=2)
    
    # Create extension.js
    extension_js = """
const vscode = require('vscode');
const { LanguageClient, TransportKind } = require('vscode-languageclient/node');

let client;

function activate(context) {
    const serverPath = vscode.workspace.getConfiguration('djangoLanguageServer').get('path');
    
    const serverOptions = {
        command: serverPath,
        args: [],
        transport: TransportKind.stdio
    };
    
    const clientOptions = {
        documentSelector: [
            { scheme: 'file', language: 'django-html' },
            { scheme: 'file', language: 'python' }
        ],
        synchronize: {
            fileEvents: vscode.workspace.createFileSystemWatcher('**/*.{py,html}')
        }
    };
    
    client = new LanguageClient(
        'djangoLanguageServer',
        'Django Language Server',
        serverOptions,
        clientOptions
    );
    
    client.start();
}

function deactivate() {
    if (client) {
        return client.stop();
    }
    return undefined;
}

module.exports = {
    activate,
    deactivate
};
"""
    
    with open(extension_dir / "extension.js", "w") as f:
        f.write(extension_js)
    
    # Create language-configuration.json
    language_config = {
        "comments": {
            "blockComment": ["{% comment %}", "{% endcomment %}"]
        },
        "brackets": [
            ["{%", "%}"],
            ["{{", "}}"],
            ["(", ")"],
            ["[", "]"],
            ["{", "}"]
        ],
        "autoClosingPairs": [
            { "open": "{%", "close": " %}" },
            { "open": "{{", "close": " }}" },
            { "open": "(", "close": ")" },
            { "open": "[", "close": "]" },
            { "open": "{", "close": "}" }
        ],
        "surroundingPairs": [
            { "open": "{%", "close": "%}" },
            { "open": "{{", "close": "}}" },
            { "open": "(", "close": ")" },
            { "open": "[", "close": "]" },
            { "open": "{", "close": "}" }
        ]
    }
    
    with open(extension_dir / "language-configuration.json", "w") as f:
        json.dump(language_config, f, indent=2)
    
    yield extension_dir
    
    # Clean up
    import shutil
    shutil.rmtree(temp_dir)


def test_vscode_extension_structure(vscode_extension_dir):
    """Test that the VS Code extension structure is valid."""
    assert (vscode_extension_dir / "package.json").exists()
    assert (vscode_extension_dir / "extension.js").exists()
    assert (vscode_extension_dir / "language-configuration.json").exists()


def test_vscode_extension_package_json(vscode_extension_dir):
    """Test that the package.json is valid."""
    with open(vscode_extension_dir / "package.json", "r") as f:
        package_json = json.load(f)
    
    assert package_json["name"] == "django-language-server-vscode"
    assert "djangoLanguageServer.path" in package_json["contributes"]["configuration"]["properties"]


# This test is a placeholder for actual VS Code integration testing
# In a real implementation, you would use something like vscode-test
# to launch VS Code with the extension and test it
@pytest.mark.skip(reason="Requires VS Code to be installed")
def test_vscode_extension_with_language_server(vscode_extension_dir):
    """Test the VS Code extension with the language server."""
    # This would require VS Code to be installed and a way to programmatically
    # interact with it, which is beyond the scope of this example
    pass