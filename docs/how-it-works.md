# How it works

An editor and a language server split the work of an IDE feature down the middle. This page walks through that split and the conversation between the two. It is written for anyone curious, and especially for contributors who have never worked on a language server before. For the server's internals (crates, the incremental computation database, the template pipeline), see [ARCHITECTURE.md](https://github.com/joshuadavidthomas/django-language-server/blob/main/ARCHITECTURE.md).

## The division of labor

Your editor never parses a Django template, and the server never draws a pixel.

The editor owns presentation: completion menus, squiggly underlines, hover popups, jumping between files. The server owns analysis: parsing templates, validating tags and filters, resolving `{% extends %}` chains, knowing which template tag libraries the project can load.

The two communicate only through JSON-RPC messages whose shapes the [Language Server Protocol](https://microsoft.github.io/language-server-protocol/) (LSP) defines. Neither side knows the other's internals. That separation is why one server can support every editor, and why one editor can support every language.

## A session, step by step

1. **The editor starts the server.** You open a Django project. The editor checks its LSP configuration, sees `djls` is registered for Django templates, and spawns `djls serve` as a child process. Everything from here on is JSON-RPC over that process's standard input and output. This is also why running `djls serve` by hand in a terminal looks like a hang: the server is waiting on stdin for an editor that isn't there.

2. **They negotiate capabilities.** The editor sends an `initialize` request describing what it supports; the server replies with what it can do: completion, hover, diagnostics, go to definition, and so on. The editor never asks for a feature the server didn't declare.

3. **The server learns the project.** djls discovers the Python environment and reads the Django settings module, `INSTALLED_APPS`, template directories, and the Python source of template tag libraries. All of this is static analysis; it never imports or runs project code.

4. **You open a template.** The editor sends a `textDocument/didOpen` notification carrying the file's full text. The server parses and validates it, then pushes back `textDocument/publishDiagnostics`: a list of ranges and messages. The editor turns each one into a squiggle.

5. **You type.** Each edit produces a `textDocument/didChange` notification. The server re-analyzes and publishes fresh diagnostics. The server works from the editor's in-memory buffer, not the file on disk, so unsaved changes are fully visible to it.

6. **You ask for something.** Hover, completion, and go to definition are request/response pairs. The editor sends the document and cursor position; the server answers from its analysis; the editor renders the result. A `textDocument/hover` response becomes a popup, a `textDocument/completion` response becomes a menu.

7. **You quit.** The editor sends `shutdown` and `exit`, and the server process ends.

Notice the two kinds of message: notifications flow one way and expect no reply (`didOpen`, `didChange`, `publishDiagnostics`), while requests expect an answer (`hover`, `completion`, `definition`).

## When something misbehaves

The split tells you where to look. How a result is displayed (the popup's styling, when the completion menu opens, which keybinding triggers a jump) is editor behavior, configured in the editor. What the result contains (why a tag is reported unknown, why definition jumped to the wrong file, why a completion is missing) is server behavior, and the answer lives in this repository.

The seam between them is observable. Most editors can log or trace LSP traffic, showing every message in both directions; on the server side, the [`debug` setting](configuration/index.md#debug) turns on djls logging. Comparing the two views usually locates a bug in one sentence: either the editor asked for the wrong thing, or the server gave the wrong answer.

## Where the server's knowledge comes from

Every answer the server gives is derived from statically reading the project: settings, `INSTALLED_APPS`, template directories, and the Python source that registers template tags and filters. [Template Validation](template-validation.md) describes that analysis and its limits, and [Configuration](configuration/index.md) covers the cases where discovery needs help.

## Going deeper

- [ARCHITECTURE.md](https://github.com/joshuadavidthomas/django-language-server/blob/main/ARCHITECTURE.md) — crate boundaries, the Salsa database, and the template pipeline
- [CONTEXT.md](https://github.com/joshuadavidthomas/django-language-server/blob/main/CONTEXT.md) — the domain glossary for the codebase
- [LSP specification](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/) — the full protocol
- [CONTRIBUTING.md](https://github.com/joshuadavidthomas/django-language-server/blob/main/CONTRIBUTING.md) — how to get a development environment running
