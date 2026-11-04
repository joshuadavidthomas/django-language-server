use std::sync::Arc;

use camino::Utf8Path;
use divan::Bencher;
use divan::black_box;
use djls_bench::primed_realistic_db;
use djls_bench::require;
use djls_bench::require_some;
use djls_ide::collect_diagnostics;
use djls_lsp::AnalysisHost;
use djls_lsp::Workspace;
use djls_source::InMemoryFileSystem;
use djls_source::LineCol;
use djls_source::LineIndex;
use djls_source::PositionEncoding;
use tower_lsp_server::ls_types;

fn main() {
    divan::main();
}

// This is the synchronous mutation kernel, not native transport latency. Every
// subsequent range is relative to the document after all preceding changes.
#[divan::bench(args = [1, 8, 32])]
fn apply_sequential_changes(bencher: Bencher, count: usize) {
    let line = "  <p>Café 😀 {{ value|default:\"—\" }}</p>\r\n";
    let source = line.repeat(512);
    let path = Utf8Path::new("/templates/changes.html");
    let uri: ls_types::Uri = "file:///templates/changes.html"
        .parse()
        .unwrap_or_else(|error| djls_bench::fail(error));
    let mut changes = vec![ls_types::TextDocumentContentChangeEvent {
        range: Some(ls_types::Range::new(
            ls_types::Position::new(128, 0),
            ls_types::Position::new(128, 0),
        )),
        range_length: None,
        text: "préface 😀\n".to_string(),
    }];
    for index in 1..count {
        let row =
            u32::try_from(129 + (index - 1) * 10).unwrap_or_else(|error| djls_bench::fail(error));
        changes.push(ls_types::TextDocumentContentChangeEvent {
            range: Some(ls_types::Range::new(
                ls_types::Position::new(row, 16),
                ls_types::Position::new(row, 21),
            )),
            range_length: None,
            text: "product".to_string(),
        });
    }
    let mut expected = String::new();
    for row in 0..512 {
        if row == 128 {
            expected.push_str("préface 😀\n");
        }
        if row >= 128 && (row - 128) % 10 == 0 && (row - 128) / 10 < count - 1 {
            expected.push_str(&line.replacen("value", "product", 1));
        } else {
            expected.push_str(line);
        }
    }

    let input = || {
        let mut host = AnalysisHost::new(
            Workspace::with_disk(Arc::new(InMemoryFileSystem::new())),
            None,
            &ls_types::InitializeParams::default(),
        );
        host.open_document(
            path,
            &ls_types::TextDocumentItem::new(
                uri.clone(),
                "htmldjango".to_string(),
                1,
                source.clone(),
            ),
            None,
        );
        (host, changes.clone())
    };
    let (mut probe, edits) = input();
    assert!(probe.update_document(path, 2, edits, None).is_some());
    let document = require_some("read changed buffer", probe.workspace().get_document(path));
    assert_eq!(document.content(), expected);
    assert_eq!(document.version(), 2);

    bencher
        .with_inputs(input)
        .bench_local_refs(|(host, edits)| {
            black_box(host.update_document(path, 2, std::mem::take(edits), None));
        });
}

const TEXT_KINDS: &[&str] = &["ascii", "unicode"];

fn position_input(kind: &str) -> (String, Vec<(LineCol, u32)>) {
    // Combining marks and astral characters distinguish byte, scalar and
    // UTF-16-unit counts. The ASCII control uses the same repetition counts.
    let (chunk, bytes, units) = match kind {
        "ascii" => ("Cafes item ", 11, 11),
        "unicode" => ("Café😀e\u{301} ", 13, 9),
        _ => djls_bench::fail("unknown position fixture"),
    };
    assert_eq!(chunk.len(), bytes);
    assert_eq!(chunk.encode_utf16().count(), units);
    let mut source = String::new();
    let mut positions = Vec::new();
    for row in 0..128 {
        let repeats = 8 + row % 57;
        let start = source.len();
        source.push_str(&chunk.repeat(repeats));
        for column in [1, repeats / 2, repeats] {
            positions.push((
                LineCol::new(
                    u32::try_from(row).unwrap_or_else(|error| djls_bench::fail(error)),
                    u32::try_from(column * units).unwrap_or_else(|error| djls_bench::fail(error)),
                ),
                u32::try_from(start + column * bytes)
                    .unwrap_or_else(|error| djls_bench::fail(error)),
            ));
        }
        source.push_str("\r\n");
    }
    (source, positions)
}

#[divan::bench(args = TEXT_KINDS)]
fn position_to_offset_utf16(bencher: Bencher, kind: &str) {
    let (source, positions) = position_input(kind);
    let index = LineIndex::from(source.as_str());
    for &(position, expected) in &positions {
        assert_eq!(
            index
                .offset(&source, position, PositionEncoding::Utf16)
                .get(),
            expected
        );
    }
    bencher.bench_local(|| {
        for &(position, _) in &positions {
            black_box(index.offset(
                black_box(&source),
                black_box(position),
                PositionEncoding::Utf16,
            ));
        }
    });
}

#[divan::bench(args = TEXT_KINDS)]
fn diagnostics_utf16(bencher: Bencher, kind: &str) {
    let (prefixes, _) = position_input(kind);
    let mut source = String::new();
    for line in prefixes.split_inclusive("\r\n") {
        source.push_str(line.trim_end());
        source.push_str("{% benchmark_unknown %}\r\n");
    }
    // Ensure both ends of a diagnostic cross a non-ASCII prefix, not merely a
    // non-ASCII file with all diagnostics at column zero.
    source.push_str("{{ value|benchmark_unknown }}");
    let mut db = require("prepare UTF-16 diagnostic database", primed_realistic_db());
    let file = require(
        "register UTF-16 diagnostic source",
        db.file_with_contents("/templates/positions.html", &source),
    );
    let diagnostics = require_some(
        "check UTF-16 diagnostic source",
        collect_diagnostics(&db, file, PositionEncoding::Utf16),
    );
    assert_eq!(diagnostics.len(), 129);
    for (row, diagnostic) in diagnostics[..128].iter().enumerate() {
        let units = if kind == "ascii" { 11 } else { 9 };
        let column = u32::try_from((8 + row % 57) * units - 1)
            .unwrap_or_else(|error| djls_bench::fail(error));
        assert_eq!(
            diagnostic.range,
            ls_types::Range::new(
                ls_types::Position::new(
                    u32::try_from(row).unwrap_or_else(|error| djls_bench::fail(error)),
                    column,
                ),
                ls_types::Position::new(
                    u32::try_from(row).unwrap_or_else(|error| djls_bench::fail(error)),
                    column + 23,
                ),
            ),
        );
        assert_eq!(
            diagnostic.code,
            Some(ls_types::NumberOrString::String("S108".to_string()))
        );
    }
    assert_eq!(
        diagnostics[128].range,
        ls_types::Range::new(
            ls_types::Position::new(128, 9),
            ls_types::Position::new(128, 26)
        )
    );
    assert_eq!(
        diagnostics[128].code,
        Some(ls_types::NumberOrString::String("S111".to_string()))
    );
    bencher.bench_local(|| black_box(collect_diagnostics(&db, file, PositionEncoding::Utf16)));
}
