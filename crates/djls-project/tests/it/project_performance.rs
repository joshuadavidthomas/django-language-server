use std::io;
use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::ScopedTemplateLibraries;
use djls_project::TemplateName;
use djls_project::TemplateResolutionResult;
use djls_project::TemplateSymbolKind;
use djls_project::template_library_candidate_files;
use djls_project::template_library_catalog;
use djls_project::template_resolution;
use djls_source::CaseSensitivity;
use djls_source::ChangeEvent;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use djls_source::RootWalk;
use djls_source::SourceChanges;
use djls_source::WalkOptions;
use djls_testing::OsTestDatabase;
use djls_testing::ProjectFixture;

#[derive(Default)]
struct CountedFileSystem {
    inner: Mutex<InMemoryFileSystem>,
    walks: Mutex<Vec<Utf8PathBuf>>,
    partial_root: Option<Utf8PathBuf>,
}

impl FileSystem for CountedFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        self.inner
            .lock()
            .map_err(|_error| io::Error::other("poisoned filesystem"))?
            .read_to_string(path)
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .exists(path)
    }

    fn is_file(&self, path: &Utf8Path) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_file(path)
    }

    fn is_dir(&self, path: &Utf8Path) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_dir(path)
    }

    fn case_sensitivity(&self) -> CaseSensitivity {
        CaseSensitivity::CaseSensitive
    }

    fn path_exists_case_sensitive(&self, path: &Utf8Path, prefix: &Utf8Path) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .path_exists_case_sensitive(path, prefix)
    }

    fn walk_root(&self, root: &Utf8Path, options: &WalkOptions) -> RootWalk {
        self.walks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(root.to_path_buf());
        let result = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .walk_root(root, options);
        match result {
            RootWalk::Directory {
                entries,
                mut issues,
            } if self.partial_root.as_deref() == Some(root) => {
                issues.push(io::ErrorKind::PermissionDenied);
                RootWalk::Directory { entries, issues }
            }
            result @ (RootWalk::Missing
            | RootWalk::File(_)
            | RootWalk::Directory { .. }
            | RootWalk::Inaccessible(_)) => result,
        }
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn candidate_membership_is_reused_until_membership_changes() {
    let fs = Arc::new(CountedFileSystem::default());
    {
        let mut inner = fs.inner.lock().expect("filesystem lock");
        for (path, source) in [
            (
                "/proj/settings.py",
                "INSTALLED_APPS = ['app']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'APP_DIRS': True}]\n",
            ),
            ("/proj/app/__init__.py", ""),
            ("/proj/app/templatetags/__init__.py", ""),
            (
                "/proj/app/templatetags/tags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef first(): pass\n",
            ),
        ] {
            inner.add_file(path.into(), source.to_string());
        }
    }
    let mut db =
        OsTestDatabase::with_file_system(Arc::clone(&fs) as Arc<dyn FileSystem>, ["/proj".into()]);
    let project = ProjectFixture::new("/proj")
        .without_bundled_django()
        .django_settings_module("settings")
        .install(&mut db)
        .expect("project fixture");
    let _ = template_library_catalog(&db, project);
    let candidates = template_library_candidate_files(&db, project).clone();
    assert!(
        candidates
            .iter()
            .any(|file| file.path(&db) == Utf8Path::new("/proj/app/templatetags/tags.py"))
    );
    let cold_walks = std::mem::take(&mut *fs.walks.lock().expect("walk log lock"));
    assert_eq!(
        cold_walks
            .iter()
            .filter(|path| path.as_str() == "/proj")
            .count(),
        1
    );
    assert_eq!(
        cold_walks
            .iter()
            .filter(|path| path.as_str() == "/proj/app/templatetags")
            .count(),
        1
    );

    // Priming uses synchronized membership, unlike imperative fresh discovery.
    assert_eq!(template_library_candidate_files(&db, project), &candidates);
    assert!(fs.walks.lock().expect("walk log lock").is_empty());

    // A registration edit changes catalog contents but not package membership.
    db.add_file("/proj/app/templatetags/tags.py", "from django import template\nregister = template.Library()\n@register.simple_tag\ndef second(): pass\n")
        .expect("edit registration");
    let catalog = template_library_catalog(&db, project);
    let library = ScopedTemplateLibraries::from_project_inventory(catalog)
        .loadable_library_str("tags")
        .found()
        .expect("installed library");
    assert!(library.symbol(TemplateSymbolKind::Tag, "first").is_none());
    assert!(library.symbol(TemplateSymbolKind::Tag, "second").is_some());
    assert_eq!(template_library_candidate_files(&db, project), &candidates);
    assert!(fs.walks.lock().expect("walk log lock").is_empty());

    // Empty candidates still need coverage: a later edit can introduce registrations.
    db.add_file("/proj/app/templatetags/added.py", "")
        .expect("new candidate");
    let _ = template_library_catalog(&db, project);
    let added = template_library_candidate_files(&db, project).clone();
    assert!(
        added
            .iter()
            .any(|file| file.path(&db) == Utf8Path::new("/proj/app/templatetags/added.py"))
    );
    let new_walks = std::mem::take(&mut *fs.walks.lock().expect("walk log lock"));
    assert_eq!(
        new_walks
            .iter()
            .filter(|path| path.as_str() == "/proj")
            .count(),
        1
    );
    assert_eq!(
        new_walks
            .iter()
            .filter(|path| path.as_str() == "/proj/app/templatetags")
            .count(),
        1
    );

    // Removing package identity must invalidate membership even with source overlays.
    fs.inner
        .lock()
        .expect("filesystem lock")
        .remove_file(Utf8Path::new("/proj/app/templatetags/__init__.py"));
    SourceChanges::new([ChangeEvent::Deleted(
        "/proj/app/templatetags/__init__.py".into(),
    )])
    .apply(&mut db);
    let _ = template_library_catalog(&db, project);
    assert!(template_library_candidate_files(&db, project).is_empty());
    db.add_file("/proj/app/templatetags/__init__.py", "")
        .expect("restore missing package identity");
    let _ = template_library_catalog(&db, project);
    assert_eq!(template_library_candidate_files(&db, project), &added);
}

#[test]
fn shared_template_roots_are_walked_once_without_losing_partial_evidence() {
    for partial in [false, true] {
        let fs = Arc::new(CountedFileSystem {
            partial_root: partial.then(|| "/proj/templates".into()),
            ..CountedFileSystem::default()
        });
        {
            let mut inner = fs.inner.lock().expect("filesystem lock");
            inner.add_file("/proj/settings.py".into(), "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates']}, {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/proj/templates']}]\n".to_string());
            inner.add_file("/proj/templates/base.html".into(), "base".to_string());
        }
        let mut db = OsTestDatabase::with_file_system(
            Arc::clone(&fs) as Arc<dyn FileSystem>,
            ["/proj".into()],
        );
        let project = ProjectFixture::new("/proj")
            .django_settings_module("settings")
            .install(&mut db)
            .expect("project fixture");
        let resolution = template_resolution(&db, project);
        let result = resolution.resolve(&db, TemplateName::new(&db, "base.html".to_string()));
        match result {
            TemplateResolutionResult::Found(origin) => {
                assert!(!partial);
                assert_eq!(
                    origin.path_buf(&db),
                    Utf8Path::new("/proj/templates/base.html")
                );
            }
            TemplateResolutionResult::Inconclusive(search) => {
                assert!(partial);
                assert_eq!(search.possible_origins.len(), 1);
                assert_eq!(
                    search.possible_origins[0].path_buf(&db),
                    Utf8Path::new("/proj/templates/base.html")
                );
            }
            TemplateResolutionResult::DoesNotExist(_) => panic!("known template missing"),
        }
        let walks = fs.walks.lock().expect("walk log lock");
        assert_eq!(
            walks
                .iter()
                .filter(|path| path.as_str() == "/proj/templates")
                .count(),
            1
        );
    }
}
