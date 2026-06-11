use djls_project::extraction::RegistrationKind;
use ruff_python_ast::StmtFunctionDef;

use crate::python::SymbolKind;
use crate::python::analysis;
use crate::python::blocks;
use crate::python::filters;
use crate::python::signature;
use crate::python::types::AsVar;
use crate::python::types::BlockSpec;
use crate::python::types::FilterArity;
use crate::python::types::TagRule;

/// Output of [`RegistrationKindExt::extract`], distinguishing filter vs tag results.
pub(crate) enum ExtractionOutput {
    Filter(FilterArity),
    Tag {
        rule: Option<Box<TagRule>>,
        block_spec: Option<BlockSpec>,
    },
}

pub(crate) trait RegistrationKindExt {
    fn symbol_kind(self) -> SymbolKind;
    fn var_assignment(self) -> AsVar;
    fn extract(self, func: &StmtFunctionDef) -> ExtractionOutput;
    fn extract_filter_arity(self, func: &StmtFunctionDef) -> Option<FilterArity>;
    fn extract_tag_rule(self, func: &StmtFunctionDef) -> Option<Box<TagRule>>;
    fn extract_block_spec(self, func: &StmtFunctionDef) -> Option<BlockSpec>;
}

impl RegistrationKindExt for RegistrationKind {
    fn symbol_kind(self) -> SymbolKind {
        match self {
            Self::Tag | Self::SimpleTag | Self::InclusionTag | Self::SimpleBlockTag => {
                SymbolKind::Tag
            }
            Self::Filter => SymbolKind::Filter,
        }
    }

    fn var_assignment(self) -> AsVar {
        match self {
            Self::SimpleTag | Self::SimpleBlockTag => AsVar::Strip,
            Self::Tag | Self::InclusionTag | Self::Filter => AsVar::Keep,
        }
    }

    fn extract(self, func: &StmtFunctionDef) -> ExtractionOutput {
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

    fn extract_filter_arity(self, func: &StmtFunctionDef) -> Option<FilterArity> {
        match self {
            Self::Filter => Some(filters::extract_filter_arity(func)),
            Self::Tag | Self::SimpleTag | Self::InclusionTag | Self::SimpleBlockTag => None,
        }
    }

    fn extract_tag_rule(self, func: &StmtFunctionDef) -> Option<Box<TagRule>> {
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

    fn extract_block_spec(self, func: &StmtFunctionDef) -> Option<BlockSpec> {
        match self {
            Self::Filter => None,
            Self::Tag | Self::SimpleTag | Self::InclusionTag | Self::SimpleBlockTag => {
                blocks::extract_block_spec(func)
            }
        }
    }
}
