use camino::Utf8PathBuf;
use djls_source::FileRootKind;
use djls_workspace::FilesForRootsRequest;
use djls_workspace::FilesForRootsResult;
use djls_workspace::WalkOptions;

use crate::django_environment_candidates;
use crate::project::Project;
use crate::settings::django_settings;
use crate::source_files::build_source_roots_plan;
use crate::source_files::source_files_update_from_partition_patches;
use crate::source_files::FileSetPartitionGroup;
use crate::source_files::ReadySourceFiles;
use crate::source_files::SourceFilePartitionPatch;
use crate::source_files::SourceFilesUpdate;
use crate::Db;
use crate::DjangoEnvironmentCandidatesOutcome;

#[must_use]
pub fn template_directory_file_roots_discovery(
    db: &dyn Db,
    project: Project,
) -> Option<Vec<Utf8PathBuf>> {
    let mut roots = Vec::new();
    let candidates = match django_environment_candidates(db, project) {
        DjangoEnvironmentCandidatesOutcome::Ready(candidates) => candidates,
        DjangoEnvironmentCandidatesOutcome::Deferred => return None,
    };

    for candidate in candidates {
        let settings = django_settings(db, project, candidate.id().clone());
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
    Some(roots)
}

pub(crate) fn template_directory_files_request(roots: Vec<Utf8PathBuf>) -> FilesForRootsRequest {
    let plan = build_source_roots_plan(roots, FileRootKind::Project);
    FilesForRootsRequest::new(
        plan.roots().to_vec(),
        Box::new(|path| {
            matches!(
                path.extension(),
                Some("html" | "htm" | "txt" | "jinja" | "jinja2")
            )
        }),
        WalkOptions {
            hidden: false,
            globs: vec!["!**/__pycache__/**".to_string()],
            no_ignore: false,
            follow_links: false,
            max_depth: None,
        },
    )
}

pub(crate) fn template_directory_source_files_update(
    current: Option<&ReadySourceFiles>,
    result: FilesForRootsResult,
) -> SourceFilesUpdate {
    source_files_update_from_partition_patches(
        current,
        FileSetPartitionGroup::ConfiguredTemplateDirectory,
        SourceFilePartitionPatch::configured_template_directory(result),
        Vec::new(),
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
            djls_workspace::load_files_for_roots(template_directory_files_request(vec![root]));
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
