use divan::Bencher;
use djls_bench::template_fixtures;
use djls_bench::Db;
use djls_bench::TemplateFixture;

fn main() {
    divan::main();
}

// Pure benchmarks without database overhead
#[divan::bench(args = template_fixtures())]
fn build_block_tree_pure(bencher: Bencher, fixture: &TemplateFixture) {
    // Parse once to get nodes
    let mut db = Db::new();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
    let nodelist = djls_templates::parse_template(&db, file).unwrap();
    let nodes = nodelist.nodelist(&db).to_vec();
    
    // Get specs (no db needed)
    let specs = djls_semantic::django_builtin_specs();
    
    bencher.bench_local(move || {
        let (tree_inner, _errors) = djls_semantic::build_block_tree_from_parts(&specs, &nodes);
        divan::black_box(tree_inner.roots.len());
    });
}

#[divan::bench(args = template_fixtures())]
fn build_semantic_forest_pure(bencher: Bencher, fixture: &TemplateFixture) {
    // Parse once to get nodes
    let mut db = Db::new();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
    let nodelist = djls_templates::parse_template(&db, file).unwrap();
    let nodes = nodelist.nodelist(&db).to_vec();
    
    // Build block tree once
    let specs = djls_semantic::django_builtin_specs();
    let (tree_inner, _) = djls_semantic::build_block_tree_from_parts(&specs, &nodes);
    
    bencher.bench_local(move || {
        let forest_inner = djls_semantic::build_forest_from_parts(tree_inner.clone(), &nodes);
        divan::black_box(forest_inner.roots.len());
    });
}

// Original benchmarks with database for comparison
#[divan::bench(args = template_fixtures())]
fn build_block_tree_with_db(bencher: Bencher, fixture: &TemplateFixture) {
    let mut db = Db::new();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);

    let _ = djls_templates::parse_template(&db, file).unwrap();

    bencher.bench_local(move || {
        if let Some(nodelist) = djls_templates::parse_template(&db, file) {
            // Use SemanticIndex instead of direct queries
            let index = djls_semantic::SemanticIndex::new(&db, nodelist);
            divan::black_box(index);
        }
    });
}

#[divan::bench(args = template_fixtures())]
fn build_semantic_forest_with_db(bencher: Bencher, fixture: &TemplateFixture) {
    let mut db = Db::new();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);

    // Warm up the cache
    if let Some(nodelist) = djls_templates::parse_template(&db, file) {
        let _ = djls_semantic::SemanticIndex::new(&db, nodelist);
    }

    bencher.bench_local(move || {
        if let Some(nodelist) = djls_templates::parse_template(&db, file) {
            let index = djls_semantic::SemanticIndex::new(&db, nodelist);
            divan::black_box(index);
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
