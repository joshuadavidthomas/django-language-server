use ruff_python_ast::StmtFunctionDef;

use crate::extraction::registry::RegistrationKind;
use crate::specs::TemplateSymbolKind;
use crate::specs::analysis;
use crate::specs::blocks;
use crate::specs::filters;
use crate::specs::signature;
use crate::specs::types::AsVar;
use crate::specs::types::BlockSpec;
use crate::specs::types::FilterArity;
use crate::specs::types::TagRule;

/// Output of [`RegistrationKind::extract`], distinguishing filter vs tag results.
pub(crate) enum ExtractionOutput {
    Filter(FilterArity),
    Tag {
        rule: Option<Box<TagRule>>,
        block_spec: Option<BlockSpec>,
    },
}

impl RegistrationKind {
    pub(crate) fn symbol_kind(self) -> TemplateSymbolKind {
        match self {
            Self::Tag | Self::SimpleTag | Self::InclusionTag | Self::SimpleBlockTag => {
                TemplateSymbolKind::Tag
            }
            Self::Filter => TemplateSymbolKind::Filter,
        }
    }

    pub(crate) fn var_assignment(self) -> AsVar {
        match self {
            Self::SimpleTag | Self::SimpleBlockTag => AsVar::Strip,
            Self::Tag | Self::InclusionTag | Self::Filter => AsVar::Keep,
        }
    }

    pub(crate) fn extract(self, func: &StmtFunctionDef) -> ExtractionOutput {
        match self {
            Self::Filter => ExtractionOutput::Filter(filters::extract_filter_arity(func)),
            Self::SimpleTag | Self::InclusionTag | Self::Tag | Self::SimpleBlockTag => {
                ExtractionOutput::Tag {
                    rule: self.extract_tag_rule(func),
                    block_spec: self.extract_block_spec(func),
                }
            }
        }
    }

    pub(crate) fn extract_filter_arity(self, func: &StmtFunctionDef) -> Option<FilterArity> {
        match self {
            Self::Filter => Some(filters::extract_filter_arity(func)),
            Self::Tag | Self::SimpleTag | Self::InclusionTag | Self::SimpleBlockTag => None,
        }
    }

    pub(crate) fn extract_tag_rule(self, func: &StmtFunctionDef) -> Option<Box<TagRule>> {
        match self {
            Self::Filter => None,
            Self::SimpleTag | Self::InclusionTag => {
                let rule = signature::extract_parse_bits_rule(func, self.var_assignment());
                rule.has_content().then(|| Box::new(rule))
            }
            Self::Tag | Self::SimpleBlockTag => {
                let mut rule = analysis::analyze_compile_function(func);
                if self.var_assignment().strips_suffix() {
                    rule.as_var = self.var_assignment();
                }
                rule.has_content().then(|| Box::new(rule))
            }
        }
    }

    pub(crate) fn extract_block_spec(self, func: &StmtFunctionDef) -> Option<BlockSpec> {
        match self {
            Self::Filter => None,
            Self::Tag | Self::SimpleTag | Self::InclusionTag | Self::SimpleBlockTag => {
                blocks::extract_block_spec(func)
            }
        }
    }
}
