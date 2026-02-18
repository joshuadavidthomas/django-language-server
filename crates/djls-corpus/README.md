# djls-corpus

Corpus of real-world Django projects for grounding tests in reality.

This crate syncs pinned versions of Django, popular third-party libraries, and open-source Django projects as git archives, then provides helpers to enumerate and locate files within them.

## Commands

```bash
cargo run -p djls-corpus -- lock          # Resolve versions and update the lockfile
cargo run -p djls-corpus -- sync          # Download repos from the lockfile
cargo run -p djls-corpus -- sync -U       # Re-resolve versions then sync
cargo run -p djls-corpus -- clean         # Remove all synced corpus data
```

## Licensing

The corpus includes repos under various open-source licenses. Each repo's license text is stored in `licenses/{repo-name}` during the `lock` command.

If your project is included and you'd like it removed, open an issue or email and we'll take it out promptly.
