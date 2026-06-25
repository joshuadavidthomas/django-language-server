//! Completion flow:
//! 1. Locate source text from `File` and decide whether this file supports completions.
//! 2. Read the `CompletionOffsetContext` for the offset.
//! 3. Generate candidates relevant to that context.
//! 4. Decide edit semantics for each candidate: replacement range, insert text, and snippets.
//! 5. Rank candidates by relevance.
//! 6. Convert candidates into an LSP completion response using client/session facts.

use djls_project::InstalledSymbolOrigin;
use djls_project::StaticKnowledge;
use djls_project::TemplateLibraries;
use djls_project::TemplateSymbolKind;
use djls_semantic::AvailableSymbols;
use djls_semantic::TagArgumentKind;
use djls_semantic::TagSpecs;
use djls_source::File;
use djls_source::FileKind;
use djls_source::Offset;
use djls_source::PositionEncoding;
use djls_source::Span;
use tower_lsp_server::ls_types;

use crate::context::CompletionOffsetContext;
use crate::context::OffsetPrefix;
use crate::context::OffsetSuffix;
use crate::context::TagClose;
use crate::context::TemplateCompletionContext;
use crate::ext::CompletionCandidateExt;
use crate::snippets::generate_partial_snippet;
use crate::snippets::generate_snippet_for_tag_with_end;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionCandidateKind {
    TagName,
    EndTag,
    TagArgumentLiteral,
    TagArgumentChoice,
    TagArgumentPlaceholder,
    TagArgumentSnippet,
    LibraryName,
    LoadSymbol,
    Filter,
}

impl CompletionCandidateKind {
    pub(crate) fn rank(self) -> u8 {
        match self {
            Self::EndTag => 0,
            Self::TagName
            | Self::TagArgumentLiteral
            | Self::TagArgumentChoice
            | Self::LibraryName
            | Self::LoadSymbol
            | Self::Filter => 1,
            Self::TagArgumentPlaceholder => 3,
            Self::TagArgumentSnippet => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionInsertFormat {
    PlainText,
    Snippet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletionEdit {
    pub(crate) replacement_span: Span,
    pub(crate) insert_text: String,
    pub(crate) insert_format: CompletionInsertFormat,
}

impl CompletionEdit {
    fn plain(replacement_span: Span, insert_text: impl Into<String>) -> Self {
        Self {
            replacement_span,
            insert_text: insert_text.into(),
            insert_format: CompletionInsertFormat::PlainText,
        }
    }

    fn snippet(replacement_span: Span, insert_text: impl Into<String>) -> Self {
        Self {
            replacement_span,
            insert_text: insert_text.into(),
            insert_format: CompletionInsertFormat::Snippet,
        }
    }

    fn tag_plain(
        name: &str,
        prefix: &OffsetPrefix<'_>,
        needs_leading_space: bool,
        close: TagClose,
    ) -> Self {
        let mut insert_text = String::new();
        if needs_leading_space {
            insert_text.push(' ');
        }
        insert_text.push_str(name);
        Self::append_tag_close(&mut insert_text, close);

        Self::plain(Self::tag_replacement_span(prefix, close), insert_text)
    }

    fn tag_snippet(
        name: &str,
        spec: &djls_semantic::TagSpec,
        prefix: &OffsetPrefix<'_>,
        needs_leading_space: bool,
        close: TagClose,
    ) -> Option<Self> {
        if spec.arguments().is_empty() {
            return None;
        }

        let mut insert_text = String::new();
        if needs_leading_space {
            insert_text.push(' ');
        }

        let snippet = generate_snippet_for_tag_with_end(name, spec);
        let snippet_closes_tag = snippet.contains("%}");
        insert_text.push_str(&snippet);
        if !snippet_closes_tag {
            Self::append_tag_close(&mut insert_text, close);
        }

        let replacement_span = if snippet_closes_tag {
            Self::replacement_span_with_suffix(
                prefix,
                close.existing_close_replacement_suffix_len(),
            )
        } else {
            Self::tag_replacement_span(prefix, close)
        };

        Some(Self::snippet(replacement_span, insert_text))
    }

    fn tag_argument(label: &str, prefix: &OffsetPrefix<'_>, close: TagClose) -> Self {
        let mut insert_text = label.to_string();
        Self::append_tag_close(&mut insert_text, close);

        Self::plain(Self::tag_replacement_span(prefix, close), insert_text)
    }

    fn tag_argument_with_suffix(
        label: &str,
        prefix: &OffsetPrefix<'_>,
        suffix: &OffsetSuffix<'_>,
        close: TagClose,
    ) -> Self {
        let mut insert_text = label.to_string();
        Self::append_tag_close(&mut insert_text, close);

        Self::plain(
            Self::span_with_source_suffix(prefix, suffix, close.partial_replacement_suffix_len()),
            insert_text,
        )
    }

    fn load_symbol(
        name: &str,
        prefix: &OffsetPrefix<'_>,
        suffix: &OffsetSuffix<'_>,
        needs_trailing_space: bool,
    ) -> Self {
        let mut insert_text = name.to_string();
        if needs_trailing_space {
            insert_text.push(' ');
        }

        Self::plain(
            Self::span_with_source_suffix(prefix, suffix, 0),
            insert_text,
        )
    }

    fn append_tag_close(insert_text: &mut String, close: TagClose) {
        match close {
            TagClose::None | TagClose::Partial { .. } => insert_text.push_str(" %}"),
            TagClose::Full { .. } => {}
        }
    }

    fn tag_replacement_span(prefix: &OffsetPrefix<'_>, close: TagClose) -> Span {
        Self::replacement_span_with_suffix(prefix, close.partial_replacement_suffix_len())
    }

    fn replacement_span_with_suffix(prefix: &OffsetPrefix<'_>, suffix_len: usize) -> Span {
        let replacement_length = prefix.span.length_usize() + suffix_len;
        prefix.span.with_length_usize_saturating(replacement_length)
    }

    fn span_with_source_suffix(
        prefix: &OffsetPrefix<'_>,
        suffix: &OffsetSuffix<'_>,
        suffix_len: usize,
    ) -> Span {
        let end = suffix
            .span
            .end_usize()
            .saturating_add(suffix_len)
            .max(prefix.span.end_usize());
        Span::saturating_from_bounds_usize(prefix.span.start_usize(), end)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletionCandidate {
    pub(crate) label: String,
    pub(crate) kind: CompletionCandidateKind,
    pub(crate) edit: CompletionEdit,
    pub(crate) detail: Option<String>,
    pub(crate) documentation: Option<String>,
}

impl CompletionCandidate {
    fn cmp_rank(left: &Self, right: &Self) -> std::cmp::Ordering {
        left.kind
            .rank()
            .cmp(&right.kind.rank())
            .then_with(|| left.label.cmp(&right.label))
    }

    fn tag_name(
        symbol: &djls_project::TemplateSymbol,
        prefix: &OffsetPrefix<'_>,
        needs_leading_space: bool,
        close: TagClose,
        spec: Option<&djls_semantic::TagSpec>,
        origin: &InstalledSymbolOrigin,
        supports_snippets: bool,
    ) -> Self {
        let name = symbol.name();
        let edit = if supports_snippets {
            spec.and_then(|spec| {
                CompletionEdit::tag_snippet(name, spec, prefix, needs_leading_space, close)
            })
            .unwrap_or_else(|| CompletionEdit::tag_plain(name, prefix, needs_leading_space, close))
        } else {
            CompletionEdit::tag_plain(name, prefix, needs_leading_space, close)
        };

        Self {
            label: name.to_string(),
            kind: CompletionCandidateKind::TagName,
            edit,
            detail: Some(tag_completion_detail(origin)),
            documentation: symbol.doc().map(str::to_string),
        }
    }

    fn tag_name_from_spec(
        name: &str,
        prefix: &OffsetPrefix<'_>,
        needs_leading_space: bool,
        close: TagClose,
        spec: &djls_semantic::TagSpec,
        supports_snippets: bool,
    ) -> Self {
        let edit = if supports_snippets {
            CompletionEdit::tag_snippet(name, spec, prefix, needs_leading_space, close)
                .unwrap_or_else(|| {
                    CompletionEdit::tag_plain(name, prefix, needs_leading_space, close)
                })
        } else {
            CompletionEdit::tag_plain(name, prefix, needs_leading_space, close)
        };

        Self {
            label: name.to_string(),
            kind: CompletionCandidateKind::TagName,
            edit,
            detail: Some("Django template tag".to_string()),
            documentation: None,
        }
    }

    fn end_tag(
        opener_name: &str,
        name: &str,
        prefix: &OffsetPrefix<'_>,
        needs_leading_space: bool,
        close: TagClose,
    ) -> Self {
        Self {
            label: name.to_string(),
            kind: CompletionCandidateKind::EndTag,
            edit: CompletionEdit::tag_plain(name, prefix, needs_leading_space, close),
            detail: Some(format!("End tag for {opener_name}")),
            documentation: None,
        }
    }

    fn tag_argument_literal(label: &str, prefix: &OffsetPrefix<'_>, close: TagClose) -> Self {
        Self {
            label: label.to_string(),
            kind: CompletionCandidateKind::TagArgumentLiteral,
            edit: CompletionEdit::tag_argument(label, prefix, close),
            detail: Some("literal argument".to_string()),
            documentation: None,
        }
    }

    fn tag_argument_choice(
        label: &str,
        argument_name: &str,
        prefix: &OffsetPrefix<'_>,
        close: TagClose,
    ) -> Self {
        Self {
            label: label.to_string(),
            kind: CompletionCandidateKind::TagArgumentChoice,
            edit: CompletionEdit::tag_argument(label, prefix, close),
            detail: Some(format!("choice for {argument_name}")),
            documentation: None,
        }
    }

    fn tag_argument_placeholder(label: String, prefix: &OffsetPrefix<'_>) -> Self {
        Self {
            edit: CompletionEdit::plain(prefix.span, label.clone()),
            label,
            kind: CompletionCandidateKind::TagArgumentPlaceholder,
            detail: Some("variable argument".to_string()),
            documentation: None,
        }
    }

    fn tag_argument_snippet(
        label: String,
        insert_text: String,
        prefix: &OffsetPrefix<'_>,
        close: TagClose,
    ) -> Self {
        let mut insert_text = insert_text;
        CompletionEdit::append_tag_close(&mut insert_text, close);

        Self {
            label,
            kind: CompletionCandidateKind::TagArgumentSnippet,
            edit: CompletionEdit::snippet(
                CompletionEdit::tag_replacement_span(prefix, close),
                insert_text,
            ),
            detail: Some("Complete remaining arguments".to_string()),
            documentation: None,
        }
    }

    fn library_name(
        name: &str,
        prefix: &OffsetPrefix<'_>,
        suffix: &OffsetSuffix<'_>,
        close: TagClose,
        detail: String,
    ) -> Self {
        Self {
            label: name.to_string(),
            kind: CompletionCandidateKind::LibraryName,
            edit: CompletionEdit::tag_argument_with_suffix(name, prefix, suffix, close),
            detail: Some(detail),
            documentation: None,
        }
    }

    fn load_symbol(
        name: &str,
        prefix: &OffsetPrefix<'_>,
        suffix: &OffsetSuffix<'_>,
        needs_trailing_space: bool,
        documentation: Option<&str>,
    ) -> Self {
        Self {
            label: name.to_string(),
            kind: CompletionCandidateKind::LoadSymbol,
            edit: CompletionEdit::load_symbol(name, prefix, suffix, needs_trailing_space),
            detail: Some("load symbol".to_string()),
            documentation: documentation.map(str::to_string),
        }
    }

    fn filter(
        name: &str,
        prefix: &OffsetPrefix<'_>,
        origin: &InstalledSymbolOrigin,
        documentation: Option<&str>,
    ) -> Self {
        Self {
            label: name.to_string(),
            kind: CompletionCandidateKind::Filter,
            edit: CompletionEdit::plain(prefix.span, name),
            detail: Some(filter_completion_detail(origin)),
            documentation: documentation.map(str::to_string),
        }
    }
}

fn tag_completion_detail(origin: &InstalledSymbolOrigin) -> String {
    match origin {
        InstalledSymbolOrigin::Builtin { module } => format!("builtin from {}", module.as_str()),
        InstalledSymbolOrigin::Loadable { load_name } => {
            format!("{{% load {} %}}", load_name.as_str())
        }
    }
}

fn filter_completion_detail(origin: &InstalledSymbolOrigin) -> String {
    match origin {
        InstalledSymbolOrigin::Builtin { .. } => "builtin filter".to_string(),
        InstalledSymbolOrigin::Loadable { load_name } => {
            format!("{{% load {} %}}", load_name.as_str())
        }
    }
}

#[must_use]
pub fn completion(
    db: &dyn djls_semantic::Db,
    file: File,
    offset: Offset,
    encoding: PositionEncoding,
    supports_snippets: bool,
) -> Option<ls_types::CompletionResponse> {
    let source = file.source(db);
    if *source.kind() != FileKind::Template {
        return None;
    }

    let tokens = djls_templates::lex_template(db, file);
    let context = CompletionOffsetContext::new(*source.kind(), source.as_str(), tokens, offset);
    let template_libraries = db.template_libraries();

    let available_symbols = if template_libraries.knowledge == StaticKnowledge::Unknown {
        None
    } else {
        match &context {
            CompletionOffsetContext::Template(
                TemplateCompletionContext::TagName { .. }
                | TemplateCompletionContext::Filter { .. },
            ) => djls_templates::parse_template(db, file)
                .map(|nodelist| djls_semantic::available_symbols_at(db, nodelist, offset.get())),
            CompletionOffsetContext::Template(
                TemplateCompletionContext::Text
                | TemplateCompletionContext::TagArgument { .. }
                | TemplateCompletionContext::LibraryName { .. }
                | TemplateCompletionContext::LoadSymbol { .. },
            )
            | CompletionOffsetContext::None => None,
        }
    };

    let mut candidates = match &context {
        CompletionOffsetContext::Template(TemplateCompletionContext::TagName {
            prefix,
            needs_leading_space,
            close,
        }) => generate_tag_name_candidates(
            prefix,
            *needs_leading_space,
            *close,
            template_libraries,
            db.tag_specs(),
            available_symbols.as_ref(),
            supports_snippets,
        ),
        CompletionOffsetContext::Template(TemplateCompletionContext::TagArgument {
            tag,
            position,
            prefix,
            close,
            ..
        }) => generate_tag_argument_candidates(
            tag,
            *position,
            prefix,
            *close,
            db.tag_specs(),
            supports_snippets,
        ),
        CompletionOffsetContext::Template(TemplateCompletionContext::LibraryName {
            prefix,
            suffix,
            close,
        }) => generate_library_name_candidates(prefix, suffix, *close, template_libraries),
        CompletionOffsetContext::Template(TemplateCompletionContext::LoadSymbol {
            prefix,
            suffix,
            library,
            needs_trailing_space,
        }) => generate_load_symbol_candidates(
            prefix,
            suffix,
            *library,
            *needs_trailing_space,
            template_libraries,
        ),
        CompletionOffsetContext::Template(TemplateCompletionContext::Filter { prefix }) => {
            generate_filter_candidates(prefix, template_libraries, available_symbols.as_ref())
        }
        CompletionOffsetContext::Template(TemplateCompletionContext::Text)
        | CompletionOffsetContext::None => Vec::new(),
    };
    if candidates.is_empty() {
        return None;
    }

    candidates.sort_by(CompletionCandidate::cmp_rank);
    let line_index = file.line_index(db);
    let items = candidates
        .iter()
        .map(|candidate| candidate.to_lsp_completion_item(source.as_str(), line_index, encoding))
        .collect::<Vec<_>>();

    Some(ls_types::CompletionResponse::Array(items))
}

fn generate_tag_name_candidates(
    prefix: &OffsetPrefix<'_>,
    needs_leading_space: bool,
    close: TagClose,
    template_libraries: &TemplateLibraries,
    tag_specs: &TagSpecs,
    available_symbols: Option<&AvailableSymbols>,
    supports_snippets: bool,
) -> Vec<CompletionCandidate> {
    let mut candidates = Vec::new();

    if prefix.text.starts_with("end") {
        for (opener_name, spec) in tag_specs {
            let Some(end_tag) = &spec.end_tag else {
                continue;
            };
            let name = end_tag.name.as_ref();
            if name.starts_with(prefix.text) {
                candidates.push(CompletionCandidate::end_tag(
                    opener_name,
                    name,
                    prefix,
                    needs_leading_space,
                    close,
                ));
            }
        }
    }

    if template_libraries.knowledge == StaticKnowledge::Unknown {
        for (name, spec) in tag_specs {
            if !name.starts_with(prefix.text) {
                continue;
            }

            candidates.push(CompletionCandidate::tag_name_from_spec(
                name,
                prefix,
                needs_leading_space,
                close,
                spec,
                supports_snippets,
            ));
        }

        candidates.sort_by(CompletionCandidate::cmp_rank);
        candidates.dedup_by(|left, right| left.label == right.label);
        return candidates;
    }

    for candidate in template_libraries.installed_symbol_candidates(TemplateSymbolKind::Tag) {
        let symbol = &candidate.symbol;
        if available_symbols.is_some_and(|symbols| !symbols.contains_symbol(symbol)) {
            continue;
        }

        let name = symbol.name();
        if !name.starts_with(prefix.text) {
            continue;
        }

        candidates.push(CompletionCandidate::tag_name(
            symbol,
            prefix,
            needs_leading_space,
            close,
            tag_specs.get(name),
            &candidate.origin,
            supports_snippets,
        ));
    }

    candidates
}

fn generate_tag_argument_candidates(
    tag: &str,
    position: usize,
    prefix: &OffsetPrefix<'_>,
    close: TagClose,
    tag_specs: &TagSpecs,
    supports_snippets: bool,
) -> Vec<CompletionCandidate> {
    let Some(spec) = tag_specs.get(tag) else {
        return Vec::new();
    };

    let arguments = spec.arguments();
    let Some(argument) = arguments.get(position) else {
        return Vec::new();
    };

    let mut candidates = match &argument.kind {
        TagArgumentKind::Literal(value) if value.starts_with(prefix.text) => {
            vec![CompletionCandidate::tag_argument_literal(
                value, prefix, close,
            )]
        }
        TagArgumentKind::Choice(choices) => choices
            .iter()
            .filter(|choice| choice.starts_with(prefix.text))
            .map(|choice| {
                CompletionCandidate::tag_argument_choice(choice, &argument.name, prefix, close)
            })
            .collect(),
        TagArgumentKind::Variable | TagArgumentKind::Keyword if prefix.text.is_empty() => {
            let label = format!("<{}>", argument.name);
            vec![CompletionCandidate::tag_argument_placeholder(label, prefix)]
        }
        TagArgumentKind::Literal(_)
        | TagArgumentKind::Variable
        | TagArgumentKind::Keyword
        | TagArgumentKind::VarArgs => Vec::new(),
    };

    if supports_snippets && prefix.text.is_empty() {
        let remaining_snippet = generate_partial_snippet(spec, position);
        if !remaining_snippet.is_empty() {
            let label = if position == 0 {
                format!("{tag} arguments")
            } else {
                "remaining arguments".to_string()
            };
            candidates.push(CompletionCandidate::tag_argument_snippet(
                label,
                remaining_snippet,
                prefix,
                close,
            ));
        }
    }

    candidates
}

fn generate_library_name_candidates(
    prefix: &OffsetPrefix<'_>,
    suffix: &OffsetSuffix<'_>,
    close: TagClose,
    template_libraries: &TemplateLibraries,
) -> Vec<CompletionCandidate> {
    let mut candidates = Vec::new();
    for name in template_libraries.completion_library_names() {
        if !name.as_str().starts_with(prefix.text) {
            continue;
        }

        let detail = template_libraries
            .loadable_library_module(&name)
            .map_or_else(
                || "Django template library".to_string(),
                |module| format!("Django template library ({})", module.as_str()),
            );
        candidates.push(CompletionCandidate::library_name(
            name.as_str(),
            prefix,
            suffix,
            close,
            detail,
        ));
    }

    candidates
}

fn generate_load_symbol_candidates(
    prefix: &OffsetPrefix<'_>,
    suffix: &OffsetSuffix<'_>,
    library: Option<&str>,
    needs_trailing_space: bool,
    template_libraries: &TemplateLibraries,
) -> Vec<CompletionCandidate> {
    if template_libraries.knowledge != StaticKnowledge::Known {
        return Vec::new();
    }

    let Some(library) = library.and_then(|name| template_libraries.loadable_library_str(name))
    else {
        return Vec::new();
    };

    library
        .symbols
        .iter()
        .filter(|symbol| symbol.name().starts_with(prefix.text))
        .map(|symbol| {
            CompletionCandidate::load_symbol(
                symbol.name(),
                prefix,
                suffix,
                needs_trailing_space,
                symbol.doc(),
            )
        })
        .collect()
}

fn generate_filter_candidates(
    prefix: &OffsetPrefix<'_>,
    template_libraries: &TemplateLibraries,
    available_symbols: Option<&AvailableSymbols>,
) -> Vec<CompletionCandidate> {
    if template_libraries.knowledge == StaticKnowledge::Known {
        let mut candidates = Vec::new();

        for candidate in template_libraries.installed_symbol_candidates(TemplateSymbolKind::Filter)
        {
            let symbol = &candidate.symbol;
            if available_symbols.is_some_and(|symbols| !symbols.contains_symbol(symbol)) {
                continue;
            }

            let name = symbol.name();
            if name.starts_with(prefix.text) {
                candidates.push(CompletionCandidate::filter(
                    name,
                    prefix,
                    &candidate.origin,
                    symbol.doc(),
                ));
            }
        }

        candidates.sort_by(|left, right| left.label.cmp(&right.label));
        candidates.dedup_by(|left, right| left.label == right.label);
        return candidates;
    }

    Vec::new()
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::collections::BTreeMap;

    use djls_project::LibraryName;
    use djls_project::PythonModulePath;
    use djls_project::SymbolDefinition;
    use djls_project::TemplateLibrary;
    use djls_project::TemplateSymbol;
    use djls_project::TemplateSymbolName;
    use djls_semantic::EndTag;
    use djls_semantic::TagArgument;
    use djls_semantic::TagSpec;
    use djls_source::Span;

    use super::*;

    fn prefix(text: &'static str) -> OffsetPrefix<'static> {
        OffsetPrefix {
            text,
            span: Span::before_offset(Offset::new(u32::try_from(text.len()).unwrap()), text.len()),
        }
    }

    fn suffix(text: &'static str, start: u32) -> OffsetSuffix<'static> {
        OffsetSuffix {
            text,
            span: Span::new(start, u32::try_from(text.len()).unwrap()),
        }
    }

    fn labels(candidates: &[CompletionCandidate]) -> Vec<&str> {
        candidates
            .iter()
            .map(|candidate| candidate.label.as_str())
            .collect()
    }

    fn template_libraries(libraries: &[(&str, &str)]) -> TemplateLibraries {
        let mut loadable = BTreeMap::new();
        for (name, module) in libraries {
            let name = LibraryName::parse(name).unwrap();
            let module = PythonModulePath::parse(module).unwrap();
            loadable.insert(name, TemplateLibrary::new(module));
        }

        TemplateLibraries {
            knowledge: StaticKnowledge::Known,
            loadable,
            builtins: Vec::new(),
        }
    }

    fn template_symbol(
        kind: TemplateSymbolKind,
        name: &str,
        module: &PythonModulePath,
        doc: Option<&str>,
    ) -> TemplateSymbol {
        TemplateSymbol {
            kind,
            name: TemplateSymbolName::parse(name).unwrap(),
            definition: SymbolDefinition::Module(module.clone()),
            doc: doc.map(str::to_string),
        }
    }

    fn filter_libraries() -> TemplateLibraries {
        let library_name = LibraryName::parse("i18n").unwrap();
        let module = PythonModulePath::parse("django.templatetags.i18n").unwrap();
        let mut library = TemplateLibrary::new(module.clone());
        library.symbols.push(template_symbol(
            TemplateSymbolKind::Filter,
            "trans",
            &module,
            Some("Translate text."),
        ));

        TemplateLibraries {
            knowledge: StaticKnowledge::Known,
            loadable: BTreeMap::from([(library_name, library)]),
            builtins: Vec::new(),
        }
    }

    fn tag_libraries() -> TemplateLibraries {
        let builtin_module = PythonModulePath::parse("django.template.defaulttags").unwrap();
        let mut builtin = TemplateLibrary::new(builtin_module.clone());
        builtin.symbols.push(template_symbol(
            TemplateSymbolKind::Tag,
            "if",
            &builtin_module,
            None,
        ));

        let i18n_name = LibraryName::parse("i18n").unwrap();
        let i18n_module = PythonModulePath::parse("django.templatetags.i18n").unwrap();
        let mut i18n = TemplateLibrary::new(i18n_module.clone());
        i18n.symbols.push(template_symbol(
            TemplateSymbolKind::Tag,
            "trans",
            &i18n_module,
            None,
        ));
        i18n.symbols.push(template_symbol(
            TemplateSymbolKind::Tag,
            "blocktrans",
            &i18n_module,
            None,
        ));

        TemplateLibraries {
            knowledge: StaticKnowledge::Known,
            loadable: BTreeMap::from([(i18n_name, i18n)]),
            builtins: vec![builtin],
        }
    }

    fn builtin_origin() -> InstalledSymbolOrigin {
        InstalledSymbolOrigin::Builtin {
            module: PythonModulePath::parse("django.template.defaulttags").unwrap(),
        }
    }

    fn full_close() -> TagClose {
        TagClose::Full {
            replacement_suffix_len: 0,
        }
    }

    fn test_tag_symbol(name: &str) -> TemplateSymbol {
        let module = PythonModulePath::parse("django.template.defaulttags").unwrap();
        template_symbol(TemplateSymbolKind::Tag, name, &module, None)
    }

    fn choice_tag_specs() -> TagSpecs {
        let mut specs = TagSpecs::default();
        specs.insert(
            "cache".to_string(),
            TagSpec::new(Cow::Borrowed("test.tags"), None, Cow::Borrowed(&[]), false)
                .with_arguments(vec![TagArgument {
                    name: "fragment_name".to_string(),
                    required: true,
                    kind: TagArgumentKind::Choice(vec![
                        "sidebar".to_string(),
                        "site_header".to_string(),
                    ]),
                    position: 0,
                }]),
        );
        specs
    }

    fn block_tag_spec() -> TagSpec {
        TagSpec::new(
            Cow::Borrowed("test.tags"),
            Some(EndTag {
                name: Cow::Borrowed("endblock"),
                required: true,
            }),
            Cow::Borrowed(&[]),
            false,
        )
        .with_arguments(vec![TagArgument {
            name: "name".to_string(),
            required: true,
            kind: TagArgumentKind::Variable,
            position: 0,
        }])
    }

    #[test]
    fn generates_library_name_candidates() {
        let libraries = template_libraries(&[
            ("i18n", "django.templatetags.i18n"),
            ("static", "django.templatetags.static"),
        ]);
        let suffix = suffix("", 2);
        let candidates =
            generate_library_name_candidates(&prefix("st"), &suffix, TagClose::None, &libraries);

        assert_eq!(labels(&candidates), vec!["static"]);
        assert_eq!(candidates[0].kind, CompletionCandidateKind::LibraryName);
        assert_eq!(candidates[0].edit.replacement_span, Span::new(0, 2));
        assert_eq!(candidates[0].edit.insert_text, "static %}");
        assert_eq!(
            candidates[0].edit.insert_format,
            CompletionInsertFormat::PlainText,
        );
        assert_eq!(
            candidates[0].detail.as_deref(),
            Some("Django template library (django.templatetags.static)")
        );
    }

    #[test]
    fn library_name_candidate_replaces_source_suffix_without_consuming_full_close() {
        let libraries = template_libraries(&[("static", "django.templatetags.static")]);
        let suffix = suffix("i18n", 0);
        let candidates = generate_library_name_candidates(
            &prefix(""),
            &suffix,
            TagClose::Full {
                replacement_suffix_len: 3,
            },
            &libraries,
        );

        assert_eq!(labels(&candidates), vec!["static"]);
        assert_eq!(candidates[0].edit.replacement_span, Span::new(0, 4));
        assert_eq!(candidates[0].edit.insert_text, "static");
    }

    #[test]
    fn generates_load_symbol_candidates() {
        let libraries = tag_libraries();
        let suffix = suffix("", 5);
        let candidates = generate_load_symbol_candidates(
            &prefix("trans"),
            &suffix,
            Some("i18n"),
            false,
            &libraries,
        );

        assert_eq!(labels(&candidates), vec!["trans"]);
        assert_eq!(candidates[0].kind, CompletionCandidateKind::LoadSymbol);
        assert_eq!(candidates[0].edit.replacement_span, Span::new(0, 5));
        assert_eq!(candidates[0].edit.insert_text, "trans");
    }

    #[test]
    fn load_symbol_candidate_adds_space_before_from_keyword() {
        let libraries = tag_libraries();
        let suffix = suffix("", 0);
        let candidates =
            generate_load_symbol_candidates(&prefix(""), &suffix, Some("i18n"), true, &libraries);

        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.edit.insert_text == "trans ")
        );
    }

    #[test]
    fn generates_tag_argument_choice_candidates() {
        let candidates = generate_tag_argument_candidates(
            "cache",
            0,
            &prefix("si"),
            full_close(),
            &choice_tag_specs(),
            false,
        );

        assert_eq!(labels(&candidates), vec!["sidebar", "site_header"]);
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.kind == CompletionCandidateKind::TagArgumentChoice)
        );
        assert_eq!(candidates[0].edit.replacement_span, Span::new(0, 2));
        assert_eq!(candidates[0].edit.insert_text, "sidebar");
        assert_eq!(
            candidates[0].detail.as_deref(),
            Some("choice for fragment_name")
        );
    }

    #[test]
    fn generates_remaining_argument_snippet_candidates() {
        let candidates = generate_tag_argument_candidates(
            "cache",
            0,
            &prefix(""),
            full_close(),
            &choice_tag_specs(),
            true,
        );

        let snippet = candidates
            .iter()
            .find(|candidate| candidate.kind == CompletionCandidateKind::TagArgumentSnippet)
            .expect("expected remaining-arguments snippet");

        assert_eq!(snippet.label, "cache arguments");
        assert_eq!(snippet.edit.insert_format, CompletionInsertFormat::Snippet);
        assert_eq!(snippet.edit.insert_text, "${1|sidebar,site_header|}");
        assert_eq!(
            snippet.detail.as_deref(),
            Some("Complete remaining arguments")
        );
    }

    #[test]
    fn tag_snippet_with_full_close_replaces_existing_close() {
        let origin = builtin_origin();
        let spec = block_tag_spec();
        let candidate = CompletionCandidate::tag_name(
            &test_tag_symbol("block"),
            &prefix("blo"),
            false,
            TagClose::Full {
                replacement_suffix_len: 3,
            },
            Some(&spec),
            &origin,
            true,
        );

        assert_eq!(
            candidate.edit.insert_format,
            CompletionInsertFormat::Snippet
        );
        assert_eq!(candidate.edit.replacement_span, Span::new(0, 6));
        assert_eq!(
            candidate.edit.insert_text,
            "block ${1:name} %}\n$0\n{% endblock ${1} %}"
        );
    }

    #[test]
    fn filter_candidates_include_detail_and_documentation() {
        let candidates = generate_filter_candidates(&prefix("tr"), &filter_libraries(), None);

        assert_eq!(labels(&candidates), vec!["trans"]);
        assert_eq!(candidates[0].detail.as_deref(), Some("{% load i18n %}"));
        assert_eq!(
            candidates[0].documentation.as_deref(),
            Some("Translate text.")
        );
    }

    #[test]
    fn tag_candidates_fall_back_to_specs_when_libraries_are_unknown() {
        let mut specs = TagSpecs::default();
        specs.insert(
            "static".to_string(),
            TagSpec::new(
                Cow::Borrowed("django.templatetags.static"),
                None,
                Cow::Borrowed(&[]),
                false,
            ),
        );

        let candidates = generate_tag_name_candidates(
            &prefix("sta"),
            false,
            full_close(),
            TemplateLibraries::empty_ref(),
            &specs,
            None,
            false,
        );

        assert_eq!(labels(&candidates), vec!["static"]);
        assert_eq!(candidates[0].detail.as_deref(), Some("Django template tag"));
    }

    #[test]
    fn partial_tag_close_extends_replacement_span() {
        let origin = builtin_origin();
        let candidate = CompletionCandidate::tag_name(
            &test_tag_symbol("load"),
            &prefix("lo"),
            false,
            TagClose::Partial {
                replacement_suffix_len: 1,
            },
            None,
            &origin,
            false,
        );

        assert_eq!(candidate.edit.replacement_span, Span::new(0, 3));
        assert_eq!(candidate.edit.insert_text, "load %}");
    }

    #[test]
    fn partial_tag_close_after_whitespace_extends_replacement_span_to_close() {
        let origin = builtin_origin();
        let candidate = CompletionCandidate::tag_name(
            &test_tag_symbol("load"),
            &prefix("lo"),
            false,
            TagClose::Partial {
                replacement_suffix_len: 2,
            },
            None,
            &origin,
            false,
        );

        assert_eq!(candidate.edit.replacement_span, Span::new(0, 4));
        assert_eq!(candidate.edit.insert_text, "load %}");
    }

    #[test]
    fn ranks_candidates_by_relevance_then_label() {
        let empty = prefix("");
        let origin = builtin_origin();
        let mut candidates = vec![
            CompletionCandidate::tag_argument_placeholder("<arg>".to_string(), &empty),
            CompletionCandidate::tag_name(
                &test_tag_symbol("url"),
                &empty,
                false,
                full_close(),
                None,
                &origin,
                false,
            ),
            CompletionCandidate::end_tag("if", "endif", &empty, false, full_close()),
            CompletionCandidate::tag_name(
                &test_tag_symbol("block"),
                &empty,
                false,
                full_close(),
                None,
                &origin,
                false,
            ),
        ];

        candidates.sort_by(CompletionCandidate::cmp_rank);

        assert_eq!(labels(&candidates), vec!["endif", "block", "url", "<arg>"]);
    }
}
