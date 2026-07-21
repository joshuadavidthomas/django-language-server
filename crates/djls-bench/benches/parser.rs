use divan::Bencher;
use djls_bench::BATCH_INNER_ITERS;
use djls_bench::Db;
use djls_bench::fail;
use djls_bench::require;
use djls_bench::template_fixtures;
use djls_templates::TemplateParseResult;

fn main() {
    divan::main();
}

#[divan::bench]
fn all(bencher: Bencher) {
    let fixtures = require("load template benchmark fixtures", template_fixtures());
    bencher.bench_local(move || {
        let mut total_nodes = 0;
        for fixture in fixtures {
            let (nodes, _errors) = djls_templates::parse_template_impl(&fixture.source);
            total_nodes += nodes.len();
        }
        divan::black_box(total_nodes);
    });
}

struct IncrementalTemplate {
    file: djls_source::File,
    original: String,
    modified: String,
    use_modified: bool,
}

#[divan::bench]
fn incremental(bencher: Bencher) {
    let fixtures = require("load template benchmark fixtures", template_fixtures());
    let mut db = Db::new();

    let mut templates = Vec::with_capacity(fixtures.len());
    for fixture in fixtures {
        let file = require(
            format_args!("register incremental parser fixture {}", fixture.path),
            db.file_with_contents(fixture.path.clone(), &fixture.source),
        );
        match djls_templates::parse_template(&db, file) {
            TemplateParseResult::Parsed(nodelist) => divan::black_box_drop(nodelist),
            TemplateParseResult::NotTemplate => fail(format_args!(
                "incremental parser fixture {} is not a template",
                fixture.path
            )),
            TemplateParseResult::Unreadable(error) => fail(format_args!(
                "prime incremental parser fixture {}: {error}",
                fixture.path
            )),
        }

        let original = fixture.source.clone();
        let modified = {
            let mut text = original.clone();
            text.push(' ');
            text
        };

        templates.push(IncrementalTemplate {
            file,
            original,
            modified,
            use_modified: true,
        });
    }

    bencher.bench_local(move || {
        let mut total_nodes = 0;
        for _ in 0..BATCH_INNER_ITERS {
            for template in &mut templates {
                let contents = if template.use_modified {
                    &template.modified
                } else {
                    &template.original
                };
                template.use_modified = !template.use_modified;

                db.set_file_contents(template.file, contents);

                match djls_templates::parse_template(&db, template.file) {
                    TemplateParseResult::Parsed(nodelist) => {
                        total_nodes += nodelist.nodelist(&db).len();
                    }
                    TemplateParseResult::NotTemplate => {
                        fail("incremental parser file stopped being a template");
                    }
                    TemplateParseResult::Unreadable(error) => {
                        fail(format_args!("parse incremental template: {error}"));
                    }
                }
            }
        }
        divan::black_box(total_nodes);
    });
}
