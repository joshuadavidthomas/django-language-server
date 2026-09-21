use camino::Utf8Path;
use djls_conf::DjangoVersion;
use djls_project::bundled_django_version;
use djls_source::InMemoryFileSystem;

#[test]
fn bundled_django_selection_uses_project_metadata_before_oldest_lts() {
    let root = Utf8Path::new("/project");
    let mut fs = InMemoryFileSystem::new();
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django52)
    );
    fs.add_file(root.join("pyproject.toml"), "[project]\ndependencies = ['Django>=6.0,<7', 'django-stubs==5.2.0']\n[project.optional-dependencies]\ntest = ['Django==5.2.1']\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django60)
    );
    fs.add_file(
        root.join("uv.lock"),
        "[[package]]\nname = 'project'\nversion = '0.1.0'\nsource = { virtual = '.' }\ndependencies = [{ name = 'django' }]\n[[package]]\nname = 'django'\nversion = '6.1.1'\nsource = { registry = 'https://pypi.org/simple' }\n".into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django61)
    );
    assert_eq!(
        bundled_django_version(&fs, root, Some(DjangoVersion::Django52)),
        Some(DjangoVersion::Django52)
    );
    // A stale lock must not override the current project requirement.
    fs.add_file(
        root.join("uv.lock"),
        "[[package]]\nname = 'project'\nsource = { virtual = '.' }\ndependencies = [{ name = 'django' }]\n[[package]]\nname = 'django'\nversion = '5.2.3'\n".into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django60)
    );
    fs.remove_file(&root.join("uv.lock"));
    fs.add_file(
        root.join("poetry.lock"),
        "[[package]]\nname = 'Django'\nversion = '6.1.0'\ngroups = ['main']\noptional = false\n"
            .into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django61)
    );
}

#[test]
fn bundled_django_reads_pylock_and_checks_declared_constraints() {
    let root = Utf8Path::new("/project");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(
        root.join("pylock.toml"),
        "lock-version = '1.0'\ncreated-by = 'pip'\n\
         [[packages]]\nname = 'django-stubs'\nversion = '5.2.0'\n\
         [[packages]]\nname = 'django'\nversion = '6.1.1'\n"
            .into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django61)
    );
    fs.add_file(root.join("requirements.txt"), "Django>=6.0,<6.1\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django60),
        "a stale pylock must not override requirements.txt"
    );
    fs.remove_file(&root.join("requirements.txt"));
    fs.add_file(
        root.join("uv.lock"),
        "[[package]]\nname = 'project'\nsource = { virtual = '.' }\ndependencies = [{ name = 'django' }]\n[[package]]\nname = 'django'\nversion = '5.2.17'\n".into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django52),
        "tool-native lockfiles take precedence over pylock exports"
    );
    fs.remove_file(&root.join("uv.lock"));
    fs.add_file(
        root.join("pylock.toml"),
        "lock-version = '1.0'\ncreated-by = 'pip'\n\
         [[packages]]\nname = 'django'\nversion = '7.0.0'\n"
            .into(),
    );
    assert_eq!(bundled_django_version(&fs, root, None), None);
}

#[test]
fn bundled_django_reads_requirements_includes_constraints_and_exact_patches() {
    let root = Utf8Path::new("/project");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(
        root.join("requirements.txt"),
        "-r requirements/base.txt\n-c constraints.txt\n".into(),
    );
    fs.add_file(
        root.join("requirements/base.txt"),
        "--requirement=../requirements.txt\nDjango[argon2]>=6.0 # project dependency\n".into(),
    );
    fs.add_file(
        root.join("constraints.txt"),
        "Django==6.1.0 \\\n    --hash=sha256:example\n".into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django61)
    );
    fs.add_file(root.join("constraints.txt"), "Django==5.1.9\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        None,
        "known incompatible requirements must not silently select an LTS"
    );
}

#[test]
fn bundled_django_reads_pdm_and_pipenv_runtime_locks() {
    let root = Utf8Path::new("/project");
    for (name, content) in [
        (
            "pdm.lock",
            "[[package]]\nname = 'django'\nversion = '6.1.1'\ngroups = ['default']\n[[package]]\nname = 'django'\nversion = '5.2.17'\ngroups = ['test']\n",
        ),
        (
            "Pipfile.lock",
            r#"{"default":{"django":{"version":"==6.1.1"}},"develop":{"django":{"version":"==5.2.17"}}}"#,
        ),
    ] {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(root.join(name), content.into());
        assert_eq!(
            bundled_django_version(&fs, root, None),
            Some(DjangoVersion::Django61),
            "{name}"
        );
        fs.add_file(root.join("requirements.txt"), "Django>=6.0,<6.1\n".into());
        assert_eq!(
            bundled_django_version(&fs, root, None),
            Some(DjangoVersion::Django60),
            "stale {name}"
        );
    }
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(
        root.join("Pipfile.lock"),
        r#"{"develop":{"django":{"version":"==6.1.1"}}}"#.into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django52)
    );
    fs.add_file(root.join("Pipfile.lock"), "not JSON".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django52)
    );
    fs.add_file(
        root.join("pdm.lock"),
        "[[package]]\nname = 'django'\nversion = '6.0.8'\ngroups = ['default']\n".into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django60)
    );
}

#[test]
fn bundled_django_reads_legacy_poetry_constraints() {
    let root = Utf8Path::new("/project");
    for (declaration, expected) in [
        ("'^6.0'", Some(DjangoVersion::Django60)),
        ("'~6.1'", Some(DjangoVersion::Django61)),
        ("'6.1.*'", Some(DjangoVersion::Django61)),
        ("'6.0.3'", Some(DjangoVersion::Django60)),
        ("'^5.1'", Some(DjangoVersion::Django52)),
        ("'~5.1'", None),
        ("'^5.1 || ~6.1'", Some(DjangoVersion::Django52)),
        ("'>=6.0 <6.1'", Some(DjangoVersion::Django60)),
        ("'>= 6.0 < 6.1 || >= 7'", Some(DjangoVersion::Django60)),
        (
            "{version = '^6.1', python = '>=3.12 <3.13'}",
            Some(DjangoVersion::Django61),
        ),
        (
            "{version = '^6.1', extras = ['argon2']}",
            Some(DjangoVersion::Django61),
        ),
        (
            "{version = '^6.1', optional = true}",
            Some(DjangoVersion::Django52),
        ),
        (
            "{version = '^6.1', platform = 'linux', markers = \"sys_platform == 'win32'\"}",
            Some(DjangoVersion::Django52),
        ),
        (
            "[{version = '~6.0', python = '<3.12'}, {version = '^6.1', python = '>=3.12'}]",
            Some(DjangoVersion::Django60),
        ),
        (
            "[{version = '~6.0', markers = \"sys_platform == 'win32'\"}, {version = '^6.1', markers = \"sys_platform != 'win32'\"}]",
            Some(DjangoVersion::Django60),
        ),
    ] {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(root.join("pyproject.toml"), format!("[tool.poetry.dependencies]\nDjango = {declaration}\n[tool.poetry.group.test.dependencies]\nDjango = '5.2.*'\n"));
        assert_eq!(
            bundled_django_version(&fs, root, None),
            expected,
            "{declaration}"
        );
    }
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(root.join("pyproject.toml"), "[project]\ndependencies = ['Django>=6.0']\n[tool.poetry.dependencies]\ndjango = '^5.2 || ~6.1'\n".into());
    fs.add_file(
        root.join("poetry.lock"),
        "[[package]]\nname = 'django'\nversion = '6.0.8'\ngroups = ['main']\noptional = false\n"
            .into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django61),
        "intersect declarations without collapsing Poetry alternatives"
    );
}

#[test]
fn bundled_django_reads_setup_cfg_runtime_requirements_and_markers() {
    let root = Utf8Path::new("/project");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(root.join("setup.cfg"), "[metadata]\nname = example\n[options]\ninstall_requires =\n    django-stubs==5.2.0\n    Django>=6.0,<6.1; python_version < '3.12'\n    Django>=6.1; python_version >= '3.12' # newer Python\n[options.extras_require]\ntest = Django==5.2.1\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django60)
    );
    fs.add_file(root.join("requirements.txt"), "Django>=6.1\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django61)
    );
    fs.remove_file(&root.join("requirements.txt"));
    fs.add_file(
        root.join("setup.cfg"),
        "[options]\ninstall_requires = Django==6.1.0\n".into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django61)
    );
}

#[test]
fn bundled_django_handles_universal_locks_and_unusable_metadata() {
    let root = Utf8Path::new("/project");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(root.join("uv.lock"), "[[package]]\nname = 'project'\nsource = { virtual = '.' }\ndependencies = [{ name = 'django', version = '6.1.1', marker = \"sys_platform == 'win32'\" }, { name = 'django', version = '6.0.8', marker = \"sys_platform != 'win32'\" }]\n[[package]]\nname = 'django'\nversion = '6.1.1'\n[[package]]\nname = 'django'\nversion = '6.0.8'\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django60)
    );
    fs.add_file(
        root.join("uv.lock"),
        "[[package]]\nname = 'project'\nsource = { virtual = '.' }\ndependencies = [{ name = 'django' }]\n[[package]]\nname = 'django'\nversion = '5.1.9'\n".into(),
    );
    assert_eq!(bundled_django_version(&fs, root, None), None);
    fs.add_file(root.join("uv.lock"), "invalid toml [".into());
    fs.add_file(
        root.join("requirements.in"),
        "Django @ https://example.invalid/django.whl\n".into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django52)
    );
}

#[test]
fn bundled_django_constraints_do_not_create_runtime_requirements() {
    let root = Utf8Path::new("/project");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(
        root.join("requirements.txt"),
        "Django>=6.0; sys_platform == 'win32'\n-c constraints.txt\n".into(),
    );
    fs.add_file(root.join("constraints.txt"), "Django<6.0\n".into());
    assert_eq!(bundled_django_version(&fs, root, None), None);

    fs.add_file(root.join("requirements.txt"), "-c constraints.txt\n".into());
    fs.add_file(root.join("constraints.txt"), "Django>=7\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django52),
        "a constraints-only file must not establish a Django requirement"
    );
    fs.add_file(root.join("constraints.txt"), "Django>=6.0,<6.1\n".into());
    fs.add_file(root.join("uv.lock"), "[[package]]\nname = 'project'\nsource = { virtual = '.' }\ndependencies = [{name = 'django'}]\n[[package]]\nname = 'django'\nversion = '6.1.1'\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django60),
        "runtime lock evidence activates constraints even when the pinned version is stale"
    );
    fs.add_file(root.join("constraints.txt"), "Django>=7\n".into());
    assert_eq!(bundled_django_version(&fs, root, None), None);
}

#[test]
fn bundled_django_uv_uses_only_marker_feasible_runtime_reachability() {
    let root = Utf8Path::new("/project");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(
        root.join("requirements.txt"),
        "Django>=5.2; sys_platform == 'win32'\nDjango>=6.1; sys_platform != 'win32'\n".into(),
    );
    fs.add_file(root.join("uv.lock"), "[[package]]\nname = 'project'\nsource = { virtual = '.' }\ndependencies = [{ name = 'middle' }]\n[package.dev-dependencies]\ndev = [{ name = 'django', version = '5.2.1' }]\n[package.optional-dependencies]\nfeature = [{ name = 'django', version = '5.2.1' }]\n[[package]]\nname = 'middle'\nversion = '1.0'\ndependencies = [{ name = 'project' }, { name = 'django', version = '6.1.1', marker = \"sys_platform != 'win32'\" }, { name = 'django', version = '6.0.8', marker = \"sys_platform == 'win32'\" }]\n[[package]]\nname = 'django'\nversion = '5.2.1'\n[[package]]\nname = 'django'\nversion = '6.0.8'\n[[package]]\nname = 'django'\nversion = '6.1.1'\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django60),
        "transitive runtime edges are followed, cycles terminate, and dev/optional edges stay disabled"
    );
}

#[test]
fn bundled_django_ignores_non_runtime_poetry_lock_entries() {
    let root = Utf8Path::new("/project");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(root.join("poetry.lock"), "[[package]]\nname = 'django'\nversion = '6.1.1'\ngroups = ['dev']\noptional = false\n[[package]]\nname = 'django'\nversion = '6.0.8'\ngroups = ['main']\noptional = true\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django52)
    );
}

#[test]
fn bundled_django_does_not_invent_runtime_lock_applicability() {
    let root = Utf8Path::new("/project");
    for (filename, source) in [
        (
            "Pipfile.lock",
            r#"{"default":{"django":{"version":"==6.1.1","markers":"sys_platform == 'linux' and sys_platform == 'win32'"}}}"#,
        ),
        (
            "poetry.lock",
            "[[package]]\nname = 'django'\nversion = '6.1.1'\ngroups = ['main']\nmarkers = \"extra == 'web'\"\n",
        ),
        (
            "pylock.toml",
            "[[packages]]\nname = 'django'\nversion = '6.1.1'\nmarker = \"'web' in extras\"\n",
        ),
        (
            "pdm.lock",
            "[[package]]\nname = 'django'\nversion = '6.1.1'\n[metadata]\ngroups = ['default', 'dev']\n",
        ),
        (
            "uv.lock",
            "[[package]]\nname = 'project'\nsource = { virtual = '.' }\n[package.dev-dependencies]\ndev = [{name = 'helper'}]\n[[package]]\nname = 'helper'\nsource = { editable = 'helper' }\ndependencies = [{name = 'django'}]\n[[package]]\nname = 'django'\nversion = '6.1.1'\n",
        ),
    ] {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(root.join(filename), source.into());
        assert_eq!(
            bundled_django_version(&fs, root, None),
            Some(DjangoVersion::Django52),
            "{filename}: {source}"
        );
    }
}

#[test]
fn bundled_django_constraint_include_modes_follow_each_directive() {
    let root = Utf8Path::new("/project");
    let mut fs = InMemoryFileSystem::new();
    fs.add_file(root.join("requirements.txt"), "-c constraints.txt\n".into());
    fs.add_file(root.join("constraints.txt"), "-r runtime.txt\n".into());
    fs.add_file(root.join("runtime.txt"), "Django>=6.1\n".into());
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django61)
    );
    fs.add_file(
        root.join("requirements.txt"),
        "-r runtime.txt\n-c runtime.txt\n".into(),
    );
    assert_eq!(
        bundled_django_version(&fs, root, None),
        Some(DjangoVersion::Django61)
    );
}

#[test]
fn bundled_django_lock_markers_must_match_the_declaration_environment() {
    let root = Utf8Path::new("/project");
    for (filename, source) in [
        (
            "poetry.lock",
            "[[package]]\nname = 'django'\nversion = '6.0.8'\ngroups = ['main','dev']\nmarkers = {main = \"sys_platform == 'linux'\", dev = \"sys_platform == 'win32'\"}\n[[package]]\nname = 'django'\nversion = '6.1.1'\ngroups = ['main']\n",
        ),
        (
            "uv.lock",
            "[[package]]\nname = 'project'\nsource = { virtual = '.' }\ndependencies = [{name = 'django', version = '6.0.8', marker = \"sys_platform == 'linux'\"}, {name = 'django', version = '6.1.1'}]\n[[package]]\nname = 'django'\nversion = '6.0.8'\n[[package]]\nname = 'django'\nversion = '6.1.1'\n",
        ),
    ] {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(
            root.join("requirements.txt"),
            "Django>=6.0; sys_platform == 'win32'\nDjango>=6.1; sys_platform != 'win32'\n".into(),
        );
        fs.add_file(root.join(filename), source.into());
        assert_eq!(
            bundled_django_version(&fs, root, None),
            Some(DjangoVersion::Django61),
            "{filename}"
        );
    }
}

#[test]
fn bundled_django_uv_only_activates_explicit_dependency_extras() {
    let root = Utf8Path::new("/project");
    for (extra, expected) in [
        ("", DjangoVersion::Django52),
        (", extra = ['web']", DjangoVersion::Django61),
    ] {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(root.join("uv.lock"), format!("[[package]]\nname = 'project'\nsource = {{ virtual = '.' }}\ndependencies = [{{name = 'helper'{extra}}}]\n[[package]]\nname = 'helper'\nversion = '1.0'\n[package.optional-dependencies]\nweb = [{{name = 'django'}}]\n[[package]]\nname = 'django'\nversion = '6.1.1'\n"));
        assert_eq!(bundled_django_version(&fs, root, None), Some(expected));
    }
}

#[test]
fn bundled_django_url_requirements_activate_constraints() {
    let root = Utf8Path::new("/project");
    for (constraint, expected) in [
        ("", Some(DjangoVersion::Django52)),
        ("Django>=6.1,<6.2", Some(DjangoVersion::Django61)),
        ("Django>=7", None),
    ] {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(
            root.join("requirements.txt"),
            "Django @ https://example.org/django.whl\n-c constraints.txt\n".into(),
        );
        fs.add_file(root.join("constraints.txt"), constraint.into());
        assert_eq!(
            bundled_django_version(&fs, root, None),
            expected,
            "{constraint}"
        );
    }
}

#[test]
fn bundled_django_declaration_extras_are_not_selected() {
    let root = Utf8Path::new("/project");
    for (marker, expected) in [
        ("extra == 'web'", DjangoVersion::Django52),
        ("extra != 'web'", DjangoVersion::Django61),
        (
            "extra == 'web' and sys_platform == 'win32'",
            DjangoVersion::Django52,
        ),
        (
            "extra == 'web' or sys_platform == 'win32'",
            DjangoVersion::Django61,
        ),
    ] {
        for poetry in [false, true] {
            let mut fs = InMemoryFileSystem::new();
            if poetry {
                fs.add_file(root.join("pyproject.toml"), format!("[tool.poetry.dependencies]\nDjango = {{version = '>=6.1', markers = \"{marker}\"}}\n"));
            } else {
                fs.add_file(
                    root.join("requirements.txt"),
                    format!("Django>=6.1; {marker}\n"),
                );
            }
            assert_eq!(
                bundled_django_version(&fs, root, None),
                Some(expected),
                "{marker}, poetry={poetry}"
            );
        }
    }
}

#[test]
fn bundled_django_respects_project_python_bounds() {
    let root = Utf8Path::new("/project");
    for (bound, expected) in [
        (">=3.12", DjangoVersion::Django61),
        (">=3.10", DjangoVersion::Django52),
    ] {
        for style in ["pep621", "poetry", "setup"] {
            let mut fs = InMemoryFileSystem::new();
            match style {
                "pep621" => fs.add_file(root.join("pyproject.toml"), format!("[project]\nrequires-python = '{bound}'\ndependencies = [\"Django>=5.2,<6; python_version < '3.12'\", \"Django>=6.1; python_version >= '3.12'\"]\n")),
                "poetry" => fs.add_file(root.join("pyproject.toml"), format!("[tool.poetry.dependencies]\npython = '{bound}'\nDjango = [{{version = '>=5.2,<6', python = '<3.12'}}, {{version = '>=6.1', python = '>=3.12'}}]\n")),
                _ => fs.add_file(root.join("setup.cfg"), format!("[options]\npython_requires = {bound}\ninstall_requires =\n    Django>=5.2,<6; python_version < '3.12'\n    Django>=6.1; python_version >= '3.12'\n")),
            }
            assert_eq!(
                bundled_django_version(&fs, root, None),
                Some(expected),
                "{style}, {bound}"
            );
            fs.add_file(root.join("pylock.toml"), "[[packages]]\nname = 'django'\nversion = '5.2.17'\nmarker = \"python_version < '3.12'\"\n".into());
            assert_eq!(
                bundled_django_version(&fs, root, None),
                Some(expected),
                "lock: {style}, {bound}"
            );
        }
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(
            root.join("pyproject.toml"),
            format!("[project]\nrequires-python = '{bound}'\n"),
        );
        fs.add_file(root.join("pylock.toml"), "[[packages]]\nname = 'django'\nversion = '5.2.17'\nmarker = \"python_version < '3.12'\"\n[[packages]]\nname = 'django'\nversion = '6.1.1'\nmarker = \"python_version >= '3.12'\"\n".into());
        assert_eq!(
            bundled_django_version(&fs, root, None),
            Some(expected),
            "lock-only: {bound}"
        );
    }
}

#[test]
fn bundled_django_lock_scope_must_overlap_project_scope() {
    let root = Utf8Path::new("/project");
    for (filename, source, expected) in [
        (
            "pylock.toml",
            "requires-python = '>=3.12'\n[[packages]]\nname = 'django'\nversion = '6.1.1'\n",
            DjangoVersion::Django52,
        ),
        (
            "pylock.toml",
            "[[packages]]\nname = 'django'\nversion = '6.1.1'\nrequires-python = '>=3.12'\n",
            DjangoVersion::Django52,
        ),
        (
            "pylock.toml",
            "requires-python = '>=3.10'\n[[packages]]\nname = 'django'\nversion = '6.1.1'\nrequires-python = '<3.12'\n",
            DjangoVersion::Django61,
        ),
        (
            "pylock.toml",
            "environments = [\"sys_platform == 'win32'\"]\n[[packages]]\nname = 'django'\nversion = '6.1.1'\n",
            DjangoVersion::Django52,
        ),
        (
            "pylock.toml",
            "environments = [\"sys_platform == 'win32'\", \"sys_platform == 'linux'\"]\n[[packages]]\nname = 'django'\nversion = '6.1.1'\n",
            DjangoVersion::Django61,
        ),
        (
            "poetry.lock",
            "[metadata]\npython-versions = '>=3.12'\n[[package]]\nname = 'django'\nversion = '6.1.1'\ngroups = ['main']\n",
            DjangoVersion::Django52,
        ),
        (
            "poetry.lock",
            "[[package]]\nname = 'django'\nversion = '6.1.1'\ngroups = ['main']\npython-versions = '>=3.12'\n",
            DjangoVersion::Django52,
        ),
        (
            "uv.lock",
            "requires-python = '>=3.12'\n[[package]]\nname = 'project'\nsource = {virtual = '.'}\ndependencies = [{name = 'django'}]\n[[package]]\nname = 'django'\nversion = '6.1.1'\n",
            DjangoVersion::Django52,
        ),
    ] {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(root.join("pyproject.toml"), "[project]\nrequires-python = '==3.11.*'\ndependencies = [\"Django>=5.2; sys_platform == 'linux'\"]\n".into());
        fs.add_file(root.join(filename), source.into());
        assert_eq!(
            bundled_django_version(&fs, root, None),
            Some(expected),
            "{filename}: {source}"
        );
    }
}

#[test]
fn bundled_django_direct_sources_preserve_runtime_evidence() {
    let root = Utf8Path::new("/project");
    for (filename, source) in [
        (
            "pyproject.toml",
            r#"[tool.poetry.dependencies]
django = { git = "https://example.org/django.git", markers = "{marker}" }
"#,
        ),
        (
            "pyproject.toml",
            r#"[tool.poetry.dependencies]
django = { url = "https://example.org/django.tar.gz", markers = "{marker}" }
"#,
        ),
        (
            "pyproject.toml",
            r#"[tool.poetry.dependencies]
django = { path = "../django", markers = "{marker}" }
"#,
        ),
        (
            "pyproject.toml",
            r#"[tool.poetry.dependencies]
django = { file = "../django.tar.gz", markers = "{marker}" }
"#,
        ),
        (
            "pylock.toml",
            r#"[[packages]]
name = "django"
directory = { path = "../django" }
marker = "{marker}"
"#,
        ),
        (
            "pylock.toml",
            r#"[[packages]]
name = "django"
vcs = { type = "git", url = "https://example.org/django.git", commit-id = "abc" }
marker = "{marker}"
"#,
        ),
        (
            "pylock.toml",
            r#"[[packages]]
name = "django"
archive = { path = "../django.tar.gz" }
marker = "{marker}"
"#,
        ),
    ] {
        for (constraint, active_expected) in [
            ("Django>=6.1,<6.2", Some(DjangoVersion::Django61)),
            ("Django>=7", None),
        ] {
            for active in [true, false] {
                let marker = if active {
                    "extra != 'web'"
                } else {
                    "extra == 'web'"
                };
                let metadata = source.replace("{marker}", marker);
                let mut fs = InMemoryFileSystem::new();
                fs.add_file(root.join(filename), metadata.clone());
                fs.add_file(root.join("requirements.txt"), "-c constraints.txt\n".into());
                fs.add_file(root.join("constraints.txt"), constraint.into());
                assert_eq!(
                    bundled_django_version(&fs, root, None),
                    if active {
                        active_expected
                    } else {
                        Some(DjangoVersion::Django52)
                    },
                    "{metadata}, {constraint}"
                );
            }
        }
    }
}
