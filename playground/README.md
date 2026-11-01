# djls playground

A browser playground for django-language-server: a small, editable Django project in Monaco, with
djls compiled to WebAssembly (`crates/djls-wasm`) running in a Web Worker. It's a SvelteKit app
(Svelte 5 runes, Tailwind v4, shadcn-svelte) built as a static site with `adapter-static`: there is
no server.

## Running it

Requirements: Rust (the `wasm32-unknown-unknown` target is added for you), Node.js with pnpm,
Python 3, and `wasm-bindgen-cli` at the exact version of the `wasm-bindgen` crate in `Cargo.lock`.
`mise install` provides the pinned CLI.

```bash
just playground wasm    # djls-wasm (wasm-release profile) + bindings + Django 5.2 into playground/djls-wasm/
just playground dev     # Vite dev server; pass Vite flags after it, e.g. --host 0.0.0.0
just playground check   # svelte-check
just playground lint    # oxfmt --check, oxlint, eslint
just playground format  # oxfmt
just playground build   # static site in playground/build/
```

A cold build of `djls-wasm` takes several minutes, and its fat-LTO link needs about 1 GB of
memory: on a small machine, stop the dev server and browsers while it runs. Rerun
`just playground wasm` after changing Rust code; the dev server picks up the new bindings on
reload.

The wasm embeds no Django. `just playground wasm` re-zips each
`crates/djls-project/vendor/django-*.zip` uncompressed (stored) next to the module, and the worker
registers one release with `registerDjango` before opening the project: `DJANGO` in
`src/lib/project.ts`, 5.2 for the example. Stored entries let HTTP compression shrink 5.2 to about
1 MB brotli, against 1.6 MB for the deflated zip. wasm-opt is deliberately not run: it grows the
module after compression.

## Packages

The sidebar's Packages list holds PyPI requirements (`name` or `name==version`). For each one the
worker fetches the newest (or pinned) release's `py3-none-any` wheel from PyPI, unpacks its `.py`,
`.html`, and `.txt` files into `.venv/lib/python3.12/site-packages/` with a `pyvenv.cfg`, and reruns
Django Discovery. There is no dependency resolution: list every package. A package with no wheel
or no pure-Python wheel fails with that reason. The example installs `django-crispy-forms`, which
`blog/templates/blog/post_form.html` loads.

Templates get document formatting (`djangofmt`) through Monaco's Format Document.

## Layout

- `src/lib/djls/worker/djls.ts` is the only module that loads the wasm or Django. Change how
  either is fetched there. `pypi.ts` next to it installs packages.
- `src/lib/djls/worker/worker.ts` owns the `Workspace` and reruns Django Discovery lazily when
  settings or the project's file set change. `protocol.ts` is the typed request contract, and
  `src/lib/djls/client.ts` is the main-thread side. A wasm trap restarts the worker.
- `src/lib/monaco/` is the editor: `setup.ts` (features, worker, themes), `django-template.ts`
  (the Monarch grammar), and `providers.ts`, which maps LSP JSON to markers, completion, hover,
  definition, and folding. Project models use `file:///project/<path>` URIs, matching the URIs
  djls returns. Definition targets in bundled Django open read-only.
- `src/lib/playground.svelte.ts` is the `Playground` state class (files, diagnostics, status),
  set in context by the page and read with `getPlayground()`.
- `src/lib/components/` holds the file tree, Problems panel, new-file dialog, and mode toggle;
  `src/lib/components/ui/` is shadcn-svelte as its CLI writes it
  (`pnpm dlx shadcn-svelte@latest add <name>`).
- `src/lib/project.ts` holds the example project: files and packages. Edits persist in
  `localStorage`; **Reset** restores the example.

## Not done yet

Deployment and CI are not set up. They would need:

- A CI job that installs Rust with the wasm32 target, the pinned `wasm-bindgen-cli`, Node, and
  pnpm, then runs `just playground build`, `check`, and `lint`. Cache `target/` because the wasm
  build is slow.
- A static host for `playground/build/`, such as GitHub Pages or Cloudflare Pages. It must serve
  `.wasm` as `application/wasm` and should compress both the wasm (about 5.3 MB raw, 1.8 MB gzip)
  and the stored Django zip (about 6 MB raw, 1 MB brotli).
