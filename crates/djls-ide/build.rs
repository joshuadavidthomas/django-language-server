use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use syn::{Attribute, File, Item, ItemEnum, Lit, Meta};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // Parse error enum files
    let validation_errors_path = manifest_dir
        .parent()
        .unwrap()
        .join("djls-semantic")
        .join("src")
        .join("errors.rs");

    let template_errors_path = manifest_dir
        .parent()
        .unwrap()
        .join("djls-templates")
        .join("src")
        .join("error.rs");

    // Tell Cargo to rerun if these files change
    println!("cargo:rerun-if-changed={}", validation_errors_path.display());
    println!("cargo:rerun-if-changed={}", template_errors_path.display());

    // Extract diagnostics from both files
    let mut all_diagnostics = Vec::new();

    all_diagnostics.extend(extract_diagnostics_from_file(
        &validation_errors_path,
        "ValidationError",
    ));

    all_diagnostics.extend(extract_diagnostics_from_file(
        &template_errors_path,
        "TemplateError",
    ));

    // Generate lookup table
    generate_lookup_table(&all_diagnostics);

    // Generate documentation
    generate_documentation(&all_diagnostics);
}

#[derive(Debug)]
struct DiagnosticInfo {
    code: String,
    category: String,
    enum_name: String,
    variant_name: String,
    title: String,
    doc_comment: String,
}

fn extract_diagnostics_from_file(path: &PathBuf, enum_name: &str) -> Vec<DiagnosticInfo> {
    let content = fs::read_to_string(path)
        .unwrap_or_else(|_| panic!("Failed to read {}", path.display()));

    let syntax: File = syn::parse_file(&content)
        .unwrap_or_else(|_| panic!("Failed to parse {}", path.display()));

    let mut diagnostics = Vec::new();

    for item in syntax.items {
        if let Item::Enum(enum_item) = item {
            if enum_item.ident == enum_name {
                diagnostics.extend(extract_from_enum(&enum_item, enum_name));
            }
        }
    }

    diagnostics
}

fn extract_from_enum(enum_item: &ItemEnum, enum_name: &str) -> Vec<DiagnosticInfo> {
    let mut diagnostics = Vec::new();

    for variant in &enum_item.variants {
        if let Some((code, category)) = extract_diagnostic_attr(&variant.attrs) {
            let doc_comment = extract_doc_comment(&variant.attrs);
            let title = extract_title_from_doc(&doc_comment);

            diagnostics.push(DiagnosticInfo {
                code,
                category,
                enum_name: enum_name.to_string(),
                variant_name: variant.ident.to_string(),
                title,
                doc_comment,
            });
        }
    }

    diagnostics
}

fn extract_diagnostic_attr(attrs: &[Attribute]) -> Option<(String, String)> {
    for attr in attrs {
        if attr.path().is_ident("diagnostic") {
            if let Meta::List(meta_list) = &attr.meta {
                let mut code = None;
                let mut category = None;

                // Parse the nested meta items
                let _ = meta_list.parse_nested_meta(|meta| {
                    if meta.path.is_ident("code") {
                        if let Ok(value) = meta.value() {
                            if let Ok(Lit::Str(lit)) = value.parse() {
                                code = Some(lit.value());
                            }
                        }
                    } else if meta.path.is_ident("category") {
                        if let Ok(value) = meta.value() {
                            if let Ok(Lit::Str(lit)) = value.parse() {
                                category = Some(lit.value());
                            }
                        }
                    }
                    Ok(())
                });

                if let (Some(code), Some(category)) = (code, category) {
                    return Some((code, category));
                }
            }
        }
    }
    None
}

fn extract_doc_comment(attrs: &[Attribute]) -> String {
    let mut doc_lines = Vec::new();

    for attr in attrs {
        if attr.path().is_ident("doc") {
            if let Meta::NameValue(meta) = &attr.meta {
                if let syn::Expr::Lit(expr_lit) = &meta.value {
                    if let Lit::Str(lit) = &expr_lit.lit {
                        doc_lines.push(lit.value());
                    }
                }
            }
        }
    }

    doc_lines.join("\n")
}

fn extract_title_from_doc(doc: &str) -> String {
    // First non-empty line is the title
    doc.lines()
        .map(|line| line.trim())
        .find(|line| !line.is_empty())
        .unwrap_or("Unknown")
        .to_string()
}

fn generate_lookup_table(diagnostics: &[DiagnosticInfo]) {
    let mut mappings = Vec::new();

    for diag in diagnostics {
        let full_name = format!("{}::{}", diag.enum_name, diag.variant_name);
        mappings.push(format!(
            "    (\"{}\", \"{}\"),",
            full_name,
            diag.code
        ));
    }

    let generated_code = format!(
r#"// This file is generated by build.rs from error enum definitions
// DO NOT EDIT MANUALLY
//
// This provides a lookup table from error type names to diagnostic codes.
// The actual trait implementations in diagnostics.rs use this data.

pub(crate) const DIAGNOSTIC_CODE_MAPPINGS: &[(&str, &str)] = &[
{}
];
"#,
        mappings.join("\n")
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("diagnostic_codes.rs"), generated_code)
        .expect("Failed to write generated code");
}

fn generate_documentation(diagnostics: &[DiagnosticInfo]) {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let docs_dir = manifest_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("docs")
        .join("rules");

    // Create rules directory if it doesn't exist
    fs::create_dir_all(&docs_dir).expect("Failed to create docs/rules directory");

    // Group by category
    let mut by_category: HashMap<String, Vec<&DiagnosticInfo>> = HashMap::new();
    for diag in diagnostics {
        by_category
            .entry(diag.category.clone())
            .or_default()
            .push(diag);
    }

    // Sort each category by code
    for rules in by_category.values_mut() {
        rules.sort_by(|a, b| a.code.cmp(&b.code));
    }

    // Generate individual rule pages
    for diag in diagnostics {
        let rule_path = docs_dir.join(format!("{}.md", diag.code));
        let content = generate_rule_page(diag);
        fs::write(&rule_path, content)
            .unwrap_or_else(|_| panic!("Failed to write {}", rule_path.display()));
    }

    // Generate index page
    generate_index_page(&docs_dir, &by_category);
}

fn generate_rule_page(diag: &DiagnosticInfo) -> String {
    let mut page = String::new();

    page.push_str(&format!("# {} - {}\n\n", diag.code, diag.title));
    page.push_str("<!-- This file is automatically generated from Rust source code -->\n");
    page.push_str("<!-- Do not edit manually. Update the error enum definition instead. -->\n\n");

    page.push_str(&format!("**Code:** `{}`  \n", diag.code));
    page.push_str(&format!("**Category:** {}  \n", diag.category));
    page.push_str(&format!("**Severity:** error  \n\n"));

    // Process doc comment
    let doc = process_doc_comment(&diag.doc_comment);
    page.push_str(&doc);

    page
}

fn process_doc_comment(doc: &str) -> String {
    let mut result = String::new();
    let mut in_section = false;
    let mut section_header = String::new();

    for line in doc.lines() {
        let trimmed = line.trim();

        // Skip the title line (first non-empty line)
        if !in_section && trimmed.is_empty() {
            continue;
        }

        // Detect markdown headers (# Examples, # Fix, etc.)
        if trimmed.starts_with('#') {
            if !section_header.is_empty() {
                result.push('\n');
            }
            section_header = trimmed.to_string();
            result.push_str(&format!("## {}\n\n", &trimmed[1..].trim()));
            in_section = true;
            continue;
        }

        // First paragraph after title becomes Description
        if !in_section && !trimmed.is_empty() {
            result.push_str("## Description\n\n");
            in_section = true;
        }

        result.push_str(trimmed);
        result.push('\n');
    }

    result
}

fn generate_index_page(docs_dir: &PathBuf, by_category: &HashMap<String, Vec<&DiagnosticInfo>>) {
    let mut index = String::new();

    index.push_str("# Diagnostic Rules Reference\n\n");
    index.push_str("<!-- This file is automatically generated from Rust source code -->\n");
    index.push_str("<!-- Do not edit manually. Update the error enum definitions instead. -->\n\n");
    index.push_str("This document lists all diagnostic rules provided by the Django Language Server.\n\n");

    // Template rules
    if let Some(template_rules) = by_category.get("template") {
        index.push_str("## Template Parsing Errors (T-series)\n\n");
        index.push_str("These errors occur during the parsing phase of template processing.\n\n");
        for diag in template_rules {
            index.push_str(&format!("- [**{}**](./{}.md) - {}\n", diag.code, diag.code, diag.title));
        }
        index.push('\n');
    }

    // Semantic rules
    if let Some(semantic_rules) = by_category.get("semantic") {
        index.push_str("## Semantic Validation Errors (S-series)\n\n");
        index.push_str("These errors are detected during semantic analysis of valid template syntax.\n\n");
        for diag in semantic_rules {
            index.push_str(&format!("- [**{}**](./{}.md) - {}\n", diag.code, diag.code, diag.title));
        }
    }

    let index_path = docs_dir.join("index.md");
    fs::write(&index_path, index)
        .expect("Failed to write index.md");
}
