# Configuration System Plan (djls)

**Version:** 1.0
**Date:** 2023-10-27

## 1. Overview

This document outlines the plan to refactor the configuration system for the Django Language Server (djls). The goal is to establish a clear separation between fixed, built-in knowledge of standard Django template tags and user-provided configuration (custom tags, server settings).

## 2. Core Principles

-   **Built-in TagSpecs:** Define standard Django syntax. Reside within `djls-templates`. Loaded internally, not overridable by users.
-   **User Configuration:** Defines custom TagSpecs and other server settings (`debug`, paths, etc.). Loaded by `djls-config` from project files (`djls.toml`, `pyproject.toml`) and potentially specific environment variables (for non-TagSpec settings).
-   **Separation:** `djls-templates` is unaware of user config files. `djls-config` loads user config but not built-ins.
-   **Dynamic Reloading:** `djls-server` must reload user configuration upon LSP notification (`workspace/didChangeConfiguration`).

## 3. Crate Responsibilities

-   **`djls-templates`:**
    -   Defines `TagSpecs`, `TagSpec`, `EndTag` structs.
    -   Contains default `.toml` files (e.g., `django.toml`) in `./tagspecs`.
    -   Loads its own built-in specs internally (e.g., via `include_dir!`, `lazy_static`/`once_cell`).
    -   Provides access via `get_builtin_specs() -> &'static TagSpecs`.
    -   Contains template parsing/validation logic using `TagSpecs`.
    -   **No** user config loading logic. **No** dependency on `djls-config`.
-   **`djls-config`:**
    -   Defines `struct Config { custom_tagspecs: TagSpecs, debug: bool, ... }`.
    -   Depends on `djls-templates` for the `TagSpecs` type definition.
    -   Uses the `config` crate to load user settings from `djls.toml`, `.djls.toml`, `pyproject.toml` (respecting priority and `[tool.djls]` section).
    -   Manually applies specific, documented environment variable overrides *after* loading files (for non-TagSpec fields like `debug`).
    -   Provides `Config::load(project_root) -> Result<Config, ConfigError>`.
    -   **No** built-in spec loading logic.
-   **`djls-server`:**
    -   Depends on `djls-templates` and `djls-config`.
    -   Holds loaded user config: `Arc<RwLock<djls_config::Config>>`.
    -   On init/workspace load, calls `djls_config::Config::load()`.
    -   For parsing:
        -   Gets built-ins via `djls_templates::get_builtin_specs()`.
        -   Gets user config via `self.config.read()`.
        -   Merges built-ins (cloned) and `user_config.custom_tagspecs`.
        -   Passes merged specs to `djls_templates::parse()`.
    -   Handles `workspace/didChangeConfiguration` by calling `djls_config::Config::load()` and updating its stored config.

## 4. Implementation Checklist

-   [ ] **Phase 1: Refactor `djls-templates`**
    -   [ ] Add `lazy_static` or `once_cell` dependency.
    -   [ ] Implement internal `load_embedded_builtins()` using `include_dir!`.
    -   [ ] Implement static `BUILTIN_TAGSPECS` using `Lazy::new()`.
    -   [ ] Implement `pub fn get_builtin_specs() -> &'static TagSpecs`.
    -   [ ] Ensure `TagSpecs`, `TagSpec`, `EndTag` have necessary derives (`Serialize`, `Deserialize`, `Default`, `Clone`, `Debug`, `PartialEq`).
    -   [ ] Remove `load_user_specs`, `load_all`, `load_from_toml`, `extract_specs`, `load_builtin_specs_from`.
    -   [ ] Remove associated tests for removed functions.
    -   [ ] Adjust dependencies (add `toml`, `include_dir`, `lazy_static`/`once_cell`; remove `config` if present; ensure no `djls-config` dep).
-   [ ] **Phase 2: Implement `djls-config`**
    -   [ ] Create crate `crates/djls-config`.
    -   [ ] Add crate to workspace `Cargo.toml`.
    -   [ ] Define dependencies (`djls-templates`, `config`, `serde`, `log`, `thiserror`, `toml`).
    -   [ ] Define `Config` struct (with `custom_tagspecs: TagSpecs`, `debug`, etc.).
    -   [ ] Define `ConfigError` enum.
    -   [ ] Implement `Config::load(project_root)` using `config` crate for files.
    -   [ ] Add manual environment variable override logic within `Config::load`.
    -   [ ] Add unit tests for `Config::load` (file priority, env vars, errors).
-   [ ] **Phase 3: Integrate into `djls-server`**
    -   [ ] Add `djls-config` dependency.
    -   [ ] Update `DjangoLanguageServer` state to hold `Arc<RwLock<djls_config::Config>>`.
    -   [ ] Update initialization/workspace loading to call `djls_config::Config::load()`.
    -   [ ] Update parsing logic to call `get_builtin_specs()`, read user config, merge specs, and pass to `djls_templates::parse()`.
    -   [ ] Implement `workspace/didChangeConfiguration` handler to reload config and update state.
-   [ ] **Phase 4: Cleanup & Final Testing**
    -   [ ] Remove any old config-loading code from `djls-server` or other crates.
    -   [ ] Run all tests (`cargo test --workspace`).
    -   [ ] Perform manual integration testing if applicable.

## 5. Open Questions / Considerations

-   Error handling strategy for failed built-in loading (should the server fail to start?). Currently assumed to log/error.
-   Specific TOML structure for `custom_tagspecs` within user files (e.g., `[custom_tagspecs.my_tag]`). Ensure `Config` struct matches.
-   Need to clearly document which environment variables are supported for overrides.
-   Testing strategy for `djls-templates` built-in loading (uses `include_dir!`).
