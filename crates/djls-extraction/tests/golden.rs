//! Golden extraction tests against real Django source.
//!
//! These tests run extraction against REAL Django template tag and filter
//! source code from the corpus — not hand-crafted subsets. This ensures
//! extraction results match what the system actually encounters at runtime.
//!
//! # Running
//!
//! These tests require a synced corpus:
//!
//! ```bash
//! cargo run -p djls-corpus -- sync
//! cargo test -p djls-extraction --test golden -- --nocapture
//! ```
//!
//! If the corpus is not available, tests skip gracefully.

use std::collections::BTreeMap;

use djls_corpus::enumerate::FileKind;
use djls_corpus::Corpus;
use djls_extraction::extract_rules;
use djls_extraction::ArgumentCountConstraint;
use djls_extraction::BlockTagSpec;
use djls_extraction::ExtractionResult;
use djls_extraction::FilterArity;
use djls_extraction::SymbolKey;
use djls_extraction::TagRule;
use serde::Serialize;

/// A deterministically-ordered version of `ExtractionResult` for snapshot testing.
#[derive(Debug, Serialize)]
struct SortedExtractionResult {
    tag_rules: BTreeMap<String, TagRule>,
    filter_arities: BTreeMap<String, FilterArity>,
    block_specs: BTreeMap<String, BlockTagSpec>,
}

impl From<ExtractionResult> for SortedExtractionResult {
    fn from(result: ExtractionResult) -> Self {
        let key_str = |k: &SymbolKey| format!("{}::{}", k.registration_module, k.name);
        Self {
            tag_rules: result
                .tag_rules
                .iter()
                .map(|(k, v)| (key_str(k), v.clone()))
                .collect(),
            filter_arities: result
                .filter_arities
                .iter()
                .map(|(k, v)| (key_str(k), v.clone()))
                .collect(),
            block_specs: result
                .block_specs
                .iter()
                .map(|(k, v)| (key_str(k), v.clone()))
                .collect(),
        }
    }
}

fn snapshot(result: ExtractionResult) -> SortedExtractionResult {
    result.into()
}

/// Extract from a real Django source file in the corpus.
fn extract_django_module(
    corpus: &Corpus,
    relative: &str,
    module_path: &str,
) -> Option<ExtractionResult> {
    let django_dir = corpus.latest_django()?;
    let path = django_dir.join(relative);
    if !path.as_std_path().exists() {
        return None;
    }
    let source = std::fs::read_to_string(path.as_std_path()).ok()?;
    Some(extract_rules(&source, module_path))
}

// Django core: defaulttags.py

#[test]
fn test_defaulttags_full_snapshot() {
    let Some(corpus) = Corpus::discover() else {
        eprintln!("Corpus not available. Run `cargo run -p djls-corpus -- sync`.");
        return;
    };

    let result = extract_django_module(
        &corpus,
        "django/template/defaulttags.py",
        "django.template.defaulttags",
    )
    .expect("defaulttags.py should be extractable");

    insta::assert_yaml_snapshot!("defaulttags_full", snapshot(result));
}

#[test]
fn test_defaulttags_tag_count() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let result = extract_django_module(
        &corpus,
        "django/template/defaulttags.py",
        "django.template.defaulttags",
    )
    .unwrap();

    let mut tag_names: Vec<&str> = result.tag_rules.keys().map(|k| k.name.as_str()).collect();
    tag_names.sort_unstable();

    eprintln!("defaulttags.py tags with rules: {tag_names:?}");
    assert!(
        result.tag_rules.len() >= 5,
        "Expected >= 5 tag rules from defaulttags.py, got {}",
        result.tag_rules.len()
    );
}

#[test]
fn test_defaulttags_for_tag_rules() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let result = extract_django_module(
        &corpus,
        "django/template/defaulttags.py",
        "django.template.defaulttags",
    )
    .unwrap();
    let for_key = SymbolKey::tag("django.template.defaulttags", "for");
    let for_rule = result
        .tag_rules
        .get(&for_key)
        .expect("for tag should have extracted rules");

    eprintln!("for tag constraints ({}):", for_rule.arg_constraints.len());
    for constraint in &for_rule.arg_constraints {
        eprintln!("  {constraint:?}");
    }

    // Real Django for tag has: len(bits) < 4 → Min(4) constraint
    assert!(
        for_rule
            .arg_constraints
            .iter()
            .any(|c| matches!(c, ArgumentCountConstraint::Min(4))),
        "for tag should have Min(4) constraint from `len(bits) < 4`"
    );

    // Block spec
    let for_block = result
        .block_specs
        .get(&for_key)
        .expect("for tag should have block spec");
    assert_eq!(for_block.end_tag.as_deref(), Some("endfor"));
    assert!(
        for_block.intermediates.contains(&"empty".to_string()),
        "for tag should have 'empty' intermediate"
    );
}

#[test]
fn test_defaulttags_if_tag() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let result = extract_django_module(
        &corpus,
        "django/template/defaulttags.py",
        "django.template.defaulttags",
    )
    .unwrap();

    // Block spec: endif, intermediates elif/else
    let if_key = SymbolKey::tag("django.template.defaulttags", "if");
    let block = result
        .block_specs
        .get(&if_key)
        .expect("if tag should have block spec");
    assert_eq!(block.end_tag.as_deref(), Some("endif"));
    assert!(
        block.intermediates.contains(&"elif".to_string()),
        "should have elif"
    );
    assert!(
        block.intermediates.contains(&"else".to_string()),
        "should have else"
    );
}

#[test]
fn test_defaulttags_url_tag_rules() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let result = extract_django_module(
        &corpus,
        "django/template/defaulttags.py",
        "django.template.defaulttags",
    )
    .unwrap();
    let url_key = SymbolKey::tag("django.template.defaulttags", "url");
    let url_rule = result
        .tag_rules
        .get(&url_key)
        .expect("url tag should have extracted rules");

    eprintln!("url tag constraints ({}):", url_rule.arg_constraints.len());
    for constraint in &url_rule.arg_constraints {
        eprintln!("  {constraint:?}");
    }

    // Real Django url tag: if len(bits) < 2 → Min(2) constraint
    assert!(
        url_rule
            .arg_constraints
            .iter()
            .any(|c| matches!(c, ArgumentCountConstraint::Min(2))),
        "url tag should have Min(2) constraint from `len(bits) < 2`"
    );
}

#[test]
fn test_defaulttags_with_tag() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let result = extract_django_module(
        &corpus,
        "django/template/defaulttags.py",
        "django.template.defaulttags",
    )
    .unwrap();
    let with_key = SymbolKey::tag("django.template.defaulttags", "with");
    let block = result
        .block_specs
        .get(&with_key)
        .expect("with tag should have block spec");
    assert_eq!(block.end_tag.as_deref(), Some("endwith"));
}

// Django core: defaultfilters.py

#[test]
fn test_defaultfilters_full_snapshot() {
    let Some(corpus) = Corpus::discover() else {
        eprintln!("Corpus not available. Run `cargo run -p djls-corpus -- sync`.");
        return;
    };

    let result = extract_django_module(
        &corpus,
        "django/template/defaultfilters.py",
        "django.template.defaultfilters",
    )
    .expect("defaultfilters.py should be extractable");

    eprintln!(
        "defaultfilters.py: {} tag rules, {} filters",
        result.tag_rules.len(),
        result.filter_arities.len()
    );

    insta::assert_yaml_snapshot!("defaultfilters_full", snapshot(result));
}

#[test]
fn test_defaultfilters_filter_count() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let result = extract_django_module(
        &corpus,
        "django/template/defaultfilters.py",
        "django.template.defaultfilters",
    )
    .unwrap();

    let mut filter_names: Vec<&str> = result
        .filter_arities
        .keys()
        .map(|k| k.name.as_str())
        .collect();
    filter_names.sort_unstable();

    eprintln!("defaultfilters.py filters ({}):", filter_names.len());
    for name in &filter_names {
        let key = result
            .filter_arities
            .keys()
            .find(|k| k.name == *name)
            .unwrap();
        let arity = &result.filter_arities[key];
        eprintln!("  {name}: {arity:?}");
    }

    // Django has ~60+ built-in filters
    assert!(
        result.filter_arities.len() >= 50,
        "Expected >= 50 filters from defaultfilters.py, got {}",
        result.filter_arities.len()
    );
}

// Django core: loader_tags.py

#[test]
fn test_loader_tags_full_snapshot() {
    let Some(corpus) = Corpus::discover() else {
        eprintln!("Corpus not available. Run `cargo run -p djls-corpus -- sync`.");
        return;
    };

    let result = extract_django_module(
        &corpus,
        "django/template/loader_tags.py",
        "django.template.loader_tags",
    )
    .expect("loader_tags.py should be extractable");

    insta::assert_yaml_snapshot!("loader_tags_full", snapshot(result));
}

#[test]
fn test_loader_tags_block_tag() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let result = extract_django_module(
        &corpus,
        "django/template/loader_tags.py",
        "django.template.loader_tags",
    )
    .unwrap();
    let block_key = SymbolKey::tag("django.template.loader_tags", "block");
    let block = result
        .block_specs
        .get(&block_key)
        .expect("block tag should have block spec");
    assert_eq!(block.end_tag.as_deref(), Some("endblock"));
}

// Django templatetags: i18n, static, cache, l10n, tz

#[test]
fn test_i18n_full_snapshot() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };
    let django_dir = corpus.latest_django().unwrap();
    let path = django_dir.join("django/templatetags/i18n.py");
    if !path.as_std_path().exists() {
        eprintln!("i18n.py not found");
        return;
    }

    let source = std::fs::read_to_string(path.as_std_path()).unwrap();
    let result = extract_rules(&source, "django.templatetags.i18n");
    insta::assert_yaml_snapshot!("i18n_full", snapshot(result));
}

#[test]
fn test_static_full_snapshot() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };
    let django_dir = corpus.latest_django().unwrap();
    let path = django_dir.join("django/templatetags/static.py");
    if !path.as_std_path().exists() {
        eprintln!("static.py not found");
        return;
    }

    let source = std::fs::read_to_string(path.as_std_path()).unwrap();
    let result = extract_rules(&source, "django.templatetags.static");
    insta::assert_yaml_snapshot!("static_full", snapshot(result));
}

#[test]
fn test_cache_full_snapshot() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };
    let django_dir = corpus.latest_django().unwrap();
    let path = django_dir.join("django/templatetags/cache.py");
    if !path.as_std_path().exists() {
        eprintln!("cache.py not found");
        return;
    }

    let source = std::fs::read_to_string(path.as_std_path()).unwrap();
    let result = extract_rules(&source, "django.templatetags.cache");
    insta::assert_yaml_snapshot!("cache_full", snapshot(result));
}

#[test]
fn test_l10n_full_snapshot() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };
    let django_dir = corpus.latest_django().unwrap();
    let path = django_dir.join("django/templatetags/l10n.py");
    if !path.as_std_path().exists() {
        eprintln!("l10n.py not found");
        return;
    }

    let source = std::fs::read_to_string(path.as_std_path()).unwrap();
    let result = extract_rules(&source, "django.templatetags.l10n");
    insta::assert_yaml_snapshot!("l10n_full", snapshot(result));
}

#[test]
fn test_tz_full_snapshot() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };
    let django_dir = corpus.latest_django().unwrap();
    let path = django_dir.join("django/templatetags/tz.py");
    if !path.as_std_path().exists() {
        eprintln!("tz.py not found");
        return;
    }

    let source = std::fs::read_to_string(path.as_std_path()).unwrap();
    let result = extract_rules(&source, "django.templatetags.tz");
    insta::assert_yaml_snapshot!("tz_full", snapshot(result));
}

// Cross-version stability

#[test]
fn test_for_tag_rules_across_django_versions() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let django_packages = corpus.root().join("packages/Django");
    if !django_packages.as_std_path().exists() {
        return;
    }

    let version_dirs = corpus.synced_dirs("packages/Django");

    for version_dir in &version_dirs {
        let version = version_dir.file_name().unwrap();
        let defaulttags = version_dir.join("django/template/defaulttags.py");
        if !defaulttags.as_std_path().exists() {
            continue;
        }

        let source = std::fs::read_to_string(defaulttags.as_std_path()).unwrap();
        let result = extract_rules(&source, "django.template.defaulttags");
        let for_key = SymbolKey::tag("django.template.defaulttags", "for");

        if let Some(for_rule) = result.tag_rules.get(&for_key) {
            eprintln!(
                "Django {version} for tag: {} constraints",
                for_rule.arg_constraints.len()
            );
            for constraint in &for_rule.arg_constraints {
                eprintln!("  {constraint:?}");
            }

            // Every Django version should have the min-args rule
            assert!(
                for_rule
                    .arg_constraints
                    .iter()
                    .any(|c| matches!(c, ArgumentCountConstraint::Min(4))),
                "Django {version}: for tag missing Min(4) constraint"
            );
        } else {
            eprintln!("Django {version}: for tag has no extracted rules");
        }
    }
}

// Third-party packages

#[test]
fn test_wagtail_extraction_snapshot() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let wagtail_dir = corpus.root().join("packages/wagtail");
    if !wagtail_dir.as_std_path().exists() {
        eprintln!("wagtail not in corpus");
        return;
    }

    let mut combined = ExtractionResult::default();
    let extraction_files = corpus.enumerate_files(&wagtail_dir, FileKind::ExtractionTarget);

    for path in &extraction_files {
        let source = std::fs::read_to_string(path.as_std_path()).unwrap_or_default();
        let result = extract_rules(&source, "wagtail");
        combined.merge(result);
    }

    eprintln!(
        "wagtail: {} tag rules, {} filters, {} block specs from {} files",
        combined.tag_rules.len(),
        combined.filter_arities.len(),
        combined.block_specs.len(),
        extraction_files.len()
    );

    insta::assert_yaml_snapshot!("wagtail_full", snapshot(combined));
}

#[test]
fn test_allauth_extraction_snapshot() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let allauth_dir = corpus.root().join("packages/django-allauth");
    if !allauth_dir.as_std_path().exists() {
        eprintln!("django-allauth not in corpus");
        return;
    }

    let mut combined = ExtractionResult::default();
    let extraction_files = corpus.enumerate_files(&allauth_dir, FileKind::ExtractionTarget);

    for path in &extraction_files {
        let source = std::fs::read_to_string(path.as_std_path()).unwrap_or_default();
        let result = extract_rules(&source, "allauth");
        combined.merge(result);
    }

    eprintln!(
        "allauth: {} tag rules, {} filters, {} block specs from {} files",
        combined.tag_rules.len(),
        combined.filter_arities.len(),
        combined.block_specs.len(),
        extraction_files.len()
    );

    insta::assert_yaml_snapshot!("allauth_full", snapshot(combined));
}
