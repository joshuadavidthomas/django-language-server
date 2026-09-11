# Installation

This page is a reference for every way to install the server. Setting up for the first time? The [getting started guide](getting-started.md) is the shortest path.

## Requirements

Django Language Server needs a discoverable project environment with a supported Python and Django version. An LSP client is required only for editor features through `djls serve`; [`djls check`](cli.md#djls-check) runs in a terminal. See [Versioning](versioning.md) for supported versions and the version support policy.

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

Installing it adds the `djls` command-line tool to your environment. See the [command-line reference](cli.md) for `djls check` and `djls serve`.

### Homebrew

Install the standalone macOS binary from the personal Homebrew tap:

```bash
brew install joshuadavidthomas/homebrew/django-language-server
```

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
    # Set the release and platform: linux-x64, linux-arm64,
    # darwin-x64, or darwin-arm64.
    VERSION="6.1.0"
    PLATFORM="linux-x64"
    ARCHIVE="django-language-server-v${VERSION}-${PLATFORM}"

    curl -LO "https://github.com/joshuadavidthomas/django-language-server/releases/download/v${VERSION}/${ARCHIVE}.tar.gz"
    tar -xzf "${ARCHIVE}.tar.gz"

    # Move the binary to a location in your PATH.
    sudo mv "${ARCHIVE}/djls" /usr/local/bin/
    ```

=== "Windows"

    ```powershell
    # Set this to the release you want to install.
    \Version = "6.1.0"
    $Archive = "django-language-server-v$Version-windows-x64.zip"

    # Download and extract the Windows x64 archive.
    Invoke-WebRequest -Uri "https://github.com/joshuadavidthomas/django-language-server/releases/download/v$Version/$Archive" -OutFile $Archive
    Expand-Archive -Path $Archive -DestinationPath .

    # Move the binary to a location in your PATH (requires admin),
    # or add its extracted directory to your PATH.
    Move-Item -Path "django-language-server-v$Version-windows-x64\djls.exe" -Destination "$env:LOCALAPPDATA\Programs\djls.exe"
    ```

## Building from source

Build and install directly from source using Rust's cargo:

```bash
cargo install --git https://github.com/joshuadavidthomas/django-language-server djls --locked
```

This requires a Rust toolchain (see [rust-toolchain.toml](https://github.com/joshuadavidthomas/django-language-server/tree/main/rust-toolchain.toml) for the required version) and will compile the language server from source.
