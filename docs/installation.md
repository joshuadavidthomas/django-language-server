# Installation

## Requirements

A client that supports the Language Server Protocol (LSP) is required, along with a supported version of Python and Django. See [Versioning](versioning.md) for the full list of supported versions and the version support policy.

## Try it out

To try the language server without installing using [`uvx`](https://docs.astral.sh/uv/guides/tools/#running-tools):

```bash
uvx --from django-language-server djls serve
```

!!! note

    The server will automatically detect and use your project's Python environment when you open a Django project. It needs access to your project's Django installation and other dependencies, but should be able to find these regardless of where the server itself is installed.

## Package manager

The language server is published to PyPI with pre-built wheels for the following platforms:

- **Linux**: x86_64, aarch64 (both glibc and musl)
- **macOS**: x86_64, aarch64
- **Windows**: x64
- **Source distribution**: Available for other platforms

Installing it adds the `djls` command-line tool to your environment.

### System-wide tool installation

Install it globally in an isolated environment using `uv` or `pipx`:

```bash
# Using uv
uv tool install django-language-server

# Or using pipx
pipx install django-language-server
```

### Install with pip

Install from PyPI using pip:

```bash
pip install django-language-server
```

Or add as a development dependency with uv:

```bash
uv add --dev django-language-server
```

## Standalone binaries

Standalone binaries are available for macOS, Linux, and Windows from [GitHub Releases](https://github.com/joshuadavidthomas/django-language-server/releases).

=== "Linux/macOS"

    ```bash
    # Download the latest release for your platform
    # Example for Linux x64:
    curl -LO https://github.com/joshuadavidthomas/django-language-server/releases/latest/download/django-language-server-VERSION-linux-x64.tar.gz

    # Extract the archive
    tar -xzf django-language-server-VERSION-linux-x64.tar.gz

    # Move the binary to a location in your PATH
    sudo mv django-language-server-VERSION-linux-x64/djls /usr/local/bin/
    ```

=== "Windows"

    ```powershell
    # Download the latest release for your platform
    # Example for Windows x64:
    Invoke-WebRequest -Uri "https://github.com/joshuadavidthomas/django-language-server/releases/latest/download/django-language-server-VERSION-windows-x64.zip" -OutFile "django-language-server-VERSION-windows-x64.zip"

    # Extract the archive
    Expand-Archive -Path "django-language-server-VERSION-windows-x64.zip" -DestinationPath .

    # Move the binary to a location in your PATH (requires admin)
    # Or add the directory containing djls.exe to your PATH
    Move-Item -Path "django-language-server-VERSION-windows-x64\djls.exe" -Destination "$env:LOCALAPPDATA\Programs\djls.exe"
    ```

## Building from source

Build and install directly from source using Rust's cargo:

```bash
cargo install --git https://github.com/joshuadavidthomas/django-language-server djls --locked
```

This requires a Rust toolchain (see [rust-toolchain.toml](https://github.com/joshuadavidthomas/django-language-server/tree/main/rust-toolchain.toml) for the required version) and will compile the language server from source.
