# djls-testing corpus

Corpus of real-world Django projects for grounding tests in reality.

This crate syncs pinned versions of Django, popular third-party libraries, and open-source Django projects as git archives, then provides helpers to enumerate and locate files within them.

Django project fact tests use minimal `django_settings_module` / `django_settings_modules` selectors in `manifest.toml`. The manifest identifies which corpus entries or local fixtures should be analyzed; expected apps, template directories, template tag modules, confidence, and reasons stay in tests or snapshots so the manifest does not become hand-written project-model data. The GH-401 multi-site monorepo shape lives under `fixtures/django-projects/` because the public corpus does not currently contain that exact real-world layout.

## Commands

```bash
cargo run -p djls-testing --bin corpus -- lock          # Resolve versions and update the lockfile
cargo run -p djls-testing --bin corpus -- sync          # Download repos from the lockfile
cargo run -p djls-testing --bin corpus -- sync -U       # Re-resolve versions then sync
cargo run -p djls-testing --bin corpus -- clean         # Remove all synced corpus data
```

## Licensing

The corpus includes repos under various open-source licenses. Each repo's license text is stored in `licenses/{repo-name}` during the `lock` command.

If your project is included and you'd like it removed, open an issue or email and we'll take it out promptly.
