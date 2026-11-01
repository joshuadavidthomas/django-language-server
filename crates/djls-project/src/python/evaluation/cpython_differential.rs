// The static Python evaluator must agree with CPython.
//
// Generated settings-style modules run under CPython once per environment, and
// the evaluator analyzes the same source once. The evaluator over-approximates:
// each name holds a set of alternatives, and `Unknown` stands for any value. It
// is sound when every value CPython produced, under every environment, is
// admitted by one of the evaluator's alternatives for that name.
//
// Environment reads (`os.environ.get`, `os.getenv`) are the opaque inputs, so
// branches that depend on them exercise path joins and correlation.

use std::fmt::Write as _;
use std::io::Write as _;
use std::process::Command;
use std::process::Stdio;

use camino::Utf8Path;
use djls_conf::Settings;
use djls_source::Db as _;
use djls_testing::TestDatabase;
use proptest::prelude::*;
use proptest::strategy::Union;
use salsa::Setter as _;
use serde_json::Value as Json;

use super::EvaluationDemand;
use super::PythonBindingState;
use super::PythonModuleFacts;
use super::PythonValue;
use super::PythonValueKind;
use super::python_module_facts;
use super::sequence::PythonMaterializedSequence;
use super::sequence::PythonSequenceAlternativeRef;
use super::sequence::PythonSequenceItem;

const STRS: &[&str] = &["S0", "S1", "S2"];
const BOOLS: &[&str] = &["B0", "B1"];
const LISTS: &[&str] = &["L0", "L1"];
const DICTS: &[&str] = &["D0", "D1"];
const PATHS: &[&str] = &["P0"];
const KEYS: &[&str] = &["'k'", "'m'"];
const LITERALS: &[&str] = &["'a'", "'b'", "''", "'/srv'"];

const PRELUDE: &str = "import os\nfrom pathlib import Path\nS0 = 'a'\nS1 = 'b'\nS2 = ''\nB0 = False\nB1 = True\nL0 = []\nL1 = ['a']\nD0 = {}\nD1 = {'k': 'a'}\nP0 = Path('/root')\n";

/// The environments every program runs under. Each variable is unset in at least one.
const ENVIRONMENTS: &[&[(&str, &str)]] = &[
    &[],
    &[("E0", "1"), ("E1", "a"), ("E2", "1")],
    &[("E0", ""), ("E2", "0")],
    &[("E0", "a"), ("E1", "")],
];

fn pick(names: &'static [&'static str]) -> BoxedStrategy<String> {
    proptest::sample::select(names)
        .prop_map(str::to_owned)
        .boxed()
}

fn env_name() -> BoxedStrategy<String> {
    pick(&["'E0'", "'E1'", "'E2'"])
}

fn str_expr(depth: u32) -> BoxedStrategy<String> {
    let leaf = Union::new([
        pick(LITERALS),
        pick(STRS),
        (env_name(), pick(LITERALS))
            .prop_map(|(env, default)| format!("os.environ.get({env}, {default})"))
            .boxed(),
        (env_name(), pick(LITERALS))
            .prop_map(|(env, default)| format!("os.getenv({env}, {default})"))
            .boxed(),
        pick(PATHS).prop_map(|path| format!("str({path})")).boxed(),
    ])
    .boxed();
    if depth == 0 {
        return leaf;
    }
    let inner = str_expr(depth - 1);
    Union::new_weighted(vec![
        (3, leaf),
        (
            1,
            (inner.clone(), inner.clone())
                .prop_map(|(left, right)| format!("({left} + {right})"))
                .boxed(),
        ),
        (
            1,
            (inner.clone(), inner.clone(), bool_expr(depth - 1))
                .prop_map(|(yes, no, test)| format!("({yes} if {test} else {no})"))
                .boxed(),
        ),
        (
            1,
            (env_name(), inner.clone())
                .prop_map(|(env, other)| format!("(os.environ.get({env}) or {other})"))
                .boxed(),
        ),
        (
            1,
            (inner.clone(), inner)
                .prop_map(|(left, right)| format!("os.path.join({left}, {right})"))
                .boxed(),
        ),
    ])
    .boxed()
}

fn bool_expr(depth: u32) -> BoxedStrategy<String> {
    let leaf = Union::new([
        pick(&["True", "False"]),
        pick(BOOLS),
        (env_name(), pick(&["'1'", "''"]))
            .prop_map(|(env, value)| format!("os.environ.get({env}) == {value}"))
            .boxed(),
        (pick(KEYS), pick(DICTS))
            .prop_map(|(key, dict)| format!("{key} in {dict}"))
            .boxed(),
    ])
    .boxed();
    if depth == 0 {
        return leaf;
    }
    let inner = bool_expr(depth - 1);
    Union::new_weighted(vec![
        (3, leaf),
        (
            1,
            inner
                .clone()
                .prop_map(|value| format!("(not {value})"))
                .boxed(),
        ),
        (
            1,
            (inner.clone(), inner.clone(), pick(&["and", "or"]))
                .prop_map(|(left, right, op)| format!("({left} {op} {right})"))
                .boxed(),
        ),
        (
            1,
            (str_expr(depth - 1), str_expr(depth - 1))
                .prop_map(|(left, right)| format!("({left} == {right})"))
                .boxed(),
        ),
        (
            1,
            (str_expr(depth - 1), pick(LISTS))
                .prop_map(|(item, list)| format!("({item} in {list})"))
                .boxed(),
        ),
    ])
    .boxed()
}

fn list_expr() -> BoxedStrategy<String> {
    Union::new([
        pick(LISTS),
        proptest::collection::vec(str_expr(1), 0..3)
            .prop_map(|items| format!("[{}]", items.join(", ")))
            .boxed(),
        (pick(LISTS), pick(LISTS))
            .prop_map(|(left, right)| format!("{left} + {right}"))
            .boxed(),
        (pick(LISTS), str_expr(1))
            .prop_map(|(list, item)| format!("[*{list}, {item}]"))
            .boxed(),
        pick(LISTS).prop_map(|list| format!("list({list})")).boxed(),
    ])
    .boxed()
}

fn dict_expr() -> BoxedStrategy<String> {
    Union::new([
        pick(DICTS),
        proptest::collection::vec((pick(KEYS), str_expr(1)), 0..3)
            .prop_map(|entries| {
                let entries: Vec<_> = entries
                    .into_iter()
                    .map(|(key, value)| format!("{key}: {value}"))
                    .collect();
                format!("{{{}}}", entries.join(", "))
            })
            .boxed(),
        (pick(DICTS), pick(KEYS), str_expr(1))
            .prop_map(|(dict, key, value)| format!("{{**{dict}, {key}: {value}}}"))
            .boxed(),
    ])
    .boxed()
}

fn path_expr() -> BoxedStrategy<String> {
    Union::new([
        pick(PATHS),
        pick(&["Path('/srv')", "Path('/root')"]),
        (pick(PATHS), pick(&["'a'", "'b'"]))
            .prop_map(|(path, part)| format!("{path} / {part}"))
            .boxed(),
        pick(PATHS)
            .prop_map(|path| format!("{path}.parent"))
            .boxed(),
    ])
    .boxed()
}

fn simple_statement() -> BoxedStrategy<Vec<String>> {
    let assign = |names: &'static [&'static str], expr: BoxedStrategy<String>| {
        (pick(names), expr)
            .prop_map(|(name, value)| vec![format!("{name} = {value}")])
            .boxed()
    };
    Union::new_weighted(vec![
        (3, assign(STRS, str_expr(2))),
        (2, assign(BOOLS, bool_expr(2))),
        (2, assign(LISTS, list_expr())),
        (2, assign(DICTS, dict_expr())),
        (1, assign(PATHS, path_expr())),
        (
            1,
            (pick(STRS), str_expr(1))
                .prop_map(|(name, value)| vec![format!("{name} += {value}")])
                .boxed(),
        ),
        (
            2,
            (
                pick(LISTS),
                str_expr(1),
                pick(&["append", "remove_safe", "insert"]),
            )
                .prop_map(|(list, item, method)| {
                    vec![match method.as_str() {
                        "append" => format!("{list}.append({item})"),
                        "insert" => format!("{list}.insert(0, {item})"),
                        _ => format!("if {item} in {list}: {list}.remove({item})"),
                    }]
                })
                .boxed(),
        ),
        (
            1,
            (pick(LISTS), list_expr())
                .prop_map(|(list, other)| vec![format!("{list} += {other}")])
                .boxed(),
        ),
        (
            1,
            (pick(LISTS), list_expr())
                .prop_map(|(list, other)| vec![format!("{list}.extend({other})")])
                .boxed(),
        ),
        (
            2,
            (pick(DICTS), pick(KEYS), str_expr(1))
                .prop_map(|(dict, key, value)| vec![format!("{dict}[{key}] = {value}")])
                .boxed(),
        ),
        (
            1,
            (pick(DICTS), dict_expr())
                .prop_map(|(dict, other)| vec![format!("{dict}.update({other})")])
                .boxed(),
        ),
        (
            1,
            (pick(DICTS), pick(KEYS))
                .prop_map(|(dict, key)| vec![format!("{dict}.pop({key}, None)")])
                .boxed(),
        ),
        (
            1,
            (pick(STRS), pick(STRS), str_expr(1), str_expr(1))
                .prop_map(|(left, right, first, second)| {
                    vec![format!("{left}, {right} = {first}, {second}")]
                })
                .boxed(),
        ),
    ])
    .boxed()
}

fn indent(lines: Vec<String>) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| format!("    {line}"))
        .collect()
}

fn block(depth: u32) -> BoxedStrategy<Vec<String>> {
    proptest::collection::vec(statement(depth), 1..3)
        .prop_map(|statements| statements.into_iter().flatten().collect())
        .boxed()
}

fn statement(depth: u32) -> BoxedStrategy<Vec<String>> {
    if depth == 0 {
        return simple_statement();
    }
    Union::new_weighted(vec![
        (4, simple_statement()),
        (
            2,
            (
                bool_expr(1),
                block(depth - 1),
                proptest::option::of(block(depth - 1)),
            )
                .prop_map(|(test, body, orelse)| {
                    let mut lines = vec![format!("if {test}:")];
                    lines.extend(indent(body));
                    if let Some(orelse) = orelse {
                        lines.push("else:".to_owned());
                        lines.extend(indent(orelse));
                    }
                    lines
                })
                .boxed(),
        ),
        (
            1,
            (
                block(depth - 1),
                any::<bool>(),
                bool_expr(1),
                block(depth - 1),
            )
                .prop_map(|(body, raises, test, handler)| {
                    let mut lines = vec!["try:".to_owned()];
                    lines.extend(indent(body));
                    if raises {
                        lines.push(format!("    if {test}: raise ValueError()"));
                    }
                    lines.push("except ValueError:".to_owned());
                    lines.extend(indent(handler));
                    lines
                })
                .boxed(),
        ),
        (
            1,
            (pick(LISTS), pick(STRS), block(depth - 1))
                .prop_map(|(list, name, body)| {
                    let mut lines = vec![format!("for {name} in list({list}):")];
                    lines.extend(indent(body));
                    lines
                })
                .boxed(),
        ),
    ])
    .boxed()
}

fn program() -> impl Strategy<Value = String> {
    proptest::collection::vec(statement(2), 1..8).prop_map(|statements| {
        let mut source = PRELUDE.to_owned();
        for line in statements.into_iter().flatten() {
            source.push_str(&line);
            source.push('\n');
        }
        source
    })
}

/// Runs a program under every environment and reports each name's value as tagged JSON.
const HARNESS: &str = r#"
import json, os, pathlib, sys, types

def encode(value):
    if isinstance(value, bool):
        return {"bool": value}
    if isinstance(value, str):
        return {"str": value}
    if isinstance(value, pathlib.PurePath):
        return {"path": str(value)}
    if isinstance(value, list):
        return {"list": [encode(item) for item in value]}
    if isinstance(value, tuple):
        return {"tuple": [encode(item) for item in value]}
    if isinstance(value, dict):
        return {"dict": [[encode(key), encode(item)] for key, item in value.items()]}
    if isinstance(value, types.ModuleType):
        return {"module": value.__name__}
    return {"other": type(value).__name__}

# Lists that grow in place inside loops, through aliases or nested loops over themselves, grow
# exponentially; raise MemoryError instead of exhausting the machine.
try:
    import resource
    _, hard = resource.getrlimit(resource.RLIMIT_AS)
    soft = 1 << 30 if hard == resource.RLIM_INFINITY else min(1 << 30, hard)
    resource.setrlimit(resource.RLIMIT_AS, (soft, hard))
except (ImportError, OSError, ValueError):
    pass

request = json.load(sys.stdin)
results = []
for environment in request["environments"]:
    os.environ.clear()
    os.environ.update(environment)
    namespace = {"__name__": "settings"}
    try:
        exec(compile(request["source"], "/project/settings.py", "exec"), namespace)
    except Exception as error:
        results.append({"error": repr(error)})
        continue
    results.append({name: encode(namespace[name]) for name in request["names"] if name in namespace})
json.dump(results, sys.stdout)
"#;

fn python() -> String {
    std::env::var("DJLS_TEST_PYTHON")
        .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.to_owned())
}

fn names() -> Vec<&'static str> {
    [STRS, BOOLS, LISTS, DICTS, PATHS].concat()
}

fn run_cpython(source: &str) -> Vec<Json> {
    let request = serde_json::json!({
        "source": source,
        "names": names(),
        "environments": ENVIRONMENTS
            .iter()
            .map(|environment| {
                environment
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), Json::from(*value)))
                    .collect::<serde_json::Map<_, _>>()
            })
            .collect::<Vec<_>>(),
    });
    let mut child = Command::new(python())
        .args(["-I", "-c", HARNESS])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("CPython should start; set DJLS_TEST_PYTHON to choose an interpreter");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(request.to_string().as_bytes())
        .expect("CPython should read the request");
    let output = child.wait_with_output().expect("CPython should finish");
    assert!(
        output.status.success(),
        "CPython harness failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("CPython harness prints JSON")
}

fn evaluate(source: &str) -> PythonModuleFacts {
    let mut db = TestDatabase::new();
    db.add_file("/project/settings.py", source)
        .expect("in-memory write");
    let project =
        crate::project::Project::initial(&db, Utf8Path::new("/project"), &Settings::default());
    let search_paths = crate::python::SearchPaths::from_project_settings(
        db.file_system(),
        Utf8Path::new("/project"),
        &crate::python::PythonEnvironment::Auto,
        &[],
    );
    search_paths.register_roots(&db);
    project.set_search_paths(&mut db).to(search_paths);
    let module = crate::python::file_to_module(&db, project, "/project/settings.py".into())
        .expect("settings module resolves");
    python_module_facts(&db, project, module, EvaluationDemand::Full)
        .as_ref()
        .expect("settings module evaluates")
        .clone()
}

// Whether the evaluator's abstract `value` allows the `concrete` value CPython produced.
fn admits(value: &PythonValue, concrete: &Json) -> bool {
    let tagged = |tag: &str| concrete.get(tag);
    match &value.kind {
        PythonValueKind::Unknown(_) | PythonValueKind::Intrinsic(_) => true,
        PythonValueKind::UnsupportedLiteral => tagged("other").is_some(),
        PythonValueKind::Str(text) => tagged("str").and_then(Json::as_str) == Some(text),
        PythonValueKind::Bool(flag) => tagged("bool").and_then(Json::as_bool) == Some(*flag),
        PythonValueKind::Path(path) => {
            tagged("path").and_then(Json::as_str) == Some(path.as_path().as_str())
        }
        PythonValueKind::Module(module) => {
            tagged("module").and_then(Json::as_str) == Some(module.name().as_str())
        }
        PythonValueKind::List(list) => tagged("list")
            .and_then(Json::as_array)
            .is_some_and(|items| sequence_admits(PythonMaterializedSequence::List(list), items)),
        PythonValueKind::Tuple(tuple) => tagged("tuple")
            .and_then(Json::as_array)
            .is_some_and(|items| sequence_admits(PythonMaterializedSequence::Tuple(tuple), items)),
        PythonValueKind::Dict(dict) => tagged("dict")
            .and_then(Json::as_array)
            .is_some_and(|entries| dict_admits(dict, entries)),
    }
}

fn sequence_admits(sequence: PythonMaterializedSequence<'_>, concrete: &[Json]) -> bool {
    let mut alternatives = sequence.alternatives().peekable();
    if alternatives.peek().is_none() {
        return items_admit(sequence.semantic_items(), concrete);
    }
    alternatives.any(|alternative| match alternative {
        PythonSequenceAlternativeRef::Exact { items, .. } => items_admit(items, concrete),
        PythonSequenceAlternativeRef::Remainder { .. } => true,
    })
}

/// Match items in order; an unknown unpack absorbs any run of concrete items.
fn items_admit(items: &[PythonSequenceItem], concrete: &[Json]) -> bool {
    match items.split_first() {
        None => concrete.is_empty(),
        Some((PythonSequenceItem::UnknownUnpack(_), rest)) => {
            (0..=concrete.len()).any(|taken| items_admit(rest, &concrete[taken..]))
        }
        Some((item, rest)) => concrete.split_first().is_some_and(|(first, tail)| {
            let matches = match item {
                PythonSequenceItem::Value(value) => admits(value, first),
                PythonSequenceItem::UnknownElement(_) | PythonSequenceItem::UnknownUnpack(_) => {
                    true
                }
            };
            matches && items_admit(rest, tail)
        }),
    }
}

fn dict_admits(dict: &super::PythonDict, concrete: &[Json]) -> bool {
    let mapping = dict.mapping();
    let concrete_key = |entry: &Json| {
        entry[0]
            .get("str")
            .and_then(Json::as_str)
            .map(str::to_owned)
    };
    // Every concrete entry must be allowed by the latest write or a later unknown write.
    let entries_admitted = concrete.iter().all(|entry| {
        let Some(key) = concrete_key(entry) else {
            return true;
        };
        let lookup = mapping.lookup_string_key(&key);
        !lookup.overrides().is_empty()
            || lookup.value().is_some_and(|value| admits(value, &entry[1]))
    });
    // Every key the evaluator states exactly must exist in CPython.
    let keys_present = mapping.effective_string_entries().iter().all(|evidence| {
        let super::MappingEntryEvidence::Value { key, .. } = evidence else {
            return true;
        };
        concrete
            .iter()
            .any(|entry| concrete_key(entry).as_deref() == Some(key))
    });
    let shadowed = mapping
        .effective_string_entries()
        .iter()
        .any(|evidence| !matches!(evidence, super::MappingEntryEvidence::Value { .. }));
    entries_admitted && (keys_present || shadowed)
}

fn binding_admits(facts: &PythonModuleFacts, name: &str, concrete: Option<&Json>) -> bool {
    let Some(binding) = facts.bindings.get(name) else {
        return concrete.is_none() || facts.namespace_remainder.is_some();
    };
    binding.alternatives().any(|state| match (state, concrete) {
        (PythonBindingState::Unbound, None) => true,
        (PythonBindingState::Bound(bound), Some(concrete)) => admits(&bound.value, concrete),
        _ => false,
    })
}

fn describe_binding(facts: &PythonModuleFacts, name: &str) -> String {
    facts.bindings.get(name).map_or_else(
        || "absent".to_owned(),
        |binding| {
            let mut out = String::new();
            for state in binding.alternatives() {
                match state {
                    PythonBindingState::Unbound => out.push_str("\n    unbound"),
                    PythonBindingState::Bound(bound) => {
                        write!(out, "\n    {:?}", bound.value.kind).expect("writing to a String");
                    }
                }
            }
            out
        },
    )
}

proptest! {
    // Small enough for every `cargo test`; set PROPTEST_CASES for a deeper run.
    #![proptest_config(ProptestConfig {
        cases: std::env::var("PROPTEST_CASES").ok().and_then(|cases| cases.parse().ok()).unwrap_or(64),
        ..ProptestConfig::default()
    })]

    #[test]
    fn evaluator_admits_every_value_cpython_produces(source in program()) {
        let runs = run_cpython(&source);
        // Generated programs avoid runtime errors, but a rare one says nothing about the evaluator.
        prop_assume!(runs.iter().all(|run| run.get("error").is_none()));
        let facts = evaluate(&source);
        for (environment, run) in ENVIRONMENTS.iter().zip(&runs) {
            for name in names() {
                let concrete = run.get(name);
                prop_assert!(
                    binding_admits(&facts, name, concrete),
                    "{name} under {environment:?}\nCPython: {}\nevaluator:{}\n\n{source}",
                    concrete.map_or_else(|| "unbound".to_owned(), ToString::to_string),
                    describe_binding(&facts, name),
                );
            }
        }
    }
}
