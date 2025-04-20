"""
Pytest configuration for django-language-server tests.
"""

from __future__ import annotations

import asyncio
import os
import subprocess
import sys
from pathlib import Path

import pytest

from fixtures.create_django_project import create_django_project, cleanup_django_project


@pytest.fixture(scope="session")
def django_version() -> str:
    """
    Get the Django version to use for testing.
    
    Returns:
        Django version string (e.g., "4.2", "5.0")
    """
    return os.environ.get("DJANGO_VERSION", "5.0")


@pytest.fixture(scope="session")
def django_project(django_version: str) -> Path:
    """
    Create a Django project for testing.
    
    Args:
        django_version: Django version to use
        
    Returns:
        Path to the Django project
    """
    project_dir = create_django_project(django_version=django_version)
    yield project_dir
    cleanup_django_project(project_dir)


@pytest.fixture(scope="session")
def language_server_path() -> Path:
    """
    Get the path to the django-language-server executable.
    
    Returns:
        Path to the django-language-server executable
    """
    # Try to find the executable in the current environment
    try:
        result = subprocess.run(
            [sys.executable, "-m", "djls", "--version"],
            capture_output=True,
            text=True,
            check=True,
        )
        return Path(sys.executable).parent / "djls"
    except (subprocess.CalledProcessError, FileNotFoundError):
        # If not found, build it
        subprocess.run(
            [sys.executable, "-m", "pip", "install", "maturin"],
            check=True,
        )
        subprocess.run(
            ["maturin", "develop", "--release"],
            check=True,
            cwd=Path(__file__).parent.parent,
        )
        return Path(sys.executable).parent / "djls"


@pytest.fixture
async def language_server_process(language_server_path: Path, django_project: Path):
    """
    Start the language server process.
    
    Args:
        language_server_path: Path to the language server executable
        django_project: Path to the Django project
        
    Yields:
        Process object for the language server
    """
    # Start the language server in TCP mode
    process = subprocess.Popen(
        [
            str(language_server_path),
            "--tcp",
            "--port",
            "8888",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        cwd=django_project,
        env={**os.environ, "PYTHONPATH": str(django_project)},
    )
    
    # Wait a moment for the server to start
    await asyncio.sleep(2)
    
    yield process
    
    # Clean up
    process.terminate()
    process.wait(timeout=5)


@pytest.fixture
async def lsp_client(language_server_process):
    """
    Create an LSP client connected to the language server.
    
    Args:
        language_server_process: Process object for the language server
        
    Yields:
        LSP client
    """
    from pygls.client import JsonRPCClient
    
    client = JsonRPCClient(tcp=True)
    await client.connect("localhost", 8888)
    
    # Initialize the client
    await client.initialize_async(
        processId=os.getpid(),
        rootUri=f"file://{os.getcwd()}",
        capabilities={},
    )
    
    yield client
    
    # Clean up
    await client.shutdown_async()
    await client.exit_async()
    await client.close()