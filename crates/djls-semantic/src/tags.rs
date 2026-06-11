mod rules;
mod specs;

use djls_project::Project;
use djls_project::extract_block_specs;
use djls_project::extract_tag_rules;
use djls_project::templatetag_modules;
pub(crate) use rules::evaluate_tag_rules;
pub use specs::EndTag;
#[cfg(test)]
pub(crate) use specs::IntermediateTag;
pub use specs::TagArgument;
pub use specs::TagArgumentKind;
pub use specs::TagSpec;
pub use specs::TagSpecs;
pub use specs::builtin_tag_specs;

use crate::db::Db;
use crate::references::TemplateReferenceKind;

/// Durable Django template meaning for a tag.
///
/// This describes what the tag does in the template domain. Feature-specific
/// projections, such as document symbols, map these roles into their own shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagRole {
    TemplateReference(TemplateReferenceKind),
    TemplateLibraryLoader,
    TemplateBlock,
    ControlTag,
    TemplateTag,
    StaticAssetReference,
    RouteReference,
}

/// Compute `TagSpecs` from tag-rule and block-spec extraction results.
///
/// This tracked function reads only the extraction domains needed to build tag
/// specs. Filter-only extraction changes should not invalidate this query.
///
/// Does NOT read from `Arc<Mutex<Settings>>`.
#[salsa::tracked(returns(ref))]
pub fn compute_tag_specs(db: &dyn Db, project: Project) -> TagSpecs {
    let tagspecs = project.tagspecs(db);

    let mut specs = builtin_tag_specs();

    for module in templatetag_modules(db, project) {
        let block_specs = extract_block_specs(db, module.file(), module.module_path().clone());
        if !block_specs.is_empty() {
            specs.merge_block_specs(block_specs);
        }

        let tag_rules = extract_tag_rules(db, module.file(), module.module_path().clone());
        if !tag_rules.is_empty() {
            specs.merge_tag_rules(tag_rules);
        }
    }

    if !tagspecs.libraries.is_empty() {
        let fallback = TagSpecs::from_tagspec_def(tagspecs);
        specs.merge_fallback(fallback);
    }

    specs
}
