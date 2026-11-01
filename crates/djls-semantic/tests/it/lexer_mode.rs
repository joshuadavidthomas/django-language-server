use camino::Utf8Path;
use djls_source::ChangeEvent;
use djls_source::SourceChanges;
use djls_templates::Token;
use djls_templates::lex_template;
use djls_testing::ProjectFixture;
use djls_testing::ProjectSettings;
use djls_testing::SalsaEventLog;
use djls_testing::TestDatabase;
use djls_testing::execution_count;
use salsa::Setter;

const APP_INIT: &str = "/proj/blog/__init__.py";
const PATCH: &str = "import re\nfrom django.template import base\nbase.tag_re = re.compile(base.tag_re.pattern, re.DOTALL)\n";

fn block_contents(db: &TestDatabase) -> Option<Vec<String>> {
    let file = db.file(Utf8Path::new("/proj/page.html")).ok()?;
    let tokens = lex_template(db, file).as_ref().ok()?;
    Some(
        tokens
            .iter()
            .filter_map(|token| {
                if let Token::Block { content, .. } = token {
                    Some(content.clone())
                } else {
                    None
                }
            })
            .collect(),
    )
}

fn edit(db: &mut TestDatabase, path: &str, source: &str) -> anyhow::Result<()> {
    db.add_file(path, source)?;
    SourceChanges::new([ChangeEvent::ContentChanged(path.into())]).apply(db);
    Ok(())
}

#[test]
fn template_lexing_follows_tag_re_patches_and_the_multiline_tags_setting() {
    let events = SalsaEventLog::default();
    let mut db = TestDatabase::with_event_log(events.clone());
    let project = ProjectFixture::new("/proj")
        .settings(&ProjectSettings {
            installed_apps: vec!["blog".to_string()],
            ..ProjectSettings::default()
        })
        .file(APP_INIT, "")
        .file("/proj/page.html", "{% if a\n %}{% endif %}")
        .install(&mut db)
        .expect("lexer mode fixture should install");

    assert_eq!(
        block_contents(&db).expect("template should lex"),
        [" endif "],
        "default tag_re"
    );

    edit(&mut db, APP_INIT, PATCH).expect("app source should update");
    events.take().expect("edit events should clear");
    assert_eq!(
        block_contents(&db).expect("template should lex"),
        [" if a\n ", " endif "],
        "DOTALL tag_re"
    );
    let names = events
        .take_will_execute_names(&db)
        .expect("relex events should be readable");
    assert_eq!(execution_count(&names, "lex_template"), 1);

    // The module scan backdates, so an edit that keeps the patch leaves lexing cached.
    edit(&mut db, APP_INIT, &format!("# patched\n{PATCH}")).expect("app source should update");
    events.take().expect("edit events should clear");
    assert_eq!(
        block_contents(&db).expect("template should lex"),
        [" if a\n ", " endif "]
    );
    let names = events
        .take_will_execute_names(&db)
        .expect("cached lexing events should be readable");
    assert_eq!(execution_count(&names, "startup_module_scan"), 1);
    assert_eq!(execution_count(&names, "lex_template"), 0);

    project.set_multiline_tags(&mut db).to(Some(false));
    assert_eq!(
        block_contents(&db).expect("template should lex"),
        [" endif "],
        "configured override"
    );
}
