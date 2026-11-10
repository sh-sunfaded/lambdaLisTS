use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Output;
use std::rc::Rc;

use lists::backends::{self, Artifacts, Backend, SourceFile};
use lists::desugar::{desugar, CoreDef};
use lists::diagnostic::Diagnostic;
use lists::eval::{global_env, run_examples, Interp};
use lists::lexer::tokenize;
use lists::parser::parse_program;
use lists::resolver::resolve_import_path;
use lists::value::{print_value, structural_equal, Env, Value};
use serde_json::Value as Json;

pub fn fixture_path(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

pub fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lists-m10-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

pub fn pipeline_file(abs: &str) -> (Vec<CoreDef>, Vec<Diagnostic>) {
    let src = std::fs::read_to_string(abs).expect("fixture readable");
    let lex = tokenize(&src, abs);
    let (program, mut diags) = parse_program(&lex);
    let (defs, ds_diags) = desugar(&program);
    diags.extend(ds_diags);
    (defs, diags)
}

/// Backend トレイト経由でコンパイルする。
pub fn compile_via(backend: &dyn Backend, names: &[&str], dir: &Path) -> Artifacts {
    compile_via_seed(backend, names, dir, lists::rng::DEFAULT_SEED)
}

pub fn compile_via_seed(backend: &dyn Backend, names: &[&str], dir: &Path, seed: u64) -> Artifacts {
    let sources: Vec<SourceFile> = names
        .iter()
        .map(|n| SourceFile {
            path: PathBuf::from(fixture_path(n)),
            name: n.to_string(),
        })
        .collect();
    backend
        .compile(&sources, dir, seed)
        .unwrap_or_else(|errs| panic!("compile {names:?}: {errs:?}"))
}

pub fn compile_to_dir(names: &[&str], dir: &Path) {
    compile_via(&backends::JsBackend, names, dir);
}

/// テストランナーのバックエンド選択（環境変数 LISTS_BACKEND、デフォルト js）。
pub fn backend_from_env() -> Box<dyn Backend> {
    let name = std::env::var("LISTS_BACKEND").unwrap_or_else(|_| "js".to_string());
    backends::lookup(&name)
        .unwrap_or_else(|| panic!("unknown LISTS_BACKEND '{name}' (available: {:?})", backends::names()))
}

/// バックエンド実行の stdout から example 値を取り出す（`example N: ok <json>` 形式）。
pub fn example_values_from_stdout(stdout: &str, count: usize) -> Vec<Json> {
    let mut values = Vec::new();
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("example ") {
            if let Some((_idx, rest)) = rest.split_once(": ok ") {
                values.push(serde_json::from_str(rest).expect("example > JSON"));
            }
        }
    }
    if values.len() > count {
        values.drain(..values.len() - count);
    }
    values
}

/// 差分テスト本体: インタープリタ vs 任意バックエンド。対象ケースは従来の T7/T10 と不変。
pub fn assert_diff(backend: &dyn Backend, compile_names: &[&str], entry_name: &str) {
    let base = entry_name.trim_end_matches(".lts");
    let dir = temp_dir(&format!("diff-{base}"));
    let artifacts = compile_via(backend, compile_names, &dir);

    let (defs, diags) = pipeline_file(&fixture_path(entry_name));
    assert!(diags.is_empty(), "{entry_name}: {diags:?}");
    let example_count = defs
        .iter()
        .filter(|d| matches!(d, lists::desugar::CoreDef::Example { .. }))
        .count();
    assert!(example_count >= 1, "{entry_name} に example が必要");

    let run = backend.run(&artifacts).expect("backend run");
    assert_eq!(
        run.code,
        Some(0),
        "{entry_name}: backend '{}' run failed: {}",
        backend.name(),
        run.stderr
    );
    let backend_values = example_values_from_stdout(&run.stdout, example_count);
    let interp = interp_example_values(&[entry_name]);
    assert_eq!(interp.len(), example_count, "{entry_name}");
    assert_eq!(backend_values.len(), example_count, "{entry_name}");
    for (i, (pass, actual)) in interp.iter().enumerate() {
        assert!(
            *pass,
            "{entry_name} example {i}: インタープリタが pass していない: {}",
            print_value(actual)
        );
        let backend_value = json_to_value(&backend_values[i]);
        assert!(
            values_equal(actual, &backend_value),
            "{entry_name} example {i} が不一致:
interp: {}
backend({}): {}",
            print_value(actual),
            backend.name(),
            serde_json::to_string_pretty(&backend_values[i]).expect("json")
        );
    }
}

pub fn run_node(file: &str, cwd: &Path, args: &[&str]) -> Output {
    lists::backends::run_node_entry(cwd, file, args).expect("node を起動できること (Node.js >= 20)")
}

pub fn interp_example_values(names: &[&str]) -> Vec<(bool, Value)> {
    let env: Rc<Env> = global_env();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut own_defs: Vec<CoreDef> = Vec::new();
    for name in names {
        let abs = fixture_path(name);
        let mut queue = vec![abs.clone()];
        let mut first = true;
        while let Some(path) = queue.pop() {
            if !seen.insert(path.clone()) {
                continue;
            }
            let (defs, diags) = pipeline_file(&path);
            assert!(diags.is_empty(), "{path}: {diags:?}");
            for d in &defs {
                if let CoreDef::Import { path: import_path, .. } = d {
                    if let Some(target) = resolve_import_path(&path, import_path) {
                        queue.push(target);
                    }
                }
            }
            lists::eval::load_defs(&defs, &env);
            if first {
                own_defs.extend(defs);
                first = false;
            }
        }
    }
    let mut interp = Interp::new();
    let results = run_examples(&mut interp, &own_defs, &env).expect("examples run");
    results.into_iter().map(|r| (r.pass, r.actual)).collect()
}

pub fn json_to_value(v: &Json) -> Value {
    match v {
        Json::Null => Value::Nil,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => Value::Num(n.as_f64().expect("f64")),
        Json::String(s) => Value::Str(Rc::from(s.as_str())),
        Json::Array(a) => Value::Array(a.iter().map(json_to_value).collect()),
        Json::Object(m) => {
            if m.get("tag") == Some(&Json::String("Map".to_string())) {
                if let Some(Json::Array(entries)) = m.get("entries") {
                    let mut pm = lists::pmap::PersistentMap::new();
                    for e in entries {
                        if let Json::Array(pair) = e {
                            if let (Some(k), Some(v)) = (pair.first(), pair.get(1)) {
                                let kv = json_to_value(k);
                                if let Some(mk) = lists::pmap::MapKey::from_value(&kv) {
                                    let (next, _) = pm.insert(mk, json_to_value(v));
                                    pm = next;
                                }
                            }
                        }
                    }
                    return Value::Map(Rc::new(pm));
                }
            }
            if m.get("tag") == Some(&Json::String("Set".to_string())) {
                if let Some(Json::Array(items)) = m.get("items") {
                    let mut pm = lists::pmap::PersistentMap::new();
                    for it in items {
                        let kv = json_to_value(it);
                        if let Some(mk) = lists::pmap::MapKey::from_value(&kv) {
                            let (next, _) = pm.insert(mk, Value::Nil);
                            pm = next;
                        }
                    }
                    return Value::Set(Rc::new(pm));
                }
            }
            if let (Some(tag), Some(val)) = (m.get("tag"), m.get("value")) {
                if tag == "Ok" {
                    return Value::Ok(Box::new(json_to_value(val)));
                }
                if tag == "Err" {
                    return Value::Err(Box::new(json_to_value(val)));
                }
                if tag == "Dec" {
                    if let Json::String(s) = val {
                        return Value::Dec(
                            lists::decimal::parse_dec_scientific(s).unwrap_or(0),
                        );
                    }
                }
            }
            Value::Obj(
                m.iter()
                    .map(|(k, v)| (Rc::from(k.as_str()), json_to_value(v)))
                    .collect::<BTreeMap<_, _>>(),
            )
        }
    }
}

pub fn normalize_diags(diags: &[Diagnostic]) -> Json {
    let v = serde_json::to_value(diags).expect("ser diags");
    strip_span_file(v)
}

fn strip_span_file(v: Json) -> Json {
    match v {
        Json::Object(m) => Json::Object(
            m.into_iter()
                .map(|(k, val)| {
                    if k == "span" {
                        if let Json::Object(mut sm) = val {
                            if let Some(Json::String(f)) = sm.get("file") {
                                let base = Path::new(f).file_name().map(|b| b.to_string_lossy().into_owned()).unwrap_or_else(|| f.clone());
                                sm.insert("file".to_string(), Json::String(base));
                            }
                            return (k, Json::Object(sm));
                        }
                    }
                    (k, strip_span_file(val))
                })
                .collect(),
        ),
        Json::Array(a) => Json::Array(a.into_iter().map(strip_span_file).collect()),
        other => other,
    }
}

pub fn assert_golden_json(label: &str, actual: Json) {
    let path = format!("{}/tests/golden/{label}", env!("CARGO_MANIFEST_DIR"));
    if std::env::var("UPDATE_SNAPSHOTS").is_ok() {
        let pretty = serde_json::to_string_pretty(&actual).expect("pretty");
        std::fs::write(&path, format!("{pretty}\n")).expect("write golden");
    }
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("golden がありません: {path} (UPDATE_SNAPSHOTS=1 cargo test で生成)"));
    let expected: Json = serde_json::from_str(&content).expect("golden json");
    assert_eq!(actual, expected, "{path}");
}

pub fn values_equal(a: &Value, b: &Value) -> bool {
    structural_equal(a, b).unwrap_or(false)
}
