# Configuration System Plan (djls)

**Version:** 2.0 (Phased Approach)
**Date:** 2023-10-28

## 1. Overview

This document outlines the plan to refactor the configuration system for the Django Language Server (djls). The goal is to establish a clear separation between fixed, built-in knowledge of standard Django template tags and user-provided configuration (custom tags, server settings). This refactor will be done in phases to create manageable Pull Requests (PRs).

## 2. Core Principles (Unchanged)

-   **Built-in TagSpecs:** Define standard Django syntax. Reside within `djls-templates`. Loaded internally, not overridable by users.
-   **User Configuration:** Defines custom TagSpecs and other server settings (`debug`, paths, etc.). Loaded by `djls-config` from project files (`djls.toml`, `pyproject.toml`) and potentially specific environment variables (for non-TagSpec settings).
-   **Separation:** `djls-templates` is unaware of user config files. `djls-config` loads user config but not built-ins.
-   **Dynamic Reloading:** `djls-server` must reload user configuration upon LSP notification (`workspace/didChangeConfiguration`).

## 3. Crate Responsibilities (Final State)

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

## 4. Phased Implementation Plan

### Phase 1 (PR 1): Introduce `djls-config` & Basic Settings

-   **Goal:** Create the `djls-config` crate, implement loading for a simple setting (`debug: bool`), and integrate basic loading/reloading into `djls-server`.
-   **`djls-config`:**
    -   [ ] Create crate `crates/djls-config`.
    -   [ ] Add crate to workspace `Cargo.toml`.
    -   [ ] Define dependencies (`config`, `serde`, `log`, `thiserror`, `toml`). **No `djls-templates` dependency yet.**
    -   [ ] Define `Config` struct (with only `debug: bool` for now).
    -   [ ] Define `ConfigError` enum.
    -   [ ] Implement `Config::load(project_root)` using `config` crate for files (`djls.toml`, `.djls.toml`, `pyproject.toml[tool.djls]`).
    -   [ ] Add manual environment variable override logic for `DJLS_DEBUG`.
    -   [ ] Add unit tests for `Config::load` (file priority, env vars, errors for `debug`).
-   **`djls-server`:**
    -   [ ] Add `djls-config` dependency.
    -   [ ] Update `DjangoLanguageServer` state to hold `Arc<RwLock<djls_config::Config>>`.
    -   [ ] Update initialization/workspace loading to call `djls_config::Config::load()` and store the result. Log errors.
    -   [ ] Implement `workspace/didChangeConfiguration` handler to reload config and update state.
    -   [ ] **No changes to parsing logic yet.**

### Phase 2 (PR 2): Load User `TagSpecs` via `djls-config`

-   **Goal:** Extend `djls-config` to load `custom_tagspecs` from user files. Update `djls-server` to merge these with *temporary* empty built-ins. Remove old user loading code from `djls-templates`.
-   **`djls-config`:**
    -   [ ] Add `djls-templates` dependency.
    -   [ ] Add `custom_tagspecs: TagSpecs` field to `Config` struct (using `TagSpecs` from `djls-templates`).
    -   [ ] Update `Config::load` to parse `custom_tagspecs` from a dedicated section (e.g., `[tool.djls.custom_tagspecs]` or `[custom_tagspecs]`) in user config files.
        -   Need to decide on the exact TOML structure and implement parsing.
        -   Use `TagSpecs::deserialize` or similar.
    -   [ ] Add tests for loading `custom_tagspecs`.
-   **`djls-templates`:**
    -   [ ] Remove `load_user_specs`, `load_all`, `load_from_toml`, `extract_specs`.
    -   [ ] Remove associated tests for removed functions.
    -   [ ] Keep `load_builtin_specs` and `load_builtin_specs_from` for now (will be replaced in Phase 3).
-   **`djls-server`:**
    -   [ ] Update parsing logic:
        -   Get user config via `self.config.read()`.
        -   Create *temporary* empty `TagSpecs` for built-ins.
        -   Merge the empty built-ins and `user_config.custom_tagspecs`.
        -   Pass merged specs to `djls_templates::parse()`.

### Phase 3 (PR 3): Implement Built-in `TagSpecs` Loading

-   **Goal:** Implement the *actual* built-in `TagSpecs` loading within `djls-templates` (using `include_dir!`, `lazy_static`, etc.) and update `djls-server` to merge correctly.
-   **`djls-templates`:**
    -   [ ] Add `lazy_static` or `once_cell` dependency.
    -   [ ] Add `include_dir` dependency.
    -   [ ] Implement internal `load_embedded_builtins()` using `include_dir!` to read `.toml` files from `./tagspecs`.
        -   This function will parse the TOML content and build a `HashMap<String, TagSpec>`.
        -   Handle potential errors during parsing.
    -   [ ] Implement static `BUILTIN_TAGSPECS: Lazy<TagSpecs>` using `Lazy::new()` and `load_embedded_builtins()`.
    -   [ ] Implement `pub fn get_builtin_specs() -> &'static TagSpecs`.
    -   [ ] Ensure `TagSpecs`, `TagSpec`, `EndTag` have necessary derives (`Serialize`, `Deserialize`, `Default`, `Clone`, `Debug`, `PartialEq`).
    -   [ ] Remove `load_builtin_specs` and `load_builtin_specs_from`.
    -   [ ] Remove associated tests for removed functions.
    -   [ ] Add tests for `get_builtin_specs()` (verify some known tags exist).
-   **`djls-server`:**
    -   [ ] Update parsing logic:
        -   Get built-ins via `djls_templates::get_builtin_specs()`.
        -   Get user config via `self.config.read()`.
        -   Merge built-ins (cloned) and `user_config.custom_tagspecs`.
        -   Pass merged specs to `djls_templates::parse()`.

### Phase 4 (PR 4): Cleanup & Final Testing

-   **Goal:** Remove any remaining old code, ensure all tests pass, and perform final checks.
-   **All Crates:**
    -   [ ] Remove any old config-loading code or unused functions/variables.
    -   [ ] Review dependencies.
    -   [ ] Run all tests (`cargo test --workspace`).
    -   [ ] Perform manual integration testing if applicable.
    -   [ ] Update documentation regarding configuration files and environment variables.

## 5. Open Questions / Considerations (Unchanged)

-   Error handling strategy for failed built-in loading (should the server fail to start?). Currently assumed to log/error.
-   Specific TOML structure for `custom_tagspecs` within user files (e.g., `[custom_tagspecs.my_tag]`). Ensure `Config` struct matches. (Decision needed in Phase 2).
-   Need to clearly document which environment variables are supported for overrides. (Documented in Phase 1 & 4).
-   Testing strategy for `djls-templates` built-in loading (uses `include_dir!`). (Addressed in Phase 3).
