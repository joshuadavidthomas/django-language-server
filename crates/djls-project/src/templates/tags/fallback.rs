//! Fallback Rules: hand-written Django facts for Extraction Gaps.
//!
//! Django template knowledge is extracted from source. An entry belongs here only when the
//! interpreter cannot prove the fact, and it must name the Extraction Gap and cite bundled Django
//! source as evidence. Entries target Django's compile functions by canonical identity, so a
//! library that registers `do_include` under another name gets the same meaning, and a function
//! with the same name in another module does not.
//!
//! Evidence cites the bundled Django 5.2 archive in `vendor/`.

use crate::python::PythonFunctionDefinition;
use crate::templates::tags::meaning::BodyLexing;
use crate::templates::tags::meaning::ConditionArguments;
use crate::templates::tags::meaning::FactProvenance;
use crate::templates::tags::meaning::FallbackCitation;
use crate::templates::tags::meaning::TagMeaning;
use crate::templates::tags::meaning::TagRole;
use crate::templates::tags::meaning::TemplateReferenceKind;

/// Who a Fallback Rule applies to.
enum Target {
    /// A compile function, by its defining module and name.
    Function {
        module: &'static str,
        function: &'static str,
    },
    /// Every Tag registered under this name. Django's Lexer decides by tag text before any
    /// library is consulted.
    LexerTagName(&'static str),
}

enum Fact {
    Role(TagRole),
    Condition {
        intermediates: &'static [&'static str],
    },
    BodyLexing(BodyLexing),
}

/// A line of bundled Django 5.2 source that shows the gap.
struct Evidence {
    path: &'static str,
    line: usize,
    /// Text that appears on that line, so a stale citation fails tests.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "checked against the bundle in tests")
    )]
    shows: &'static str,
}

struct FallbackRule {
    target: Target,
    fact: Fact,
    gap: &'static str,
    evidence: Evidence,
}

const RULES: &[FallbackRule] = &[
    FallbackRule {
        target: Target::Function {
            module: "django.template.loader_tags",
            function: "do_block",
        },
        fact: Fact::Role(TagRole::TemplateBlock),
        gap: "Inheritance Blocks exist only through BlockNode and ExtendsNode at render time; \
              the compile function calls no parser API that marks the definition.",
        evidence: Evidence {
            path: "django/template/loader_tags.py",
            line: 242,
            shows: "return BlockNode(",
        },
    },
    FallbackRule {
        target: Target::Function {
            module: "django.template.loader_tags",
            function: "do_extends",
        },
        fact: Fact::Role(TagRole::TemplateReference(TemplateReferenceKind::Extends)),
        gap: "The parent template is loaded by ExtendsNode at render time; extraction does not \
              follow node classes into the loader.",
        evidence: Evidence {
            path: "django/template/loader_tags.py",
            line: 304,
            shows: "return ExtendsNode(",
        },
    },
    FallbackRule {
        target: Target::Function {
            module: "django.template.loader_tags",
            function: "do_include",
        },
        fact: Fact::Role(TagRole::TemplateReference(TemplateReferenceKind::Include)),
        gap: "The included template is loaded by IncludeNode at render time; extraction does \
              not follow node classes into the loader.",
        evidence: Evidence {
            path: "django/template/loader_tags.py",
            line: 358,
            shows: "return IncludeNode(",
        },
    },
    FallbackRule {
        target: Target::Function {
            module: "django.template.defaulttags",
            function: "url",
        },
        fact: Fact::Role(TagRole::RouteReference),
        gap: "URLNode reverses the route at render time; extraction does not model URL \
              resolution.",
        evidence: Evidence {
            path: "django/template/defaulttags.py",
            line: 1440,
            shows: "return URLNode(",
        },
    },
    FallbackRule {
        target: Target::Function {
            module: "django.templatetags.static",
            function: "do_static",
        },
        fact: Fact::Role(TagRole::StaticAssetReference),
        gap: "StaticNode resolves the path through the staticfiles storage at render time; \
              extraction does not model storages.",
        evidence: Evidence {
            path: "django/templatetags/static.py",
            line: 171,
            shows: "return StaticNode.handle_token(",
        },
    },
    FallbackRule {
        target: Target::Function {
            module: "django.template.defaulttags",
            function: "do_if",
        },
        fact: Fact::Condition {
            intermediates: &["elif"],
        },
        gap: "Extraction does not record Argument Consumption; the opener's and each elif's \
              bits go to TemplateIfParser.",
        evidence: Evidence {
            path: "django/template/defaulttags.py",
            line: 969,
            shows: "condition = TemplateIfParser(parser, bits).parse()",
        },
    },
    FallbackRule {
        target: Target::LexerTagName("verbatim"),
        fact: Fact::BodyLexing(BodyLexing::Verbatim),
        gap: "The Lexer, not do_verbatim, emits the body as text until a tag whose contents \
              are 'end' plus the opener's contents.",
        evidence: Evidence {
            path: "django/template/base.py",
            line: 390,
            shows: r#"self.verbatim = "end%s" % content"#,
        },
    },
];

/// Apply Fallback Rules to one Tag registration, leaving extracted facts in place.
pub(crate) fn apply_fallback_rules(
    meaning: &mut TagMeaning,
    name: &str,
    function: Option<&PythonFunctionDefinition>,
) {
    for rule in RULES {
        let applies = match rule.target {
            Target::Function {
                module,
                function: expected,
            } => function.is_some_and(|function| is_django_function(function, module, expected)),
            Target::LexerTagName(tag_name) => name == tag_name,
        };
        if !applies {
            continue;
        }
        let citation = FallbackCitation::new(rule.gap, rule.evidence.path, rule.evidence.line);
        match &rule.fact {
            Fact::Role(role) => {
                if meaning.role().is_some() {
                    continue;
                }
                meaning.set_role(*role, FactProvenance::Fallback);
            }
            Fact::Condition { intermediates } => meaning.set_condition(ConditionArguments::new(
                intermediates.iter().map(ToString::to_string).collect(),
            )),
            Fact::BodyLexing(lexing) => meaning.set_body_lexing(*lexing),
        }
        meaning.cite_fallback(citation);
    }
}

/// Matches by canonical module identity, so a Django checkout opened as the project keeps its
/// meaning; a namesake in any other module does not.
fn is_django_function(function: &PythonFunctionDefinition, module: &str, name: &str) -> bool {
    function.name() == name
        && function
            .module()
            .is_some_and(|source| source.name().as_str() == module)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use djls_conf::DjangoVersion;
    use djls_source::FileSystem;
    use djls_source::InMemoryFileSystem;

    use super::*;
    use crate::bundled::BundledFileSystem;
    use crate::bundled::source_root;

    #[test]
    fn every_rule_names_its_gap_and_cites_bundled_source() {
        let root = source_root(DjangoVersion::Django52).expect("bundled Django 5.2");
        let fs = BundledFileSystem::new(Arc::new(InMemoryFileSystem::new()));
        for rule in RULES {
            assert!(!rule.gap.is_empty());
            let source = fs
                .read_to_string(&root.join(rule.evidence.path))
                .expect("cited file should exist in the bundle");
            let line = source
                .lines()
                .nth(rule.evidence.line - 1)
                .expect("cited line should exist");
            assert!(
                line.contains(rule.evidence.shows),
                "{}:{} should show {:?}, found {line:?}",
                rule.evidence.path,
                rule.evidence.line,
                rule.evidence.shows,
            );
            if let Target::Function { function, .. } = rule.target {
                assert!(source.contains(&format!("def {function}(parser, token):")));
            }
        }
    }
}
