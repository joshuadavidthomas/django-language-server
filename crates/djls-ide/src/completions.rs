//! Completion flow:
//! 1. Locate source text from `File` and decide whether this file supports completions.
//! 2. Read the `CompletionOffsetContext` for the offset.
//! 3. Generate candidates relevant to that context.
//! 4. Decide edit semantics for each candidate: replacement range, insert text, and snippets.
//! 5. Rank candidates by relevance.
//! 6. Convert candidates into an LSP completion response using client/session facts.

use djls_project::EnvironmentSymbolLookup;
use djls_project::LoadableLibraryLookup;
use djls_project::TemplateEnvironment;
use djls_project::TemplateLibrary;
use djls_project::TemplateSymbol;
use djls_project::TemplateSymbolAvailability;
use djls_project::TemplateSymbolCandidate;
use djls_project::TemplateSymbolKind;
use djls_project::template_resolution;
use djls_semantic::Db as SemanticDb;
use djls_semantic::TagArgumentKind;
use djls_semantic::TagRole;
use djls_semantic::TagSpec;
use djls_semantic::TagSpecs;
use djls_semantic::effective_symbol_candidate_at;
use djls_semantic::tag_spec_at;
use djls_semantic::tag_specs_at;
use djls_semantic::tag_specs_for_file;
use djls_semantic::template_environment_for_file;
use djls_source::File;
use djls_source::FileKind;
use djls_source::Offset;
use djls_source::PositionEncoding;
use djls_source::Span;
use djls_templates::NodeList;
use djls_templates::TemplateParseResult;
use djls_templates::parse_template;
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
    TemplateName,
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
            | Self::TemplateName
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
        spec: &TagSpec,
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

    fn template_name(
        name: &str,
        quote: char,
        prefix: &OffsetPrefix<'_>,
        suffix: &OffsetSuffix<'_>,
        closed: bool,
        close: TagClose,
    ) -> Self {
        let mut insert_text = name.to_string();
        let close_suffix_len = match (closed, close) {
            (true, TagClose::Full { .. }) => 0,
            (true, close) => {
                insert_text.push(quote);
                Self::append_tag_close(&mut insert_text, close);
                quote.len_utf8() + close.partial_replacement_suffix_len()
            }
            (false, close) => {
                insert_text.push(quote);
                Self::append_tag_close(&mut insert_text, close);
                close.partial_replacement_suffix_len()
            }
        };

        Self::plain(
            Self::span_with_source_suffix(prefix, suffix, close_suffix_len),
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
        symbol: &TemplateSymbol,
        prefix: &OffsetPrefix<'_>,
        needs_leading_space: bool,
        close: TagClose,
        spec: Option<&TagSpec>,
        availability: &TemplateSymbolAvailability,
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
            detail: Some(tag_completion_detail(availability)),
            documentation: symbol.doc().map(str::to_string),
        }
    }

    fn tag_name_from_spec(
        name: &str,
        prefix: &OffsetPrefix<'_>,
        needs_leading_space: bool,
        close: TagClose,
        spec: &TagSpec,
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

    fn template_name(
        name: &str,
        quote: char,
        prefix: &OffsetPrefix<'_>,
        suffix: &OffsetSuffix<'_>,
        closed: bool,
        close: TagClose,
    ) -> Self {
        Self {
            label: name.to_string(),
            kind: CompletionCandidateKind::TemplateName,
            edit: CompletionEdit::template_name(name, quote, prefix, suffix, closed, close),
            detail: Some("Django template".to_string()),
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
        availability: &TemplateSymbolAvailability,
        documentation: Option<&str>,
    ) -> Self {
        Self {
            label: name.to_string(),
            kind: CompletionCandidateKind::Filter,
            edit: CompletionEdit::plain(prefix.span, name),
            detail: Some(filter_completion_detail(availability)),
            documentation: documentation.map(str::to_string),
        }
    }
}

fn tag_completion_detail(availability: &TemplateSymbolAvailability) -> String {
    match availability {
        TemplateSymbolAvailability::Builtin { module } => {
            format!("builtin from {}", module.as_str())
        }
        TemplateSymbolAvailability::RequiresLoad { load_name } => {
            format!("{{% load {} %}}", load_name.as_str())
        }
    }
}

fn filter_completion_detail(availability: &TemplateSymbolAvailability) -> String {
    match availability {
        TemplateSymbolAvailability::Builtin { module: _ } => "builtin filter".to_string(),
        TemplateSymbolAvailability::RequiresLoad { load_name } => {
            format!("{{% load {} %}}", load_name.as_str())
        }
    }
}

// Completion dispatch intentionally keeps all context variants in one exhaustive match so adding
// a parser context forces the IDE translation to handle it here.
#[allow(clippy::too_many_lines)]
#[must_use]
pub fn completion(
    db: &dyn SemanticDb,
    file: File,
    offset: Offset,
    encoding: PositionEncoding,
    supports_snippets: bool,
) -> Option<ls_types::CompletionResponse> {
    let Ok(source) = file.try_source(db) else {
        return None;
    };
    if *source.kind() != FileKind::Template {
        return None;
    }

    let Ok(tokens) = djls_templates::lex_template(db, file) else {
        return None;
    };
    let context = CompletionOffsetContext::new(*source.kind(), source.as_str(), tokens, offset);

    // Dispatch on the syntax-only cursor context before requesting semantic products. Most
    // completion contexts need either no tag meaning or one occurrence lookup; only tag-name
    // completion enumerates the complete tag inventory.
    let mut candidates = match &context {
        CompletionOffsetContext::Template(TemplateCompletionContext::TagName {
            prefix,
            needs_leading_space,
            close,
        }) => {
            let environment = template_environment_for_file(db, file);
            let nodelist = parsed_nodelist(db, file);
            let tag_specs = nodelist.map_or_else(
                || tag_specs_for_file(db, file),
                |nodelist| tag_specs_at(db, file, nodelist, offset.get()),
            );
            generate_tag_name_candidates(
                db,
                file,
                nodelist,
                offset,
                environment,
                TagNameCandidateInput {
                    prefix,
                    needs_leading_space: *needs_leading_space,
                    close: *close,
                    tag_specs,
                    supports_snippets,
                },
            )
        }
        CompletionOffsetContext::Template(TemplateCompletionContext::TagArgument {
            tag,
            position,
            prefix,
            close,
            ..
        }) => {
            let spec = parsed_nodelist(db, file)
                .and_then(|nodelist| tag_spec_at(db, file, nodelist, offset.get(), tag));
            generate_tag_argument_candidates(
                tag,
                *position,
                prefix,
                *close,
                spec.as_ref(),
                supports_snippets,
            )
        }
        CompletionOffsetContext::Template(TemplateCompletionContext::QuotedArgument {
            tag,
            position,
            quote,
            prefix,
            suffix,
            closed,
            close,
        }) => {
            let spec = parsed_nodelist(db, file)
                .and_then(|nodelist| tag_spec_at(db, file, nodelist, offset.get(), tag));
            generate_template_name_candidates(
                db,
                Some(file),
                TemplateNameCandidateInput {
                    position: *position,
                    quote: *quote,
                    prefix,
                    suffix,
                    closed: *closed,
                    close: *close,
                },
                spec.as_ref(),
            )
        }
        CompletionOffsetContext::Template(TemplateCompletionContext::LibraryName {
            prefix,
            suffix,
            close,
        }) => {
            if effective_tag_role_at(db, file, offset, "load")
                == Some(TagRole::TemplateLibraryLoader)
            {
                generate_library_name_candidates(
                    template_environment_for_file(db, file),
                    prefix,
                    suffix,
                    *close,
                )
            } else {
                Vec::new()
            }
        }
        CompletionOffsetContext::Template(TemplateCompletionContext::LoadSymbol {
            prefix,
            suffix,
            library,
            needs_trailing_space,
        }) => {
            if effective_tag_role_at(db, file, offset, "load")
                == Some(TagRole::TemplateLibraryLoader)
            {
                generate_load_symbol_candidates(
                    prefix,
                    suffix,
                    *library,
                    *needs_trailing_space,
                    template_environment_for_file(db, file),
                )
            } else {
                Vec::new()
            }
        }
        CompletionOffsetContext::Template(TemplateCompletionContext::Filter { prefix }) => {
            generate_filter_candidates(
                db,
                file,
                parsed_nodelist(db, file),
                offset,
                template_environment_for_file(db, file),
                prefix,
            )
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

fn parsed_nodelist(db: &dyn SemanticDb, file: File) -> Option<NodeList<'_>> {
    match parse_template(db, file) {
        TemplateParseResult::Parsed(nodelist) => Some(nodelist),
        TemplateParseResult::NotTemplate | TemplateParseResult::Unreadable(_) => None,
    }
}

fn effective_tag_role_at(
    db: &dyn SemanticDb,
    file: File,
    offset: Offset,
    tag: &str,
) -> Option<TagRole> {
    let nodelist = parsed_nodelist(db, file)?;
    tag_spec_at(db, file, nodelist, offset.get(), tag)
        .as_ref()
        .and_then(TagSpec::role)
}

#[derive(Clone, Copy)]
struct TagNameCandidateInput<'a> {
    prefix: &'a OffsetPrefix<'a>,
    needs_leading_space: bool,
    close: TagClose,
    tag_specs: &'a TagSpecs,
    supports_snippets: bool,
}

fn completion_symbol_candidates(
    db: &dyn SemanticDb,
    file: File,
    nodelist: Option<NodeList<'_>>,
    offset: Offset,
    environment: TemplateEnvironment<'_>,
    name: &str,
    kind: TemplateSymbolKind,
) -> Vec<TemplateSymbolCandidate> {
    nodelist.map_or_else(
        || environment.contextual_symbol_candidates(name, kind),
        |nodelist| {
            effective_symbol_candidate_at(db, file, nodelist, offset.get(), name, kind)
                .into_iter()
                .collect()
        },
    )
}

fn environment_has_definite_symbols(
    environment: TemplateEnvironment<'_>,
    kind: TemplateSymbolKind,
) -> bool {
    environment.inventory_symbol_names(kind).any(|name| {
        matches!(
            environment.symbol(name, kind),
            EnvironmentSymbolLookup::Builtin | EnvironmentSymbolLookup::RequiresLoad(_)
        )
    })
}

fn generate_tag_name_candidates(
    db: &dyn SemanticDb,
    file: File,
    nodelist: Option<NodeList<'_>>,
    offset: Offset,
    environment: TemplateEnvironment<'_>,
    input: TagNameCandidateInput<'_>,
) -> Vec<CompletionCandidate> {
    let TagNameCandidateInput {
        prefix,
        needs_leading_space,
        close,
        tag_specs,
        supports_snippets,
    } = input;
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

    let contextual_candidate_start = candidates.len();
    for name in environment
        .inventory_symbol_names(TemplateSymbolKind::Tag)
        .filter(|name| name.starts_with(prefix.text))
    {
        for candidate in completion_symbol_candidates(
            db,
            file,
            nodelist,
            offset,
            environment,
            name,
            TemplateSymbolKind::Tag,
        ) {
            candidates.push(CompletionCandidate::tag_name(
                &candidate.symbol,
                prefix,
                needs_leading_space,
                close,
                tag_specs.get(name),
                &candidate.availability,
                supports_snippets,
            ));
        }
    }

    if candidates.len() == contextual_candidate_start
        && !environment_has_definite_symbols(environment, TemplateSymbolKind::Tag)
    {
        for (name, spec) in tag_specs {
            if name.starts_with(prefix.text) {
                candidates.push(CompletionCandidate::tag_name_from_spec(
                    name,
                    prefix,
                    needs_leading_space,
                    close,
                    spec,
                    supports_snippets,
                ));
            }
        }
    }

    candidates
}

fn generate_tag_argument_candidates(
    tag: &str,
    position: usize,
    prefix: &OffsetPrefix<'_>,
    close: TagClose,
    spec: Option<&TagSpec>,
    supports_snippets: bool,
) -> Vec<CompletionCandidate> {
    let Some(spec) = spec else {
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

#[derive(Clone, Copy)]
struct TemplateNameCandidateInput<'context, 'source> {
    position: usize,
    quote: char,
    prefix: &'context OffsetPrefix<'source>,
    suffix: &'context OffsetSuffix<'source>,
    closed: bool,
    close: TagClose,
}

fn generate_template_name_candidates(
    db: &dyn SemanticDb,
    file: Option<File>,
    input: TemplateNameCandidateInput<'_, '_>,
    spec: Option<&TagSpec>,
) -> Vec<CompletionCandidate> {
    let Some(spec) = spec else {
        return Vec::new();
    };
    if !matches!(spec.role(), Some(TagRole::TemplateReference(_))) || input.position != 0 {
        return Vec::new();
    }

    let Some(project) = db.project() else {
        return Vec::new();
    };

    let resolution = template_resolution(db, project);
    let names = match file {
        Some(file) => resolution.template_names_for_backend_scope(db, file),
        None => resolution.template_names(db).collect(),
    };
    names
        .into_iter()
        .filter_map(|name| {
            let name = name.name(db);
            name.starts_with(input.prefix.text).then(|| {
                CompletionCandidate::template_name(
                    name,
                    input.quote,
                    input.prefix,
                    input.suffix,
                    input.closed,
                    input.close,
                )
            })
        })
        .collect()
}

fn generate_library_name_candidates(
    environment: TemplateEnvironment<'_>,
    prefix: &OffsetPrefix<'_>,
    suffix: &OffsetSuffix<'_>,
    close: TagClose,
) -> Vec<CompletionCandidate> {
    let mut candidates = Vec::new();
    for name in environment.completion_library_names() {
        if !name.as_str().starts_with(prefix.text) {
            continue;
        }

        let detail = match environment.loadable_library(&name) {
            LoadableLibraryLookup::Found(library) => {
                format!("Django template library ({})", library.module_name_str())
            }
            LoadableLibraryLookup::Ambiguous(_)
            | LoadableLibraryLookup::Inconclusive(_)
            | LoadableLibraryLookup::Absent => "Django template library".to_string(),
        };
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
    environment: TemplateEnvironment<'_>,
) -> Vec<CompletionCandidate> {
    let Some(name) = library else {
        return Vec::new();
    };
    let libraries = match environment.loadable_library_str(name) {
        LoadableLibraryLookup::Found(library) => vec![library],
        LoadableLibraryLookup::Ambiguous(libraries)
        | LoadableLibraryLookup::Inconclusive(libraries) => libraries,
        LoadableLibraryLookup::Absent => Vec::new(),
    };
    let mut candidates = libraries
        .into_iter()
        .flat_map(TemplateLibrary::symbols)
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
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.label.cmp(&right.label));
    candidates.dedup_by(|left, right| left.label == right.label);
    candidates
}

fn generate_filter_candidates(
    db: &dyn SemanticDb,
    file: File,
    nodelist: Option<NodeList<'_>>,
    offset: Offset,
    environment: TemplateEnvironment<'_>,
    prefix: &OffsetPrefix<'_>,
) -> Vec<CompletionCandidate> {
    let mut candidates = Vec::new();
    for name in environment
        .inventory_symbol_names(TemplateSymbolKind::Filter)
        .filter(|name| name.starts_with(prefix.text))
    {
        for candidate in completion_symbol_candidates(
            db,
            file,
            nodelist,
            offset,
            environment,
            name,
            TemplateSymbolKind::Filter,
        ) {
            candidates.push(CompletionCandidate::filter(
                name,
                prefix,
                &candidate.availability,
                candidate.symbol.doc(),
            ));
        }
    }

    candidates.sort_by(|left, right| left.label.cmp(&right.label));
    candidates.dedup_by(|left, right| left.label == right.label);
    candidates
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::collections::HashMap;

    use camino::Utf8Path;
    use djls_project::PythonModuleName;
    use djls_project::SymbolDefinition;
    use djls_project::TemplateLibraries;
    use djls_project::TemplateSymbol;
    use djls_project::TemplateSymbolName;
    use djls_semantic::EndTag;
    use djls_semantic::TagArgument;
    use djls_semantic::TagSpec;
    use djls_source::Span;
    use djls_testing::TestDatabase;

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
        let libraries = libraries
            .iter()
            .map(|(name, module)| ((*name).to_string(), (*module).to_string()))
            .collect::<HashMap<_, _>>();
        let db = TestDatabase::new();
        djls_testing::make_template_libraries(&db, &[], &[], &libraries, &[])
    }

    fn template_symbol(
        kind: TemplateSymbolKind,
        name: &str,
        module: &PythonModuleName,
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
        let libraries =
            HashMap::from([("i18n".to_string(), "django.templatetags.i18n".to_string())]);
        let mut filter = djls_testing::library_filter("trans", "i18n", "django.templatetags.i18n");
        filter["doc"] = "Translate text.".into();

        let db = TestDatabase::new();
        djls_testing::make_template_libraries(&db, &[], &[filter], &libraries, &[])
    }

    fn tag_libraries() -> TemplateLibraries {
        let builtins = vec!["django.template.defaulttags".to_string()];
        let libraries =
            HashMap::from([("i18n".to_string(), "django.templatetags.i18n".to_string())]);
        let tags = vec![
            djls_testing::builtin_tag("if", "django.template.defaulttags"),
            djls_testing::library_tag("trans", "i18n", "django.templatetags.i18n"),
            djls_testing::library_tag("blocktrans", "i18n", "django.templatetags.i18n"),
        ];

        let db = TestDatabase::new();
        djls_testing::make_template_libraries(&db, &tags, &[], &libraries, &builtins)
    }

    fn builtin_availability() -> TemplateSymbolAvailability {
        let libraries = tag_libraries();
        TemplateEnvironment::from_project_inventory(&libraries)
            .contextual_symbol_candidates("if", TemplateSymbolKind::Tag)
            .into_iter()
            .next()
            .expect("builtin if candidate should exist")
            .availability
    }

    fn full_close() -> TagClose {
        TagClose::Full {
            replacement_suffix_len: 0,
        }
    }

    fn test_tag_symbol(name: &str) -> TemplateSymbol {
        let module = PythonModuleName::parse("django.template.defaulttags").unwrap();
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
        let environment = TemplateEnvironment::from_project_inventory(&libraries);
        let candidates =
            generate_library_name_candidates(environment, &prefix("st"), &suffix, TagClose::None);

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
        let environment = TemplateEnvironment::from_project_inventory(&libraries);
        let candidates = generate_library_name_candidates(
            environment,
            &prefix(""),
            &suffix,
            TagClose::Full {
                replacement_suffix_len: 3,
            },
        );

        assert_eq!(labels(&candidates), vec!["static"]);
        assert_eq!(candidates[0].edit.replacement_span, Span::new(0, 4));
        assert_eq!(candidates[0].edit.insert_text, "static");
    }

    #[test]
    fn generates_load_symbol_candidates() {
        let libraries = tag_libraries();
        let suffix = suffix("", 5);
        let environment = TemplateEnvironment::from_project_inventory(&libraries);
        let candidates = generate_load_symbol_candidates(
            &prefix("trans"),
            &suffix,
            Some("i18n"),
            false,
            environment,
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
        let environment = TemplateEnvironment::from_project_inventory(&libraries);
        let candidates =
            generate_load_symbol_candidates(&prefix(""), &suffix, Some("i18n"), true, environment);

        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.edit.insert_text == "trans ")
        );
    }

    #[test]
    fn generates_tag_argument_choice_candidates() {
        let specs = choice_tag_specs();
        let candidates = generate_tag_argument_candidates(
            "cache",
            0,
            &prefix("si"),
            full_close(),
            specs.get("cache"),
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
        let specs = choice_tag_specs();
        let candidates = generate_tag_argument_candidates(
            "cache",
            0,
            &prefix(""),
            full_close(),
            specs.get("cache"),
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
    fn template_name_candidate_closes_open_quote_and_tag() {
        let candidate = CompletionCandidate::template_name(
            "base.html",
            '"',
            &prefix("ba"),
            &suffix("", 2),
            false,
            TagClose::None,
        );

        assert_eq!(candidate.kind, CompletionCandidateKind::TemplateName);
        assert_eq!(candidate.edit.replacement_span, Span::new(0, 2));
        assert_eq!(candidate.edit.insert_text, "base.html\" %}");
        assert_eq!(candidate.detail.as_deref(), Some("Django template"));
    }

    #[test]
    fn template_name_candidate_preserves_existing_full_close_after_open_quote() {
        let candidate = CompletionCandidate::template_name(
            "base.html",
            '"',
            &prefix("ba"),
            &suffix("", 2),
            false,
            full_close(),
        );

        assert_eq!(candidate.edit.replacement_span, Span::new(0, 2));
        assert_eq!(candidate.edit.insert_text, "base.html\"");
    }

    #[test]
    fn template_name_candidate_replaces_autopaired_quote_and_partial_close() {
        let candidate = CompletionCandidate::template_name(
            "base.html",
            '"',
            &prefix("ba"),
            &suffix("", 2),
            true,
            TagClose::Partial {
                replacement_suffix_len: 1,
            },
        );

        assert_eq!(candidate.edit.replacement_span, Span::new(0, 4));
        assert_eq!(candidate.edit.insert_text, "base.html\" %}");
    }

    #[test]
    fn template_name_candidate_replaces_closed_quote_interior() {
        let candidate = CompletionCandidate::template_name(
            "base.html",
            '"',
            &prefix("ba"),
            &suffix("se.html", 2),
            true,
            full_close(),
        );

        assert_eq!(candidate.edit.replacement_span, Span::new(0, 9));
        assert_eq!(candidate.edit.insert_text, "base.html");
    }

    #[test]
    fn template_name_candidates_are_role_and_position_gated() {
        let db = TestDatabase::new();
        let tag_specs = djls_semantic::builtin_tag_specs();

        let prefix = prefix("");
        let suffix = suffix("", 0);

        assert!(
            generate_template_name_candidates(
                &db,
                None,
                TemplateNameCandidateInput {
                    position: 0,
                    quote: '"',
                    prefix: &prefix,
                    suffix: &suffix,
                    closed: false,
                    close: TagClose::None,
                },
                tag_specs.get("cache"),
            )
            .is_empty()
        );
        assert!(
            generate_template_name_candidates(
                &db,
                None,
                TemplateNameCandidateInput {
                    position: 1,
                    quote: '"',
                    prefix: &prefix,
                    suffix: &suffix,
                    closed: false,
                    close: TagClose::None,
                },
                tag_specs.get("extends"),
            )
            .is_empty()
        );
    }

    #[test]
    fn tag_snippet_with_full_close_replaces_existing_close() {
        let availability = builtin_availability();
        let spec = block_tag_spec();
        let candidate = CompletionCandidate::tag_name(
            &test_tag_symbol("block"),
            &prefix("blo"),
            false,
            TagClose::Full {
                replacement_suffix_len: 3,
            },
            Some(&spec),
            &availability,
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
        let db = TestDatabase::new();
        db.add_file("/test.html", "");
        let file = db.file(Utf8Path::new("/test.html"));
        let libraries = filter_libraries();
        let environment = TemplateEnvironment::from_project_inventory(&libraries);
        let candidates =
            generate_filter_candidates(&db, file, None, Offset::new(0), environment, &prefix("tr"));

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

        let db = TestDatabase::new();
        db.add_file("/test.html", "");
        let file = db.file(Utf8Path::new("/test.html"));
        let environment =
            TemplateEnvironment::from_project_inventory(TemplateLibraries::empty_ref());
        let prefix = prefix("sta");
        let candidates = generate_tag_name_candidates(
            &db,
            file,
            None,
            Offset::new(0),
            environment,
            TagNameCandidateInput {
                prefix: &prefix,
                needs_leading_space: false,
                close: full_close(),
                tag_specs: &specs,
                supports_snippets: false,
            },
        );

        assert_eq!(labels(&candidates), vec!["static"]);
        assert_eq!(candidates[0].detail.as_deref(), Some("Django template tag"));
    }

    #[test]
    fn partial_tag_close_extends_replacement_span() {
        let availability = builtin_availability();
        let candidate = CompletionCandidate::tag_name(
            &test_tag_symbol("load"),
            &prefix("lo"),
            false,
            TagClose::Partial {
                replacement_suffix_len: 1,
            },
            None,
            &availability,
            false,
        );

        assert_eq!(candidate.edit.replacement_span, Span::new(0, 3));
        assert_eq!(candidate.edit.insert_text, "load %}");
    }

    #[test]
    fn partial_tag_close_after_whitespace_extends_replacement_span_to_close() {
        let availability = builtin_availability();
        let candidate = CompletionCandidate::tag_name(
            &test_tag_symbol("load"),
            &prefix("lo"),
            false,
            TagClose::Partial {
                replacement_suffix_len: 2,
            },
            None,
            &availability,
            false,
        );

        assert_eq!(candidate.edit.replacement_span, Span::new(0, 4));
        assert_eq!(candidate.edit.insert_text, "load %}");
    }

    #[test]
    fn ranks_candidates_by_relevance_then_label() {
        let empty = prefix("");
        let availability = builtin_availability();
        let mut candidates = vec![
            CompletionCandidate::tag_argument_placeholder("<arg>".to_string(), &empty),
            CompletionCandidate::tag_name(
                &test_tag_symbol("url"),
                &empty,
                false,
                full_close(),
                None,
                &availability,
                false,
            ),
            CompletionCandidate::end_tag("if", "endif", &empty, false, full_close()),
            CompletionCandidate::tag_name(
                &test_tag_symbol("block"),
                &empty,
                false,
                full_close(),
                None,
                &availability,
                false,
            ),
        ];

        candidates.sort_by(CompletionCandidate::cmp_rank);

        assert_eq!(labels(&candidates), vec!["endif", "block", "url", "<arg>"]);
    }
}
