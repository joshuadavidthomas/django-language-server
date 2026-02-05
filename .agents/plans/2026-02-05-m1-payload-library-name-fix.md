# M1: Payload Shape + Library Name Fix Implementation Plan

## Overview

Fix the inspector payload structure to preserve Django library load-names and distinguish builtins from loadable libraries, then fix completions to show correct library names for `{% load %}`.

## Current State Analysis

### Python Inspector (`crates/djls-project/inspector/queries.py`)
- `TemplateTag` dataclass has `name`, `module`, `doc`
- `get_installed_templatetags()` iterates `engine.libraries.values()`, **losing the library name key**
- `module` field stores `tag_func.__module__` (defining module), not the library module
- No distinction between builtins and loadable libraries—all flattened into one list

### Rust Types (`crates/djls-project/src/django.rs`)
- `TemplateTag` struct mirrors Python: `name`, `module`, `doc`
- `module()` returns the defining module path (e.g., `django.template.defaulttags`)
- No concept of library load-name or provenance

### Completions (`crates/djls-ide/src/completions.rs`)
- `generate_library_completions()` at line 526 collects `tag.module()` as library names
- Results in completions like `django.templatetags.static` instead of `static`
- Cannot filter out builtins (which shouldn't appear in `{% load %}` completions)

## Desired End State

Per charter section 1.1:

1. **Inventory items carry:**
   - `name` — tag name as used in templates
   - `provenance` — **exactly one of:**
     - `Library { load_name, module }` — requires `{% load X %}`
     - `Builtin { module }` — always available
   - `defining_module` — where the function is defined (`tag_func.__module__`)
   - `doc` — optional docstring

2. **`{% load %}` completions show library load-names** (`static`, `i18n`) not module paths

3. **Builtins excluded from `{% load %}` completions** (they're always available)

## What We're NOT Doing

- **M3 load scoping**: Unknown tag diagnostics remain silent (pre-M3 behavior)
- **Filter inventory**: Filter collection is M4 scope
- **Collision handling**: Per charter, no collision detection in M1
- **Salsa invalidation fixes**: That's M2 scope

## Implementation Approach

Single PR with three components:
1. Expand Python inspector payload with new data model
2. Update Rust types to deserialize new payload
3. Fix completions to use `load_name` from `Library` provenance

## Phase 1: Python Inspector Payload Changes

### Overview
Update the inspector to return library information with proper provenance distinction, plus top-level registry structures for downstream use.

### Changes Required:

#### 1. Update Data Structures
**File**: `crates/djls-project/inspector/queries.py`
**Changes**: Add new dataclasses for provenance and top-level registry data

```python
@dataclass
class TemplateTag:
    name: str
    provenance: dict  # {"library": {"load_name": str, "module": str}} | {"builtin": {"module": str}}
    defining_module: str
    doc: str | None


@dataclass
class TemplateTagQueryData:
    # Top-level registry structures (preserved from Django engine)
    libraries: dict[str, str]  # load_name → module_path mapping
    builtins: list[str]  # ordered builtin module paths
    # Tag inventory
    templatetags: list[TemplateTag]
```

**Note**: We use `dict` for provenance (not dataclass) because it serializes naturally with `asdict()` as the externally-tagged union `{"library": {...}}` or `{"builtin": {...}}` that Rust's serde expects.

#### 2. Update Collection Logic
**File**: `crates/djls-project/inspector/queries.py`
**Changes**: Rewrite `get_installed_templatetags()` to preserve library keys and use `engine.builtins` for correct module paths

```python
def get_installed_templatetags() -> TemplateTagQueryData:
    import django
    from django.apps import apps
    from django.template.engine import Engine
    from django.template.library import import_library

    if not apps.ready:
        django.setup()

    engine = Engine.get_default()
    templatetags: list[TemplateTag] = []

    # Preserve top-level registry structures
    # engine.libraries: {load_name: module_path} - the authoritative mapping
    libraries = dict(engine.libraries)
    # engine.builtins: ordered list of builtin module paths
    builtins = list(engine.builtins)

    # Collect builtins with Builtin provenance
    # Use zip to pair module paths (engine.builtins) with Library objects (engine.template_builtins)
    # Guard: these should always be the same length, but check to avoid silent data loss
    if len(engine.builtins) != len(engine.template_builtins):
        raise RuntimeError(
            f"engine.builtins ({len(engine.builtins)}) and "
            f"engine.template_builtins ({len(engine.template_builtins)}) length mismatch"
        )
    for builtin_module, library in zip(engine.builtins, engine.template_builtins):
        if library.tags:
            for tag_name, tag_func in library.tags.items():
                templatetags.append(
                    TemplateTag(
                        name=tag_name,
                        provenance={"builtin": {"module": builtin_module}},
                        defining_module=tag_func.__module__,
                        doc=tag_func.__doc__,
                    )
                )

    # Collect libraries with Library provenance, preserving load-name
    for load_name, lib_module in engine.libraries.items():
        library = import_library(lib_module)
        if library and library.tags:
            for tag_name, tag_func in library.tags.items():
                templatetags.append(
                    TemplateTag(
                        name=tag_name,
                        provenance={"library": {"load_name": load_name, "module": lib_module}},
                        defining_module=tag_func.__module__,
                        doc=tag_func.__doc__,
                    )
                )

    return TemplateTagQueryData(
        libraries=libraries,
        builtins=builtins,
        templatetags=templatetags,
    )
```

**Key fix**: `engine.builtins` gives us the ordered module paths (e.g., `["django.template.defaulttags", "django.template.defaultfilters"]`), while `engine.template_builtins` gives us the corresponding `Library` objects. Using `zip()` pairs them correctly.

### Success Criteria:

#### Automated Verification:
- [ ] Rust build passes (which exercises inspector via tests): `cargo build`
- [ ] Inspector integration test passes: `cargo test -p djls-project`
- [ ] Manual inspector test: `echo '{"query":"templatetags"}' | python crates/djls-project/inspector/__main__.py`

#### Manual Verification:
- [ ] Verify payload includes top-level `libraries` dict with load-names as keys
- [ ] Verify payload includes top-level `builtins` list in correct order
- [ ] Confirm builtin provenance modules are correct (e.g., `django.template.defaulttags`, not `django.template.library`)
- [ ] Confirm library provenance has both `load_name` and `module` fields

**Implementation Note**: After completing this phase and all automated verification passes, pause here for manual confirmation from the human that the payload structure is correct before proceeding to the next phase.

---

## Phase 2: Rust Type Updates

### Overview
Update Rust types to deserialize the new payload structure with `TagProvenance` enum and top-level registry data.

### Changes Required:

#### 1. Add TagProvenance Enum and Update Response
**File**: `crates/djls-project/src/django.rs`
**Changes**: Add new enum, update `TemplateTag` struct, and expand response to include registry data

```rust
use std::collections::HashMap;
use serde::Deserialize;

/// Provenance of a template tag - either from a loadable library or a builtin
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagProvenance {
    /// Tag requires `{% load X %}` to use
    Library {
        /// The name used in `{% load X %}` (e.g., "static", "i18n")
        load_name: String,
        /// The Python module path where the library is registered
        module: String,
    },
    /// Tag is always available (builtin)
    Builtin {
        /// The Python module path where the builtin is registered
        module: String,
    },
}

#[derive(Deserialize)]
struct TemplatetagsResponse {
    /// Load-name → module path mapping (from engine.libraries)
    libraries: HashMap<String, String>,
    /// Ordered builtin module paths (from engine.builtins)
    builtins: Vec<String>,
    /// Tag inventory
    templatetags: Vec<TemplateTag>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TemplateTag {
    name: String,
    provenance: TagProvenance,
    defining_module: String,
    doc: Option<String>,
}
```

#### 2. Update TemplateTag Accessors
**File**: `crates/djls-project/src/django.rs`
**Changes**: Add clear accessors - avoid confusing `module()` name

```rust
impl TemplateTag {
    pub fn name(&self) -> &String {
        &self.name
    }

    pub fn provenance(&self) -> &TagProvenance {
        &self.provenance
    }

    /// The Python module where the tag function is defined (tag_func.__module__)
    /// This is where the actual code lives, useful for docs/jump-to-def.
    pub fn defining_module(&self) -> &String {
        &self.defining_module
    }

    pub fn doc(&self) -> Option<&String> {
        self.doc.as_ref()
    }

    /// Returns the library load-name if this is a library tag, None for builtins.
    /// This is the name used in `{% load X %}`.
    pub fn library_load_name(&self) -> Option<&str> {
        match &self.provenance {
            TagProvenance::Library { load_name, .. } => Some(load_name),
            TagProvenance::Builtin { .. } => None,
        }
    }

    /// Returns true if this tag is a builtin (always available without {% load %})
    pub fn is_builtin(&self) -> bool {
        matches!(self.provenance, TagProvenance::Builtin { .. })
    }

    /// The Python module where this tag is registered (the library/builtin module).
    /// For libraries, this is the module in engine.libraries.
    /// For builtins, this is the module in engine.builtins.
    pub fn registration_module(&self) -> &str {
        match &self.provenance {
            TagProvenance::Library { module, .. } => module,
            TagProvenance::Builtin { module } => module,
        }
    }
}
```

**Note**: We use `defining_module()` for where the function is defined (the old `module()` behavior) and `registration_module()` for the library/builtin module. This avoids the ambiguity of a bare `module()` accessor.

#### 3. Update TemplateTags to Include Registry Data
**File**: `crates/djls-project/src/django.rs`
**Changes**: Expand `TemplateTags` to hold top-level registry structures

```rust
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TemplateTags {
    /// Load-name → module path mapping (from engine.libraries)
    libraries: HashMap<String, String>,
    /// Ordered builtin module paths (from engine.builtins)
    builtins: Vec<String>,
    /// Tag inventory
    tags: Vec<TemplateTag>,
}

impl TemplateTags {
    /// Create a new TemplateTags (primarily for testing)
    pub fn new(
        libraries: HashMap<String, String>,
        builtins: Vec<String>,
        tags: Vec<TemplateTag>,
    ) -> Self {
        Self { libraries, builtins, tags }
    }

    /// Get the tag list
    pub fn tags(&self) -> &[TemplateTag] {
        &self.tags
    }

    /// Get the libraries mapping (load_name → module_path)
    pub fn libraries(&self) -> &HashMap<String, String> {
        &self.libraries
    }

    /// Get the ordered builtin module paths
    pub fn builtins(&self) -> &[String] {
        &self.builtins
    }

    /// Iterate over tags (convenience method)
    pub fn iter(&self) -> impl Iterator<Item = &TemplateTag> {
        self.tags.iter()
    }

    /// Number of tags
    pub fn len(&self) -> usize {
        self.tags.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }
}

impl TemplateTag {
    /// Create a library tag (for testing)
    pub fn new_library(name: &str, load_name: &str, module: &str, doc: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            provenance: TagProvenance::Library {
                load_name: load_name.to_string(),
                module: module.to_string(),
            },
            defining_module: module.to_string(),
            doc: doc.map(String::from),
        }
    }

    /// Create a builtin tag (for testing)
    pub fn new_builtin(name: &str, module: &str, doc: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            provenance: TagProvenance::Builtin {
                module: module.to_string(),
            },
            defining_module: module.to_string(),
            doc: doc.map(String::from),
        }
    }
}
```

#### 4. Update the Salsa Query
**File**: `crates/djls-project/src/django.rs`
**Changes**: Convert response to new TemplateTags structure

```rust
#[salsa::tracked]
pub fn templatetags(db: &dyn ProjectDb, _project: Project) -> Option<TemplateTags> {
    let response = inspector::query(db, &TemplatetagsRequest)?;
    let tag_count = response.templatetags.len();
    tracing::debug!("Retrieved {} templatetags from inspector", tag_count);
    Some(TemplateTags {
        libraries: response.libraries,
        builtins: response.builtins,
        tags: response.templatetags,
    })
}
```

#### 5. Update lib.rs Exports
**File**: `crates/djls-project/src/lib.rs`
**Changes**: Export the new enum

```rust
pub use django::TagProvenance;
```

#### 6. Update Tests
**File**: `crates/djls-project/src/django.rs`
**Changes**: Update existing tests and add new ones for provenance

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_tag_library_provenance() {
        let tag = TemplateTag {
            name: "static".to_string(),
            provenance: TagProvenance::Library {
                load_name: "static".to_string(),
                module: "django.templatetags.static".to_string(),
            },
            defining_module: "django.templatetags.static".to_string(),
            doc: Some("Display static file URL".to_string()),
        };
        assert_eq!(tag.name(), "static");
        assert_eq!(tag.library_load_name(), Some("static"));
        assert!(!tag.is_builtin());
        assert_eq!(tag.registration_module(), "django.templatetags.static");
        assert_eq!(tag.defining_module(), "django.templatetags.static");
    }

    #[test]
    fn test_template_tag_builtin_provenance() {
        let tag = TemplateTag {
            name: "if".to_string(),
            provenance: TagProvenance::Builtin {
                module: "django.template.defaulttags".to_string(),
            },
            defining_module: "django.template.defaulttags".to_string(),
            doc: Some("Conditional block".to_string()),
        };
        assert_eq!(tag.name(), "if");
        assert_eq!(tag.library_load_name(), None);
        assert!(tag.is_builtin());
        assert_eq!(tag.registration_module(), "django.template.defaulttags");
    }

    #[test]
    fn test_template_tag_deserialization() {
        let json = r#"{
            "name": "trans",
            "provenance": {"library": {"load_name": "i18n", "module": "django.templatetags.i18n"}},
            "defining_module": "django.templatetags.i18n",
            "doc": "Translate text"
        }"#;
        let tag: TemplateTag = serde_json::from_str(json).expect("Should deserialize");
        assert_eq!(tag.name(), "trans");
        assert_eq!(tag.library_load_name(), Some("i18n"));
    }

    #[test]
    fn test_template_tags_registry_data() {
        let mut libraries = HashMap::new();
        libraries.insert("static".to_string(), "django.templatetags.static".to_string());
        libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());

        let tags = TemplateTags {
            libraries,
            builtins: vec![
                "django.template.defaulttags".to_string(),
                "django.template.defaultfilters".to_string(),
            ],
            tags: vec![
                TemplateTag {
                    name: "if".to_string(),
                    provenance: TagProvenance::Builtin {
                        module: "django.template.defaulttags".to_string(),
                    },
                    defining_module: "django.template.defaulttags".to_string(),
                    doc: None,
                },
                TemplateTag {
                    name: "static".to_string(),
                    provenance: TagProvenance::Library {
                        load_name: "static".to_string(),
                        module: "django.templatetags.static".to_string(),
                    },
                    defining_module: "django.templatetags.static".to_string(),
                    doc: None,
                },
            ],
        };

        assert_eq!(tags.len(), 2);
        assert_eq!(tags.libraries().len(), 2);
        assert_eq!(tags.builtins().len(), 2);
        assert!(tags.iter().next().unwrap().is_builtin());
    }
}
```

### Success Criteria:

#### Automated Verification:
- [ ] Rust compiles: `cargo build -p djls-project`
- [ ] Clippy passes: `cargo clippy -p djls-project --all-targets -- -D warnings`
- [ ] Unit tests pass: `cargo test -p djls-project`
- [ ] Deserialization test passes with mock JSON

#### Manual Verification:
- [ ] Confirm `TagProvenance` enum is ergonomic to use in downstream code

**Implementation Note**: After completing this phase and all automated verification passes, pause here for manual confirmation from the human that the Rust types are correct before proceeding to the next phase.

---

## Phase 3: Completions Fix

### Overview
Update completions to use library load-name and exclude builtins from `{% load %}` completions.

### Changes Required:

#### 1. Update Library Completions
**File**: `crates/djls-ide/src/completions.rs`
**Changes**: Fix `generate_library_completions()` to use library keys directly, with deterministic ordering

```rust
/// Generate completions for library names (for {% load %} tag)
fn generate_library_completions(
    partial: &str,
    closing: &ClosingBrace,
    template_tags: Option<&TemplateTags>,
) -> Vec<ls_types::CompletionItem> {
    let Some(tags) = template_tags else {
        return Vec::new();
    };

    // Collect and sort library names for deterministic ordering
    let mut library_entries: Vec<_> = tags.libraries()
        .iter()
        .filter(|(load_name, _)| load_name.starts_with(partial))
        .collect();
    library_entries.sort_by_key(|(load_name, _)| load_name.as_str());

    let mut completions = Vec::new();

    for (load_name, module_path) in library_entries {
        let mut insert_text = load_name.clone();

        // Add closing if needed
        match closing {
            ClosingBrace::None => insert_text.push_str(" %}"),
            ClosingBrace::PartialClose => insert_text.push_str(" %"),
            ClosingBrace::FullClose => {} // No closing needed
        }

        completions.push(ls_types::CompletionItem {
            label: load_name.clone(),
            kind: Some(ls_types::CompletionItemKind::MODULE),
            detail: Some(format!("Django template library ({})", module_path)),
            insert_text: Some(insert_text),
            insert_text_format: Some(ls_types::InsertTextFormat::PLAIN_TEXT),
            filter_text: Some(load_name.clone()),
            ..Default::default()
        });
    }

    completions
}
```

**Note**: We sort library names alphabetically for deterministic completion ordering. HashMap iteration order is nondeterministic, which would cause flaky tests and inconsistent UX.

#### 2. Update Tag Name Completion Detail
**File**: `crates/djls-ide/src/completions.rs`
**Changes**: Update detail to show more useful info in `generate_tag_name_completions()`

Find the line (around line 488):
```rust
detail: Some(format!("from {}", tag.module())),
```

Change to show library info when available:
```rust
detail: Some(if let Some(lib) = tag.library_load_name() {
    format!("from {} ({{% load {} %}})", tag.defining_module(), lib)
} else {
    format!("builtin from {}", tag.defining_module())
}),
```

#### 3. Update Tag Iteration
**File**: `crates/djls-ide/src/completions.rs`
**Changes**: Since `TemplateTags` no longer implements `Deref`, update iteration

Find uses of `tags.iter()` in `generate_tag_name_completions()` and ensure they still work (the `iter()` method is still available on `TemplateTags`).

### Success Criteria:

#### Automated Verification:
- [ ] Rust compiles: `cargo build -p djls-ide`
- [ ] Clippy passes: `cargo clippy -p djls-ide --all-targets -- -D warnings`
- [ ] Unit tests pass: `cargo test -p djls-ide`
- [ ] Full build passes: `cargo build`
- [ ] All tests pass: `cargo test`

#### Manual Verification:
- [ ] Open a Django project in editor with djls
- [ ] Type `{% load ` and verify completions show `static`, `i18n`, `cache` etc.
- [ ] Verify completions do NOT show module paths like `django.templatetags.static`
- [ ] Verify builtin tag completions show "builtin from ..." in detail
- [ ] Verify library tag completions show "from ... ({% load X %})" in detail

**Implementation Note**: After completing this phase and all automated verification passes, pause here for manual confirmation from the human that completions work correctly in the editor.

---

## Testing Strategy

### Unit Tests (Automated):

#### In `crates/djls-project/src/django.rs`:
1. **Rust deserialization** - verify `TagProvenance` enum deserializes correctly from JSON
2. **Accessor methods** - verify `library_load_name()`, `is_builtin()`, `defining_module()`, `registration_module()` work correctly
3. **Registry data accessors** - verify `libraries()`, `builtins()` return expected data

#### In `crates/djls-ide/src/completions.rs`:
Add a new test for library completions:

```rust
#[test]
fn test_generate_library_completions() {
    use std::collections::HashMap;
    use djls_project::{TemplateTags, TemplateTag, TagProvenance};

    let mut libraries = HashMap::new();
    libraries.insert("static".to_string(), "django.templatetags.static".to_string());
    libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());
    libraries.insert("cache".to_string(), "django.templatetags.cache".to_string());

    let tags = TemplateTags::new(
        libraries,
        vec!["django.template.defaulttags".to_string()],
        vec![
            TemplateTag::new_builtin("if", "django.template.defaulttags", None),
            TemplateTag::new_library("static", "static", "django.templatetags.static", None),
        ],
    );

    let completions = generate_library_completions("", &ClosingBrace::None, Some(&tags));

    // Should return library names, not module paths
    let labels: Vec<_> = completions.iter().map(|c| c.label.as_str()).collect();
    assert!(labels.contains(&"static"));
    assert!(labels.contains(&"i18n"));
    assert!(labels.contains(&"cache"));
    
    // Should NOT contain module paths
    assert!(!labels.iter().any(|l| l.contains("django.")));
    
    // Should be deterministically ordered (alphabetical)
    assert_eq!(labels, vec!["cache", "i18n", "static"]);
}

#[test]
fn test_generate_library_completions_partial() {
    // ... similar test with partial = "st" expecting only "static"
}
```

**Note**: This requires adding constructor methods to `TemplateTags`/`TemplateTag` for test convenience, or making fields pub(crate).

### Manual Testing Steps:
1. Start djls in a Django project
2. Open a template file
3. Type `{% load ` and trigger completion
4. Verify library names appear (not module paths)
5. Verify completions are in alphabetical order
6. Select a completion and verify it inserts correctly
7. Type `{% if` and trigger completion
8. Verify detail shows "builtin from django.template.defaulttags"

## Performance Considerations

- Payload size slightly increases due to nested provenance structure
- No performance impact on completion generation (still O(n) iteration)
- HashSet deduplication is the same algorithmic complexity

## Migration Notes

This is a **breaking change** to the inspector payload format:

**Old payload:**
```json
{
  "templatetags": [
    {"name": "static", "module": "django.templatetags.static", "doc": "..."}
  ]
}
```

**New payload:**
```json
{
  "libraries": {"static": "django.templatetags.static", "i18n": "django.templatetags.i18n"},
  "builtins": ["django.template.defaulttags", "django.template.defaultfilters"],
  "templatetags": [
    {
      "name": "static",
      "provenance": {"library": {"load_name": "static", "module": "django.templatetags.static"}},
      "defining_module": "django.templatetags.static",
      "doc": "..."
    },
    {
      "name": "if",
      "provenance": {"builtin": {"module": "django.template.defaulttags"}},
      "defining_module": "django.template.defaulttags",
      "doc": "..."
    }
  ]
}
```

No data migration needed—this is runtime data from Django introspection.

## References

- Charter: `.agents/charter/2026-02-05-template-validation-port-charter.md`
- Current inspector: `crates/djls-project/inspector/queries.py`
- Current Rust types: `crates/djls-project/src/django.rs`
- Current completions: `crates/djls-ide/src/completions.rs`
