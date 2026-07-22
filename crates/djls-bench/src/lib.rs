mod check;
mod db;
mod fixtures;
mod specs;

use std::env;
use std::fmt;
use std::process;

pub use check::MANY_ERRORS_SOURCE;
pub use check::SyntheticDiagnosticError;
pub use check::synthetic_render_diagnostics;
pub use db::Db;
pub use fixtures::CorpusLoadError;
pub use fixtures::CorpusTemplates;
pub use fixtures::Fixture;
pub use fixtures::FixtureLoadError;
pub use fixtures::ValidationErrorFixture;
pub use fixtures::django_corpus_templates;
pub use fixtures::full_corpus_templates;
pub use fixtures::model_benchmark_module_name;
pub use fixtures::model_fixtures;
pub use fixtures::python_fixtures;
pub use fixtures::template_fixtures;
pub use fixtures::validation_error_fixtures;
pub use specs::BenchmarkSetupError;
pub use specs::primed_realistic_db;
pub use specs::realistic_db;
pub use specs::structure_db;

pub const BATCH_INNER_ITERS: usize = 8;
pub const REPEATED_INNER_ITERS: usize = 100;
pub const DIAGNOSTICS_INNER_ITERS: usize = BATCH_INNER_ITERS;
pub const DIAGNOSTICS_WARMUP_ITERS: usize = 3;

/// Print a benchmark failure with its operation context, then stop the process.
pub fn fail(context: impl fmt::Display) -> ! {
    eprintln!("benchmark failed: {context}");
    process::exit(1);
}

/// Extract a successful benchmark setup result or stop before recording timings.
pub fn require<T, E>(context: impl fmt::Display, result: Result<T, E>) -> T
where
    E: fmt::Display,
{
    match result {
        Ok(value) => value,
        Err(error) => fail(format_args!("{context}: {error}")),
    }
}

/// Extract a required benchmark setup value or stop before recording timings.
pub fn require_some<T>(context: impl fmt::Display, value: Option<T>) -> T {
    match value {
        Some(value) => value,
        None => fail(context),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CorpusRequirement {
    Optional,
    Required,
}

impl CorpusRequirement {
    fn from_environment() -> Self {
        if env::var_os("DJLS_REQUIRE_BENCH_CORPUS").is_some() {
            Self::Required
        } else {
            Self::Optional
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum CorpusRequirementError<E> {
    #[error("required benchmark corpus is not synchronized; run `just corpus sync`")]
    Missing,
    #[error("{0}; run `just corpus sync`")]
    Invalid(E),
}

fn apply_corpus_requirement<T, E>(
    requirement: CorpusRequirement,
    corpus: Result<Option<T>, E>,
) -> Result<Option<T>, CorpusRequirementError<E>> {
    match corpus {
        Ok(Some(corpus)) => Ok(Some(corpus)),
        Ok(None) if requirement == CorpusRequirement::Required => {
            Err(CorpusRequirementError::Missing)
        }
        Ok(None) => Ok(None),
        Err(error) => Err(CorpusRequirementError::Invalid(error)),
    }
}

/// Load a corpus benchmark input, skipping only when an optional corpus is absent.
pub fn corpus_or_skip<T, E>(context: impl fmt::Display, corpus: Result<Option<T>, E>) -> Option<T>
where
    E: fmt::Display,
{
    match apply_corpus_requirement(CorpusRequirement::from_environment(), corpus) {
        Ok(Some(corpus)) => Some(corpus),
        Ok(None) => {
            eprintln!("{context} corpus is not synchronized; skipping");
            None
        }
        Err(error) => fail(format_args!("load {context} corpus: {error}")),
    }
}

/// Whether benchmark and snapshot jobs require synchronized corpus data.
#[cfg(test)]
#[must_use]
pub(crate) fn bench_corpus_is_required() -> bool {
    CorpusRequirement::from_environment() == CorpusRequirement::Required
}

pub fn prime<F: FnMut()>(times: usize, mut f: F) {
    for _ in 0..times {
        f();
    }
}

#[cfg(test)]
mod tests {
    use super::CorpusRequirement;
    use super::CorpusRequirementError;
    use super::apply_corpus_requirement;

    #[test]
    fn optional_missing_corpus_is_skipped() {
        let corpus = apply_corpus_requirement(
            CorpusRequirement::Optional,
            Result::<Option<()>, &str>::Ok(None),
        )
        .expect("an absent optional corpus should not fail");

        assert!(corpus.is_none());
    }

    #[test]
    fn required_missing_corpus_is_an_error() {
        let error = apply_corpus_requirement(
            CorpusRequirement::Required,
            Result::<Option<()>, &str>::Ok(None),
        )
        .expect_err("an absent required corpus should fail");

        assert_eq!(
            error.to_string(),
            "required benchmark corpus is not synchronized; run `just corpus sync`"
        );
        assert!(matches!(error, CorpusRequirementError::Missing));
    }

    #[test]
    fn required_invalid_corpus_is_an_error() {
        let error = apply_corpus_requirement(
            CorpusRequirement::Required,
            Result::<Option<()>, _>::Err("lockfile does not match synchronized data"),
        )
        .expect_err("an invalid required corpus should fail");

        assert_eq!(
            error.to_string(),
            "lockfile does not match synchronized data; run `just corpus sync`"
        );
        assert!(matches!(
            error,
            CorpusRequirementError::Invalid("lockfile does not match synchronized data")
        ));
    }
}
