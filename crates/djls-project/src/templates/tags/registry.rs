use ruff_python_ast::StmtFunctionDef;

use crate::templates::FilterArity;
use crate::templates::RegistrationKind;
use crate::templates::TemplateSymbolKind;
use crate::templates::filters;
use crate::templates::registrations::RegisteredEnd;
use crate::templates::registrations::RegistrationOptions;
use crate::templates::tags::analysis;
use crate::templates::tags::analysis::TagSourceContext;
use crate::templates::tags::blocks;
use crate::templates::tags::signature;
use crate::templates::tags::types::AsVar;
use crate::templates::tags::types::BodyAnalysisEvidence;
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

    pub(crate) fn extract_tag_rule(
        self,
        source: Option<&mut TagSourceContext<'_>>,
        func: &StmtFunctionDef,
        options: &RegistrationOptions,
        trusted_callable: bool,
    ) -> Option<Box<TagRule>> {
        match self {
            Self::Filter => None,
            Self::SimpleTag | Self::InclusionTag | Self::SimpleBlockTag => {
                let mut rule = signature::extract_parse_bits_rule(
                    func,
                    self,
                    options.context,
                    self.var_assignment(),
                )?;
                if !trusted_callable
                    && let crate::templates::tags::types::TagArgumentSyntax::Signature {
                        parameters,
                        ..
                    } = rule.argument_syntax
                {
                    rule.argument_syntax =
                        crate::templates::tags::types::TagArgumentSyntax::Parameters(parameters);
                }
                rule.has_content().then(|| Box::new(rule))
            }
            Self::Tag => {
                let mut rule = source.map_or_else(
                    || analysis::analyze_compile_function(func),
                    |source| analysis::analyze_compile_function_in_source(source, func),
                );
                if self.var_assignment().strips_suffix() {
                    rule.as_var = self.var_assignment();
                }
                rule.has_content().then(|| Box::new(rule))
            }
        }
    }

    pub(crate) fn extract_block_spec(
        self,
        func: &StmtFunctionDef,
        options: &RegistrationOptions,
    ) -> Option<blocks::ExtractedBlockSpec> {
        match self {
            Self::Filter => None,
            Self::SimpleBlockTag => Some(blocks::ExtractedBlockSpec {
                end_tag: match options.block_end.as_ref()? {
                    RegisteredEnd::Default => blocks::EndTagEvidence::SelfNamed,
                    RegisteredEnd::Named(name) => blocks::EndTagEvidence::Literal(name.clone()),
                    RegisteredEnd::Unknown => blocks::EndTagEvidence::Unknown,
                },
                intermediates: Vec::new(),
                body_analysis_evidence: BodyAnalysisEvidence::NotDetected,
            }),
            Self::Tag | Self::SimpleTag | Self::InclusionTag => blocks::extract_block_spec(func),
        }
    }
}
