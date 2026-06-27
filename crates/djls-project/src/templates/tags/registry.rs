use ruff_python_ast::StmtFunctionDef;

use crate::templates::FilterArity;
use crate::templates::RegistrationKind;
use crate::templates::TemplateSymbolKind;
use crate::templates::filters;
use crate::templates::tags::analysis;
use crate::templates::tags::blocks;
use crate::templates::tags::signature;
use crate::templates::tags::types::AsVar;
use crate::templates::tags::types::BlockSpec;
use crate::templates::tags::types::TagRule;

impl RegistrationKind {
    pub(crate) fn symbol_kind(self) -> TemplateSymbolKind {
        match self {
            Self::Tag | Self::SimpleTag | Self::InclusionTag | Self::SimpleBlockTag => {
                TemplateSymbolKind::Tag
            }
            Self::Filter => TemplateSymbolKind::Filter,
        }
    }

    fn var_assignment(self) -> AsVar {
        match self {
            Self::SimpleTag | Self::SimpleBlockTag => AsVar::Strip,
            Self::Tag | Self::InclusionTag | Self::Filter => AsVar::Keep,
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
