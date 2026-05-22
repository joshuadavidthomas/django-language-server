use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileRootKind;
use djls_workspace::FilesForRootsRequest;
use djls_workspace::FilesForRootsResult;
use djls_workspace::WalkOptions;

use crate::build_source_roots_with_kind;
use crate::django_environment_candidates;
use crate::effective_settings;
use crate::Db;
use crate::DjangoEnvironmentCandidatesOutcome;
use crate::PartitionedSourceFileLoadOutcome;
use crate::PartitionedSourceFilePatch;
use crate::Project;
use crate::ProjectSourceFilesIssue;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemplateDirectoryFilesLoadRequest {
    roots: Vec<Utf8PathBuf>,
}

impl TemplateDirectoryFilesLoadRequest {
    #[must_use]
    pub fn new(roots: Vec<Utf8PathBuf>) -> Self {
        Self { roots }
    }

    #[must_use]
    pub fn roots(&self) -> &[Utf8PathBuf] {
        &self.roots
    }
}

#[must_use]
pub fn template_directory_files_request(
    request: TemplateDirectoryFilesLoadRequest,
) -> FilesForRootsRequest {
    let plan = build_source_roots_with_kind(request.roots, FileRootKind::Project);
    FilesForRootsRequest::new(
        plan.roots().to_vec(),
        Box::new(template_file_predicate),
        template_directory_walk_options(),
    )
}

#[must_use]
pub fn load_template_directory_files(
    request: TemplateDirectoryFilesLoadRequest,
) -> FilesForRootsResult {
    djls_workspace::load_files_for_roots(template_directory_files_request(request))
}

#[must_use]
pub fn template_directory_file_roots(
    db: &dyn Db,
    project: Project,
) -> TemplateDirectoryFilesLoadRequest {
    let mut roots = Vec::new();
    let candidates = match django_environment_candidates(db, project) {
        DjangoEnvironmentCandidatesOutcome::Ready { candidates, .. }
        | DjangoEnvironmentCandidatesOutcome::Ambiguous { candidates, .. } => candidates,
        DjangoEnvironmentCandidatesOutcome::Unavailable { .. }
        | DjangoEnvironmentCandidatesOutcome::Deferred { .. } => {
            return TemplateDirectoryFilesLoadRequest::new(roots);
        }
    };

    for candidate in candidates {
        let settings = effective_settings(db, project, candidate.id().clone());
        for backend in settings.templates().backends() {
            for segment in backend.dirs().segments() {
                if let Some(dir) = segment.value() {
                    roots.push(Utf8PathBuf::from(dir));
                }
            }
        }
    }
    roots.sort();
    roots.dedup();
    TemplateDirectoryFilesLoadRequest::new(roots)
}

#[must_use]
pub fn template_directory_file_load_outcome(
    db: &dyn Db,
    project: Project,
) -> PartitionedSourceFileLoadOutcome {
    match django_environment_candidates(db, project) {
        DjangoEnvironmentCandidatesOutcome::Deferred { .. } => {
            return PartitionedSourceFileLoadOutcome::Deferred {
                issue: ProjectSourceFilesIssue::TemplateDirectoryGap,
            };
        }
        DjangoEnvironmentCandidatesOutcome::Unavailable { .. } => {
            return PartitionedSourceFileLoadOutcome::Unavailable {
                issue: ProjectSourceFilesIssue::TemplateDirectoryGap,
            };
        }
        DjangoEnvironmentCandidatesOutcome::Ready { .. }
        | DjangoEnvironmentCandidatesOutcome::Ambiguous { .. } => {}
    }
    let request = template_directory_file_roots(db, project);
    PartitionedSourceFileLoadOutcome::Ready(
        PartitionedSourceFilePatch::configured_template_directory(load_template_directory_files(
            request,
        )),
    )
}

fn template_directory_walk_options() -> WalkOptions {
    WalkOptions {
        hidden: false,
        globs: vec!["!**/__pycache__/**".to_string()],
        no_ignore: false,
        follow_links: false,
        max_depth: None,
    }
}

fn template_file_predicate(path: &Utf8Path) -> bool {
    matches!(
        path.extension(),
        Some("html" | "htm" | "txt" | "jinja" | "jinja2")
    )
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use djls_source::FileRootKind;

    use super::*;

    #[test]
    fn template_directory_files_loads_template_files_only() {
        let tempdir = tempfile::tempdir().expect("tempdir should be created");
        let root = utf8(tempdir.path()).join("templates");
        std::fs::create_dir_all(root.join("emails")).expect("template root should be created");
        std::fs::write(root.join("base.html"), "").expect("template should be written");
        std::fs::write(root.join("emails/welcome.txt"), "")
            .expect("text template should be written");
        std::fs::write(root.join("notes.py"), "").expect("python file should be written");

        let result =
            load_template_directory_files(TemplateDirectoryFilesLoadRequest::new(vec![root]));
        let loaded = result
            .files()
            .iter()
            .map(|file| file.path().file_name().unwrap().to_string())
            .collect::<Vec<_>>();

        assert!(loaded.contains(&"base.html".to_string()));
        assert!(loaded.contains(&"welcome.txt".to_string()));
        assert!(!loaded.contains(&"notes.py".to_string()));
        assert_eq!(result.roots()[0].kind(), FileRootKind::Project);
    }

    fn utf8(path: &std::path::Path) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(path.to_path_buf()).expect("path should be utf8")
    }
}
