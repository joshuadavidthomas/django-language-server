# Django Language Server Testing Framework

This directory contains the end-to-end testing framework for the Django Language Server.

## Overview

The testing framework is designed to test the Django Language Server against:

- Multiple Python versions (3.9, 3.10, 3.11, 3.12, 3.13)
- Multiple Django versions (4.2, 5.0, 5.1)
- Different LSP clients (VS Code, Neovim)

## Directory Structure

- `e2e/`: End-to-end tests for the language server
- `fixtures/`: Test fixtures, including Django project generation
- `clients/`: Client-specific tests for different editors
- `conftest.py`: Pytest configuration and fixtures
- `requirements-test.txt`: Test dependencies

## Running Tests

### Local Testing

To run the tests locally, you can use tox:

```bash
# Install tox
pip install tox

# Run tests with the default Python and Django versions
tox

# Run tests with a specific Python and Django version
tox -e py311-django50

# Run tests with a specific test file
tox -- tests/e2e/test_basic_functionality.py
```

### GitHub Actions

The tests are automatically run on GitHub Actions for all supported Python and Django versions. The workflow is defined in `.github/workflows/e2e-tests.yml`.

## Adding New Tests

### Adding a New End-to-End Test

1. Create a new test file in the `e2e/` directory
2. Use the existing fixtures from `conftest.py`
3. Write your test using the pytest-asyncio framework

Example:

```python
@pytest.mark.asyncio
async def test_my_feature(lsp_client, django_project):
    # Test code here
    pass
```

### Adding a New Client Test

1. Create a new test file in the `clients/` directory
2. Create fixtures for the client configuration
3. Write tests for the client integration

## Test Fixtures

### Django Project

The `django_project` fixture creates a Django project with:

- A basic project structure
- A sample app with views and templates
- Template files with Django template tags

### Language Server

The `language_server_process` fixture starts the Django Language Server in TCP mode for testing.

### LSP Client

The `lsp_client` fixture creates an LSP client connected to the language server for sending requests and receiving responses.

## Extending the Framework

### Testing with Additional Django Versions

To test with additional Django versions, update the `envlist` in `tox.ini` and the matrix in `.github/workflows/e2e-tests.yml`.

### Testing with Additional Clients

To test with additional LSP clients, add a new test file in the `clients/` directory and update the matrix in `.github/workflows/e2e-tests.yml`.

### Testing Additional Features

To test additional features of the language server, add new test files in the `e2e/` directory.