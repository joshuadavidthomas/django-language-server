use divan::Bencher;
use djls_bench::realistic_db;
use djls_bench::template_fixtures;
use djls_bench::Db;
use djls_bench::TemplateFixture;

fn main() {
    divan::main();
}

#[divan::bench(args = template_fixtures())]
fn build_block_tree(bencher: Bencher, fixture: &TemplateFixture) {
    let mut db = Db::new();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);

    let _ = djls_templates::parse_template(&db, file).unwrap();

    bencher.bench_local(move || {
        if let Some(nodelist) = djls_templates::parse_template(&db, file) {
            let tree = djls_semantic::build_block_tree(&db, nodelist);
            divan::black_box(tree.roots(&db).len());
        }
    });
}

#[divan::bench(args = template_fixtures())]
fn build_semantic_forest(bencher: Bencher, fixture: &TemplateFixture) {
    let mut db = Db::new();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);

    if let Some(nodelist) = djls_templates::parse_template(&db, file) {
        let _ = djls_semantic::build_block_tree(&db, nodelist);
    }

    bencher.bench_local(move || {
        if let Some(nodelist) = djls_templates::parse_template(&db, file) {
            let tree = djls_semantic::build_block_tree(&db, nodelist);
            let forest = djls_semantic::build_semantic_forest(&db, tree, nodelist);
            divan::black_box(forest.roots(&db).len());
        }
    });
}

#[divan::bench(args = template_fixtures())]
fn validate_template(bencher: Bencher, fixture: &TemplateFixture) {
    let mut db = Db::new();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);

    let _ = djls_templates::parse_template(&db, file);

    bencher.bench_local(move || {
        if let Some(nodelist) = djls_templates::parse_template(&db, file) {
            djls_semantic::validate_nodelist(&db, nodelist);
        }
    });
}

#[divan::bench]
fn validate_all_templates(bencher: Bencher) {
    let fixtures = template_fixtures();
    let mut db = Db::new();

    let files: Vec<_> = fixtures
        .iter()
        .map(|fixture| {
            let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
            // Warm up the parse cache
            let _ = djls_templates::parse_template(&db, file);
            file
        })
        .collect();

    bencher.bench_local(move || {
        for file in &files {
            if let Some(nodelist) = djls_templates::parse_template(&db, *file) {
                djls_semantic::validate_nodelist(&db, nodelist);
            }
        }
    });
}

#[divan::bench(args = template_fixtures())]
fn validate_template_incremental_bench(bencher: Bencher, fixture: &TemplateFixture) {
    let mut db = Db::new();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);

    if let Some(nodelist) = djls_templates::parse_template(&db, file) {
        djls_semantic::validate_nodelist(&db, nodelist);
    }

    let original = fixture.source.clone();
    let modified = {
        let mut text = original.clone();
        text.push(' ');
        text
    };

    let mut revision = 1_u64;
    let mut use_modified = true;

    bencher.bench_local(move || {
        let contents = if use_modified { &modified } else { &original };
        use_modified = !use_modified;

        db.set_file_contents(file, contents, revision);
        revision = revision.wrapping_add(1);

        if let Some(nodelist) = djls_templates::parse_template(&db, file) {
            djls_semantic::validate_nodelist(&db, nodelist);
        }
    });
}

// Realistic validation: with real Django tag specs, scoping rules, and filter arities

#[divan::bench(args = template_fixtures())]
fn validate_template_realistic(bencher: Bencher, fixture: &TemplateFixture) {
    let mut db = realistic_db();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);

    let _ = djls_templates::parse_template(&db, file);

    bencher.bench_local(move || {
        if let Some(nodelist) = djls_templates::parse_template(&db, file) {
            djls_semantic::validate_nodelist(&db, nodelist);
        }
    });
}

#[divan::bench]
fn validate_all_templates_realistic(bencher: Bencher) {
    let fixtures = template_fixtures();
    let mut db = realistic_db();

    let files: Vec<_> = fixtures
        .iter()
        .map(|fixture| {
            let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
            let _ = djls_templates::parse_template(&db, file);
            file
        })
        .collect();

    bencher.bench_local(move || {
        for file in &files {
            if let Some(nodelist) = djls_templates::parse_template(&db, *file) {
                djls_semantic::validate_nodelist(&db, nodelist);
            }
        }
    });
}

#[divan::bench(args = template_fixtures())]
fn validate_template_realistic_incremental(bencher: Bencher, fixture: &TemplateFixture) {
    let mut db = realistic_db();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);

    if let Some(nodelist) = djls_templates::parse_template(&db, file) {
        djls_semantic::validate_nodelist(&db, nodelist);
    }

    let original = fixture.source.clone();
    let modified = {
        let mut text = original.clone();
        text.push(' ');
        text
    };

    let mut revision = 1_u64;
    let mut use_modified = true;

    bencher.bench_local(move || {
        let contents = if use_modified { &modified } else { &original };
        use_modified = !use_modified;

        db.set_file_contents(file, contents, revision);
        revision = revision.wrapping_add(1);

        if let Some(nodelist) = djls_templates::parse_template(&db, file) {
            djls_semantic::validate_nodelist(&db, nodelist);
        }
    });
}

// Opaque region computation

#[divan::bench(args = template_fixtures())]
fn compute_opaque_regions(bencher: Bencher, fixture: &TemplateFixture) {
    let mut db = realistic_db();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);

    let _ = djls_templates::parse_template(&db, file);

    bencher.bench_local(move || {
        if let Some(nodelist) = djls_templates::parse_template(&db, file) {
            divan::black_box(djls_semantic::compute_opaque_regions(&db, nodelist));
        }
    });
}
