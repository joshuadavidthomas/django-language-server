use std::collections::BTreeMap;

use djls_project::BlockSpecs;
use djls_project::FilterArity;
use djls_project::FilterArityMap;
use djls_project::PythonModuleName;
use djls_project::SymbolKey;
use djls_project::TagRule;
use djls_project::TagRuleMap;
use djls_project::TemplateSymbolKind;
use djls_source::File;
use serde::Serialize;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExtractionBundle {
    pub tag_rules: TagRuleMap,
    pub filter_arities: FilterArityMap,
    pub block_specs: BlockSpecs,
}

impl ExtractionBundle {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tag_rules.is_empty() && self.filter_arities.is_empty() && self.block_specs.is_empty()
    }
}

#[must_use]
pub fn extract_bundle(
    db: &dyn djls_project::Db,
    file: File,
    registration_module: PythonModuleName,
) -> ExtractionBundle {
    let key = djls_project::TemplateLibraryKey::new(db, Some(file), registration_module);
    let tag_facts = djls_project::template_library_tag_facts(db, key);
    let filter_facts = djls_project::template_library_filter_facts(db, key);
    let tag_rules = tag_facts.tag_rules().to_owned();
    let filter_arities = filter_facts.filter_arities().to_owned();
    let block_specs = tag_facts.block_specs().to_owned();

    ExtractionBundle {
        tag_rules,
        filter_arities,
        block_specs,
    }
}

#[derive(Debug, Serialize)]
pub struct SortedExtractionResult {
    tag_rules: BTreeMap<String, TagRule>,
    filter_arities: BTreeMap<String, FilterArity>,
    block_specs: BTreeMap<String, serde_json::Value>,
}

/// Convert an extraction bundle into deterministic snapshot data.
///
/// # Panics
///
/// Panics if `BlockSpecs` serialization fails or does not produce a JSON map.
/// The extraction types are serializable by construction, so this indicates a
/// programming error.
#[must_use]
pub fn sorted_snapshot(bundle: &ExtractionBundle) -> SortedExtractionResult {
    SortedExtractionResult {
        tag_rules: bundle
            .tag_rules
            .iter()
            .map(|(key, rule)| (key_str(key), rule.as_ref().clone()))
            .collect(),
        filter_arities: bundle
            .filter_arities
            .iter()
            .map(|(key, arity)| (key_str(key), arity.clone()))
            .collect(),
        block_specs: serde_json::from_value(
            serde_json::to_value(&bundle.block_specs)
                .expect("BlockSpecs serialization should succeed"),
        )
        .expect("serialized BlockSpecs should be a JSON object"),
    }
}

fn key_str(key: &SymbolKey) -> String {
    let kind = match key.kind {
        TemplateSymbolKind::Tag => "tag",
        TemplateSymbolKind::Filter => "filter",
    };
    format!("{}::{kind}::{}", key.registration_module, key.name)
}
