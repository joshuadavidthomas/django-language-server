//! Oracle report: findings with stable keys, JSON for machines and Markdown for people.

use std::fmt::Write as _;

use serde::Deserialize;
use serde::Serialize;

use crate::oracle::SettingsMode;
use crate::oracle::compare::Comparison;
use crate::oracle::compare::Coverage;

/// How DJLS's answer relates to Django's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingClass {
    /// DJLS states a definite fact that Django contradicts. Every one is a bug.
    Wrong,
    /// Django rejects a template that DJLS accepts silently.
    Missed,
    /// DJLS says it does not know, where Django has a definite answer.
    Imprecise,
    /// Reference evidence cannot safely establish this fact.
    Unchecked,
}

/// The kind of fact a finding is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    LibraryLoad,
    SymbolInventory,
    FilterArity,
    ArgumentRequiredness,
    BlockStructure,
    ReferenceIntegrity,
    Validation,
    TemplateResolution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// Stable identity used by the exact ledger: family, library, symbol, and class.
    pub(crate) key: String,
    class: FindingClass,
    family: Family,
    library: Option<String>,
    symbol: Option<String>,
    django: String,
    djls: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    note: String,
    /// The Python file DJLS extracted the library from, for fixing.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    source: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub(crate) evidence: Vec<String>,
}

impl Finding {
    pub(super) fn new(
        class: FindingClass,
        family: Family,
        library: Option<&str>,
        symbol: Option<&str>,
        django: &str,
        djls: &str,
        note: &str,
    ) -> Self {
        let family_name = serde_json::to_value(family)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default();
        let class_name = serde_json::to_value(class)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default();
        let key = format!(
            "{family_name}:{}:{}:{class_name}",
            library.unwrap_or("-"),
            symbol.unwrap_or("-")
        );
        Self {
            key,
            class,
            family,
            library: library.map(str::to_owned),
            symbol: symbol.map(str::to_owned),
            django: django.to_owned(),
            djls: djls.to_owned(),
            note: note.to_owned(),
            source: None,
            evidence: Vec::new(),
        }
    }

    pub(super) fn with_source(mut self, source: Option<&str>) -> Self {
        self.source = source.map(str::to_owned);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OracleReport {
    project: String,
    settings: SettingsMode,
    pub(crate) backend: String,
    pub(crate) python_version: Option<String>,
    django_version: Option<String>,
    notes: Vec<String>,
    coverage: Coverage,
    probe_verdicts: Vec<crate::oracle::compare::ProbeVerdict>,
    pub findings: Vec<Finding>,
}

impl OracleReport {
    pub(super) fn new(
        project: String,
        settings: SettingsMode,
        django_version: Option<String>,
        notes: Vec<String>,
        comparison: Comparison,
    ) -> Self {
        let mut findings = comparison.findings;
        findings.sort_by(|a, b| {
            (a.class, a.family, &a.library, &a.symbol, &a.key)
                .cmp(&(b.class, b.family, &b.library, &b.symbol, &b.key))
        });
        Self {
            project,
            settings,
            backend: String::new(),
            python_version: None,
            django_version,
            notes,
            coverage: comparison.coverage,
            probe_verdicts: comparison.probe_verdicts,
            findings,
        }
    }

    #[must_use]
    fn scope(&self) -> String {
        let settings = match self.settings {
            SettingsMode::Project => "project",
            SettingsMode::Synthetic => "synthetic",
        };
        format!("{}/{settings}/{}", self.project, self.backend)
    }

    #[must_use]
    fn count(&self, class: FindingClass) -> usize {
        self.findings
            .iter()
            .filter(|finding| finding.class == class)
            .count()
    }
}

pub fn render_markdown(reports: &[OracleReport]) -> Result<String, std::fmt::Error> {
    let mut out = String::from("# Django oracle report\n\n");
    out.push_str(
        "| Project | Settings | Django | Libraries | Symbols | Probes | Templates | Wrong | Missed | Imprecise | Unchecked |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|---|---|---|---|\n");
    for report in reports {
        let c = &report.coverage;
        writeln!(
            out,
            "| {} | {:?} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            report.project,
            report.settings,
            report.django_version.as_deref().unwrap_or("?"),
            c.libraries,
            c.symbols,
            c.probes,
            c.templates,
            report.count(FindingClass::Wrong),
            report.count(FindingClass::Missed),
            report.count(FindingClass::Imprecise),
            report.count(FindingClass::Unchecked),
        )?;
    }
    for report in reports {
        writeln!(out, "\n## {}\n", report.scope())?;
        for note in &report.notes {
            writeln!(out, "- Note: {note}")?;
        }
        for class in [
            FindingClass::Wrong,
            FindingClass::Missed,
            FindingClass::Imprecise,
            FindingClass::Unchecked,
        ] {
            let findings: Vec<_> = report
                .findings
                .iter()
                .filter(|finding| finding.class == class)
                .collect();
            if findings.is_empty() {
                continue;
            }
            writeln!(out, "\n### {class:?} ({})\n", findings.len())?;
            for finding in findings {
                let subject = match (&finding.library, &finding.symbol) {
                    (Some(library), Some(symbol)) => format!("`{library}` `{symbol}`"),
                    (Some(library), None) => format!("`{library}`"),
                    (None, Some(symbol)) => format!("`{symbol}`"),
                    (None, None) => String::new(),
                };
                writeln!(
                    out,
                    "- **{:?}** {subject}: Django {}; DJLS {}.",
                    finding.family, finding.django, finding.djls
                )?;
                if !finding.note.is_empty() {
                    writeln!(out, "  - {}", finding.note)?;
                }
                if let Some(source) = &finding.source {
                    writeln!(out, "  - Source: `{source}`")?;
                }
                for example in &finding.evidence {
                    writeln!(out, "  - {example}")?;
                }
            }
        }
    }
    Ok(out)
}
