use divan::Bencher;
use djls_bench::realistic_db;
use djls_bench::template_fixtures;
use djls_bench::Db;
use djls_bench::TemplateFixture;
use djls_source::Diagnostic;
use djls_source::DiagnosticRenderer;
use djls_source::Severity;
use djls_source::Span;

fn main() {
    divan::main();
}

// Rendering engine benchmarks

fn span_of(source: &str, needle: &str) -> Span {
    let start = source.find(needle).unwrap_or(0);
    Span::saturating_from_bounds_usize(start, start + needle.len())
}

static SINGLE_SPAN_SOURCE: &str = "{% block content %}\n<p>Hello</p>\n{% endblock %}\n";

static MULTI_SPAN_SOURCE: &str = "{% block sidebar %}\n<nav>Links</nav>\n{% endblock content %}\n";

static MANY_ERRORS_SOURCE: &str = concat!(
    "{% if and x %}oops{% endif %}\n",
    "{{ name|title:\"arg\" }}\n",
    "{{ text|truncatewords }}\n",
    "{% trans \"hello\" %}\n",
    "{% unknown_tag %}\n",
    "{% for %}empty{% endfor %}\n",
    "{{ value|bogus }}\n",
    "{% block a %}{% endblock b %}\n",
);

#[divan::bench]
fn render_single_span(bencher: Bencher) {
    let renderer = DiagnosticRenderer::plain();
    let diag = Diagnostic::new(
        SINGLE_SPAN_SOURCE,
        "templates/page.html",
        "S100",
        "Unclosed tag: block",
        Severity::Error,
        span_of(SINGLE_SPAN_SOURCE, "{% block content %}"),
        "this block tag is never closed",
    );

    bencher.bench_local(move || {
        divan::black_box(renderer.render(&diag));
    });
}

#[divan::bench]
fn render_multi_span(bencher: Bencher) {
    let renderer = DiagnosticRenderer::plain();
    let diag = Diagnostic::new(
        MULTI_SPAN_SOURCE,
        "templates/layout.html",
        "S103",
        "'content' does not match 'sidebar'",
        Severity::Error,
        span_of(MULTI_SPAN_SOURCE, "{% endblock content %}"),
        "closing tag says 'content'",
    )
    .annotation(
        span_of(MULTI_SPAN_SOURCE, "{% block sidebar %}"),
        "opening tag is 'sidebar'",
        false,
    );

    bencher.bench_local(move || {
        divan::black_box(renderer.render(&diag));
    });
}

#[divan::bench]
fn render_styled_single_span(bencher: Bencher) {
    let renderer = DiagnosticRenderer::styled();
    let diag = Diagnostic::new(
        SINGLE_SPAN_SOURCE,
        "templates/page.html",
        "S100",
        "Unclosed tag: block",
        Severity::Error,
        span_of(SINGLE_SPAN_SOURCE, "{% block content %}"),
        "this block tag is never closed",
    );

    bencher.bench_local(move || {
        divan::black_box(renderer.render(&diag));
    });
}

// Collect diagnostics (minimal db — no specs)

#[divan::bench(args = template_fixtures())]
fn collect_diagnostics_minimal(bencher: Bencher, fixture: &TemplateFixture) {
    let mut db = Db::new();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);

    let nodelist = djls_templates::parse_template(&db, file);
    if let Some(nl) = nodelist {
        djls_semantic::validate_nodelist(&db, nl);
    }

    bencher.bench_local(move || {
        let nodelist = djls_templates::parse_template(&db, file);
        divan::black_box(djls_ide::collect_diagnostics(&db, file, nodelist));
    });
}

// Collect diagnostics (realistic db — real Django specs)

#[divan::bench(args = template_fixtures())]
fn collect_diagnostics_realistic(bencher: Bencher, fixture: &TemplateFixture) {
    let mut db = realistic_db();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);

    let nodelist = djls_templates::parse_template(&db, file);
    if let Some(nl) = nodelist {
        djls_semantic::validate_nodelist(&db, nl);
    }

    bencher.bench_local(move || {
        let nodelist = djls_templates::parse_template(&db, file);
        divan::black_box(djls_ide::collect_diagnostics(&db, file, nodelist));
    });
}

#[divan::bench]
fn collect_diagnostics_all_realistic(bencher: Bencher) {
    let fixtures = template_fixtures();
    let mut db = realistic_db();

    let files: Vec<_> = fixtures
        .iter()
        .map(|fixture| {
            let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
            let nodelist = djls_templates::parse_template(&db, file);
            if let Some(nl) = nodelist {
                djls_semantic::validate_nodelist(&db, nl);
            }
            file
        })
        .collect();

    bencher.bench_local(move || {
        for file in &files {
            let nodelist = djls_templates::parse_template(&db, *file);
            divan::black_box(djls_ide::collect_diagnostics(&db, *file, nodelist));
        }
    });
}

// Incremental diagnostics (realistic db — simulates editor edits)

#[divan::bench(args = template_fixtures())]
fn collect_diagnostics_incremental(bencher: Bencher, fixture: &TemplateFixture) {
    let mut db = realistic_db();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);

    let nodelist = djls_templates::parse_template(&db, file);
    if let Some(nl) = nodelist {
        djls_semantic::validate_nodelist(&db, nl);
        let _ = djls_ide::collect_diagnostics(&db, file, Some(nl));
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

        let nodelist = djls_templates::parse_template(&db, file);
        if let Some(nl) = nodelist {
            djls_semantic::validate_nodelist(&db, nl);
        }
        divan::black_box(djls_ide::collect_diagnostics(&db, file, nodelist));
    });
}

// Render validation errors from real templates (realistic db)

#[divan::bench(args = template_fixtures())]
fn render_validation_errors(bencher: Bencher, fixture: &TemplateFixture) {
    let mut db = realistic_db();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);

    let nodelist = djls_templates::parse_template(&db, file);
    if let Some(nl) = nodelist {
        djls_semantic::validate_nodelist(&db, nl);
    }

    let errors: Vec<_> = if let Some(nl) = nodelist {
        djls_semantic::validate_nodelist::accumulated::<djls_semantic::ValidationErrorAccumulator>(
            &db, nl,
        )
        .into_iter()
        .map(|acc| acc.0.clone())
        .collect()
    } else {
        Vec::new()
    };

    let config = djls_conf::DiagnosticsConfig::default();
    let renderer = DiagnosticRenderer::plain();

    bencher.bench_local(move || {
        for error in &errors {
            divan::black_box(djls_ide::render_validation_error(
                &fixture.source,
                fixture.path.as_str(),
                error,
                &config,
                &renderer,
            ));
        }
    });
}

// Render many synthetic errors (stress test with realistic specs)

#[divan::bench]
fn render_many_synthetic_errors(bencher: Bencher) {
    let mut db = realistic_db();
    let file = db.file_with_contents("bench.html".into(), MANY_ERRORS_SOURCE);

    let nodelist = djls_templates::parse_template(&db, file);
    if let Some(nl) = nodelist {
        djls_semantic::validate_nodelist(&db, nl);
    }

    let errors: Vec<_> = if let Some(nl) = nodelist {
        djls_semantic::validate_nodelist::accumulated::<djls_semantic::ValidationErrorAccumulator>(
            &db, nl,
        )
        .into_iter()
        .map(|acc| acc.0.clone())
        .collect()
    } else {
        Vec::new()
    };

    let config = djls_conf::DiagnosticsConfig::default();
    let renderer = DiagnosticRenderer::plain();

    bencher.bench_local(move || {
        for error in &errors {
            divan::black_box(djls_ide::render_validation_error(
                MANY_ERRORS_SOURCE,
                "bench.html",
                error,
                &config,
                &renderer,
            ));
        }
    });
}
