---
title: Zed
---

# Zed

The Django Language Server can be used with [Zed](https://zed.dev) via the [zed-django](https://github.com/joshuadavidthomas/zed-django) extension.

## Installation

> [!NOTE]
> The extension is currently awaiting approval for the official Zed extension store ([relevant PR](https://github.com/zed-industries/extensions/pull/3525)).
>
> In the meantime, you can install it as a dev extension. To do so, you will need a Rust toolchain available on your machine, the easiest and recommended way is via [rustup](https://rust-lang.org/tools/install). See the Zed docs on [developing an extension locally](https://zed.dev/docs/extensions/developing-extensions#developing-an-extension-locally) for more information.
>
> Once you have Rust available, you can follow these steps:
>
> 1. Clone the [zed-django](https://github.com/joshuadavidthomas/zed-django) repository locally
> 2. Open the Extensions panel (`zed: extensions` in the command palette or `ctrl-shift-x`/`cmd-shift-x`)
> 3. Click "Install Dev Extension" in the upper right corner and select the cloned repo folder

Install the extension from the Zed extensions directory:

1. Open the Extensions panel (`zed: extensions` in the command palette or `ctrl-shift-x`/`cmd-shift-x`)
2. Search for "Django"
3. Click "Install"

The extension uses the Django Language Server by default and will automatically download it if not already installed.

## Documentation

For configuration, file associations, and advanced usage, see the [zed-django repository](https://github.com/joshuadavidthomas/zed-django).
