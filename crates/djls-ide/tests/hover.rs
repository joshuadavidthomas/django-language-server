use camino::Utf8Path;
use djls_ide::hover;
use djls_source::Offset;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;
use tower_lsp_server::ls_types;

fn hover_markdown(hover: ls_types::Hover) -> String {
    let ls_types::HoverContents::Markup(contents) = hover.contents else {
        panic!("template hover should use markup content");
    };
    contents.value
}

fn collision_fixture(source: &str) -> (TestDatabase, djls_source::File) {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['builtin_tags'], 'libraries': {'alpha': 'alpha_tags', 'beta': 'beta_tags'}}}]\n";
    let library_source = |doc: &str| {
        format!(
            "from django import template\nregister = template.Library()\n\n@register.simple_tag(name='shared')\ndef shared_tag():\n    \"\"\"{doc} tag.\"\"\"\n    return ''\n\n@register.filter(name='shared_filter')\ndef shared_filter(value):\n    \"\"\"{doc} filter.\"\"\"\n    return value\n"
        )
    };
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/builtin_tags.py",
            library_source("Builtin definition"),
        )
        .file(
            "/test/project/alpha_tags.py",
            library_source("Alpha definition"),
        )
        .file(
            "/test/project/beta_tags.py",
            library_source("Beta definition"),
        )
        .file("/test/project/templates/page.html", source)
        .install(&mut db);
    let file = db.file(Utf8Path::new("/test/project/templates/page.html"));
    (db, file)
}

#[test]
fn tag_hover_follows_definition_collisions_at_each_load_position() {
    let source = "{% shared %}{% load alpha %}{% shared %}{% load beta %}{% shared %}";
    let (db, file) = collision_fixture(source);
    let offsets = source
        .match_indices("shared")
        .map(|(offset, _)| Offset::new(u32::try_from(offset).unwrap()))
        .collect::<Vec<_>>();

    let before = hover_markdown(hover(&db, file, offsets[0]).expect("builtin tag hover"));
    let after_alpha = hover_markdown(hover(&db, file, offsets[1]).expect("alpha tag hover"));
    let after_beta = hover_markdown(hover(&db, file, offsets[2]).expect("beta tag hover"));

    assert!(before.contains("Defined in `builtin_tags`."), "{before}");
    assert!(
        after_alpha.contains("Defined in `alpha_tags`."),
        "{after_alpha}"
    );
    assert!(
        after_beta.contains("Defined in `beta_tags`."),
        "{after_beta}"
    );
}

#[test]
fn captured_if_else_does_not_hover_a_colliding_custom_definition() {
    let mut db = TestDatabase::new();
    let source = "{% load custom %}{% if condition %}{% else %}{% endif %}{% else %}";
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': 'custom_tags'}}}]\n",
        )
        .file(
            "/test/project/custom_tags.py",
            "from django import template\nregister = template.Library()\n\n@register.simple_tag(name='else')\ndef custom_else():\n    \"\"\"Custom else definition.\"\"\"\n    return ''\n",
        )
        .file("/test/project/templates/page.html", source)
        .install(&mut db);
    let file = db.file(Utf8Path::new("/test/project/templates/page.html"));
    let offsets = source
        .match_indices("else")
        .map(|(offset, _)| Offset::new(u32::try_from(offset).unwrap()))
        .collect::<Vec<_>>();

    assert_eq!(hover(&db, file, offsets[0]), None);

    let standalone = hover_markdown(
        hover(&db, file, offsets[1]).expect("standalone custom else definition hover"),
    );
    assert!(standalone.contains("(tag) else"), "{standalone}");
    assert!(
        standalone.contains("Defined in `custom_tags`."),
        "{standalone}"
    );
}

#[test]
fn filter_hover_follows_definition_collisions_at_each_load_position() {
    let source = "{{ value|shared_filter }}{% load alpha %}{{ value|shared_filter }}{% load beta %}{{ value|shared_filter }}";
    let (db, file) = collision_fixture(source);
    let offsets = source
        .match_indices("shared_filter")
        .map(|(offset, _)| Offset::new(u32::try_from(offset).unwrap()))
        .collect::<Vec<_>>();

    let before = hover_markdown(hover(&db, file, offsets[0]).expect("builtin filter hover"));
    let after_alpha = hover_markdown(hover(&db, file, offsets[1]).expect("alpha filter hover"));
    let after_beta = hover_markdown(hover(&db, file, offsets[2]).expect("beta filter hover"));

    assert!(before.contains("Defined in `builtin_tags`."), "{before}");
    assert!(
        after_alpha.contains("Defined in `alpha_tags`."),
        "{after_alpha}"
    );
    assert!(
        after_beta.contains("Defined in `beta_tags`."),
        "{after_beta}"
    );
}

#[test]
fn multi_backend_same_definition_hovers_across_builtin_and_loaded_exposure() {
    let mut db = TestDatabase::new();
    let settings = "INSTALLED_APPS = []\nif FLAG:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['shared_tags'], 'libraries': {'shared': 'empty_tags'}}}]\nelse:\n    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'shared_tags'}}}]\n";
    let source = "{% load shared %}\n{% common %}";
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file(
            "/test/project/shared_tags.py",
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef common(): pass\n",
        )
        .file(
            "/test/project/empty_tags.py",
            "from django import template\nregister = template.Library()\n",
        )
        .file("/test/project/shared/page.html", source)
        .install(&mut db);
    let file = db.file(Utf8Path::new("/test/project/shared/page.html"));
    let offset = Offset::new(u32::try_from(source.find("common").unwrap()).unwrap());

    let markdown = hover_markdown(
        hover(&db, file, offset).expect("the shared definition should have a consensus hover"),
    );

    assert!(markdown.contains("(tag) common"), "{markdown}");
    assert!(markdown.contains("Defined in `shared_tags`."), "{markdown}");
}

#[test]
fn selective_import_symbol_hover_uses_the_source_library() {
    let source = "{% load shared from alpha %}";
    let (db, file) = collision_fixture(source);
    let offset = Offset::new(u32::try_from(source.find("shared").unwrap()).unwrap());

    let markdown = hover_markdown(hover(&db, file, offset).expect("selective import hover"));

    assert!(markdown.contains("Defined in `alpha_tags`."), "{markdown}");
    assert!(!markdown.contains("Defined in `beta_tags`."), "{markdown}");
}

#[test]
fn template_hover_does_not_resolve_from_another_backend() {
    let mut db = TestDatabase::new();
    let source = r#"{% extends "base.html" %}"#;
    let settings = "TEMPLATES = [\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/a'], 'APP_DIRS': False},\n    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/b'], 'APP_DIRS': False},\n]\n";
    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file("/test/project/testproject/settings.py", settings)
        .file("/test/project/a/child.html", source)
        .file("/test/project/b/base.html", "other backend")
        .install(&mut db);
    let file = db.file(Utf8Path::new("/test/project/a/child.html"));

    let result = hover(
        &db,
        file,
        Offset::new(u32::try_from(source.find("base").unwrap()).unwrap()),
    )
    .expect("missing template hover should still explain the miss");
    let markdown = hover_markdown(result);
    assert!(markdown.contains("Template not found."));
    assert!(!markdown.contains("Resolved to"));
}

#[test]
fn template_hover_resolves_absolute_reference_from_originless_file() {
    let mut db = TestDatabase::new();
    let source = r#"{% extends "base.html" %}"#;

    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file("/test/project/scratch.html", source)
        .file("/test/project/templates/base.html", "base")
        .install(&mut db);
    let file = db.file(Utf8Path::new("/test/project/scratch.html"));

    let result = hover(
        &db,
        file,
        Offset::new(u32::try_from(source.find("base").unwrap()).unwrap()),
    )
    .expect("absolute reference should resolve from project inventory");
    let markdown = hover_markdown(result);

    assert!(
        markdown.contains("Resolved to `/test/project/templates/base.html`"),
        "{markdown}"
    );
}

#[test]
fn missing_template_hover_says_template_not_found() {
    let mut db = TestDatabase::new();
    let source = r#"{% extends "missing.html" %}"#;
    let child_path = "/test/project/templates/child.html";

    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/test/project/templates'], 'APP_DIRS': False}]\n",
        )
        .file(child_path, source)
        .install(&mut db);

    let file = db.file(Utf8Path::new(child_path));
    let result = hover(
        &db,
        file,
        Offset::new(u32::try_from(source.find("missing").unwrap()).unwrap()),
    )
    .expect("missing template with known search roots should have hover");
    let markdown = hover_markdown(result);

    assert!(markdown.contains("Template not found."));
    assert!(markdown.contains("`/test/project/templates/missing.html`"));
    assert!(!markdown.contains("search is incomplete"));
}

#[test]
fn inconclusive_template_hover_describes_incomplete_search_and_possible_matches() {
    let mut db = TestDatabase::new();
    let source = r#"{% extends "base.html" %}"#;
    let child_path = "/test/project/templates/child.html";

    ProjectFixture::new("/test/project")
        .django_settings_module("testproject.settings")
        .file(
            "/test/project/testproject/settings.py",
            "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [UNKNOWN, '/test/project/templates', '/test/project/app/templates'], 'APP_DIRS': False}]\n",
        )
        .file(child_path, source)
        .file("/test/project/templates/base.html", "first")
        .file("/test/project/app/templates/base.html", "second")
        .install(&mut db);

    let file = db.file(Utf8Path::new(child_path));
    let result = hover(
        &db,
        file,
        Offset::new(u32::try_from(source.find("base").unwrap()).unwrap()),
    )
    .expect("inconclusive template search should have hover");
    let markdown = hover_markdown(result);

    assert!(markdown.contains("Template search is incomplete."));
    assert!(markdown.contains("Possible matches:"));
    assert!(markdown.contains("`/test/project/templates/base.html`"));
    assert!(!markdown.contains("`/test/project/app/templates/base.html`"));
    assert!(!markdown.contains("Template not found."));
}
