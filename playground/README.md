# djls playground

A browser playground for django-language-server: a small, editable Django project in Monaco, with
djls compiled to WebAssembly (`crates/djls-wasm`) running in a Web Worker.

## Running it

Requirements: Rust (the `wasm32-unknown-unknown` target is added for you), Node.js, and
`wasm-bindgen-cli` at the exact version of the `wasm-bindgen` crate in `Cargo.lock`. `mise install`
provides the pinned CLI.

```bash
just playground wasm   # cargo build -p djls-wasm for wasm32 + wasm-bindgen into playground/djls-wasm/
just playground dev    # Vite dev server; pass Vite flags after it, e.g. --host 0.0.0.0
just playground check  # type-check
just playground build  # static site in playground/dist/
```

A cold release build of `djls-wasm` takes about five minutes. Rerun `just playground wasm` after
changing Rust code; the dev server picks up the new bindings on reload.

## Layout

- `src/worker/djls.ts` is the only module that loads the wasm. Change how the wasm or the bundled
  Django sources are fetched there.
- `src/worker/worker.ts` owns the `Workspace` and reruns Django Discovery lazily when settings or
  the project's file set change. `src/worker/protocol.ts` is the typed request contract, and
  `src/client.ts` is the main-thread side. A wasm trap restarts the worker.
- `src/providers.ts` maps LSP JSON to Monaco: markers, completion, hover, definition, and folding.
  Project models use `file:///project/<path>` URIs, matching the URIs djls returns. Definition
  targets in bundled Django open read-only.
- `src/django-template.ts` is the Monarch grammar for Django templates.
- `src/project.ts` holds the example project. Edits persist in `localStorage`; **Reset** restores
  the example.

## Not done yet

Deployment and CI are not set up. They would need:

- A CI job that installs Rust with the wasm32 target, the pinned `wasm-bindgen-cli`, and Node, then
  runs `just playground build` and `just playground check`. Cache `target/` because the wasm build is
  slow.
- A static host for `playground/dist/`, such as GitHub Pages or Cloudflare Pages. The host must
  serve `.wasm` as `application/wasm` and should compress it: the wasm is about 15 MB, roughly 7.7 MB
  gzipped.
