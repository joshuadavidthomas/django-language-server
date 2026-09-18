# djls-testing corpus

Corpus of real-world Django projects for grounding tests in reality.

This crate syncs pinned versions of Django, popular third-party libraries, and open-source Django projects as git archives, then provides helpers to enumerate and locate files within them.

Django project fact tests use minimal `django_settings_module` / `django_settings_modules` selectors in `manifest.toml`. Repos whose Django project lives below the checkout root also declare a relative `project_root`. These fields describe how to build the test project; expected apps, template directories, template tag modules, confidence, and reasons stay in tests or snapshots so the manifest does not become hand-written project-model data. The GH-401 multi-site monorepo shape lives under `fixtures/django-projects/` because the public corpus does not currently contain that exact real-world layout.

## Commands

```bash
cargo run -p djls-testing --bin corpus -- lock          # Resolve versions and update the lockfile
cargo run -p djls-testing --bin corpus -- sync          # Download repos from the lockfile
cargo run -p djls-testing --bin corpus -- sync -U       # Re-resolve versions then sync
cargo run -p djls-testing --bin corpus -- clean         # Remove all synced corpus data
cargo run -p djls-testing --bin corpus -- vendor-spec-fixtures         # Regenerate vendored djls-project spec fixtures
cargo run -p djls-testing --bin corpus -- vendor-spec-fixtures --check # Check vendored spec fixtures are current
```

## Per-repository environments

Source sync does not install dependencies. Each `[[repo]]` in `manifest.toml`
may contain its reviewed Python version, source roots, and dependency recipe.
Repositories are isolated from one another and from the developer's active
environment. Python 3.12 and `["."]` are the defaults. Python 3.6/3.7 recipes
retain the historical Django versions used by those corpus projects.

```bash
bash tools/corpus-python.sh                 # Bootstrap historical interpreters on Linux
just corpus environment sync healthchecks   # Resolve and install the declared inputs
just corpus environment check healthchecks  # Check readiness and build the project database
just corpus environment check               # Report every unconfigured/unready repository
just corpus environment extract healthchecks # Emit project-backed extraction as JSON lines
```

The source lock pins each checkout SHA. Dependency setup then follows the
upstream-owned source of truth: an upstream `uv.lock` or `poetry.lock`, or direct
metadata and requirements inputs from the pinned checkout. The corpus does not
generate or keep dependency lockfiles. If upstream leaves dependencies unlocked,
the corpus leaves them unlocked too, and each sync resolves them anew. Checkout
source is supplied separately to the analyzer through the configured source
roots.

Some packages omit Django from runtime metadata. Their recipes use upstream test
application requirements or `supplemental` requirements taken from a named tox
environment. When no upstream selection exists, setup adds the minimal supported
Django default. These are direct resolution inputs, not hand-written transitive
locks; the Django default applies only when the upstream inputs omit Django.
Optional `metadata_version` supplies setuptools-scm metadata absent from git
archives.

Environments live under `.corpus/environments/`. After a successful sync and
`uv pip check`, `provenance.json` records the source revision, effective recipe,
actual Python/platform, installer version, installed packages, direct-source
URLs, and provisioning time. This ignored diagnostic file is not used to install
anything. CI uploads it so unexpected snapshot changes can be compared against
the environment that produced them.

A separate disposable readiness stamp detects source, recipe, setup-policy, and
native-lock changes. It does not claim that unlocked dependencies are current.
`environment sync` clears and rebuilds the environment, refreshing dependency
resolution. Tests never install or update dependencies, and missing setup never
falls back to isolated extraction.

`Corpus::environment_database` exposes checkout source and the repository's own
site-packages to the existing project resolver, including declared settings and
nested project roots. It rejects environments that cannot resolve Django, even
if dependency installation succeeded. It does not execute Django settings or
start backing services. Source code remains project code: a Django checkout is
not relabelled as an installed dependency to obtain different extraction results.

Extraction and settings snapshots use these project-backed databases. Extraction
has one test per repository, so a library can be run individually:

```bash
cargo test -p djls-project --test corpus django-bootstrap3
cargo test -p djls-project --test corpus_settings healthchecks
uv run --no-project --with 'nox[uv]' nox --session corpus
```

The Nox `corpus` session rebuilds all environments before testing. CI runs it in
one Linux job, separate from the ambient Python/Django matrix. It caches source,
downloaded packages, and historical interpreters, but not resolved environments.
GeoNode is explicitly deferred for its native GDAL prerequisites and appears as
an ignored extraction test with a reason; explicitly requesting its environment
is an error, not a successful empty setup.

The Linux bootstrap uses python-build for Python 3.6.15 and 3.7.17 under
`.corpus/interpreters/`, which environment sync discovers automatically. It needs
a C compiler and OpenSSL, zlib, bzip2, readline, SQLite, libffi, and lzma development
headers. CI and `.agents/setup` install those prerequisites. Existing system or
pyenv interpreters can also satisfy a recipe without this bootstrap. Historical
environments are for testing corpus source, not production deployment.

When snapshots change unexpectedly, compare provenance and run the previous
analyzer against the same environment before blaming the code change. Review
ecosystem-driven changes rather than automatically accepting them. Django's own
checkout remains first-party source: these environments do not change the
separate `stringfilter` callable-evidence policy.

## Licensing

The corpus includes repos under various open-source licenses. Each repo's license text is stored in `licenses/{repo-name}` during the `lock` command.

If your project is included and you'd like it removed, open an issue or email and we'll take it out promptly.
