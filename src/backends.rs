//! バックエンド抽象（M13）。
//!
//! 差分テストを「インタープリタ vs JS」から「インタープリタ vs 任意バックエンド」へ一般化する。
//! 新バックエンド（例: WASM）は `Backend` の3メソッドを実装して `registry()` に登録するだけで、
//! 差分テスト（tests/acceptance/diff.rs）の対象に乗る。詳細は docs/backends.md を参照。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;

use crate::check::{check_file, ModuleCache};
use crate::desugar::{desugar, CoreDef};
use crate::diagnostic::{DiagKind, Diagnostic, Severity};
use crate::emit::{emit, EmitInput};
use crate::eval::{global_env, load_file, run_examples, Interp};
use crate::lexer::tokenize;
use crate::parser::parse_program;
use crate::runtime::RUNTIME_JS;
use crate::span::Span;
use crate::value::Value;
use std::rc::Rc;

/// コンパイル対象のソースファイル。
pub struct SourceFile {
    /// 実ファイルパス。
    pub path: PathBuf,
    /// ソースファイル名（source map の `sources` に載る名前）。
    pub name: String,
}

/// コンパイル成果物。`entry` はバックエンドごとの実行エントリ（js なら .js ファイル名）。
pub struct Artifacts {
    pub dir: PathBuf,
    pub entry: String,
}

/// バックエンド実行の結果。環境変数は子プロセスに継承され、stderr は取り込まれる（現行どおり）。
pub struct RunOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i32>,
}

/// バックエンドの規約。新バックエンド追加時はこの3メソッドを実装する。
pub trait Backend {
    /// バックエンド名（`--backend <name>` / `LISTS_BACKEND` で指定される名前）。
    fn name(&self) -> &str;
    /// sources を out_dir にコンパイルする。診断があれば Err で返す（成果物は出せる範囲で書く）。
    fn compile(&self, sources: &[SourceFile], out_dir: &Path, seed: u64) -> Result<Artifacts, Vec<Diagnostic>>;
    /// 成果物を実行して標準出力を得る。子プロセス実行・環境変数継承・stderr 取り込みは各実装の契約どおり。
    fn run(&self, artifacts: &Artifacts) -> std::io::Result<RunOutput>;
}

fn read_diag(file: &str, message: String) -> Diagnostic {
    Diagnostic {
        kind: DiagKind::ImportNotFound,
        severity: Severity::Error,
        span: Span {
            file: Rc::from(file),
            start: 0,
            end: 0,
            line: 1,
            col: 1,
        },
        message,
        data: None,
        fix: None,
    }
}

fn out_name_for(source_name: &str) -> String {
    source_name.replace(".lts", ".js")
}

/// JS バックエンド（現行どおり: .js + .js.map + runtime.js を出し、Node 子プロセスで実行する）。
pub struct JsBackend;

/// Node 子プロセスで `dir/file` を実行する（環境変数継承・stdout/stderr 取り込み・exit code 返却）。
pub fn run_node_entry(dir: &Path, file: &str, args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("node")
        .args(args)
        .arg(file)
        .current_dir(dir)
        .output()
}

fn run_output_of(o: std::process::Output) -> RunOutput {
    RunOutput {
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        code: o.status.code(),
    }
}

/// フロントエンドの1ソース分の結果。
pub struct Frontend {
    pub file: String,
    pub defs: Vec<CoreDef>,
    pub type_names: BTreeSet<String>,
    /// import 先も含む defn! 名の集合（emit の async 判定用 §7.2/§12.4）。
    pub impure_names: BTreeSet<String>,
    /// 型エイリアス環境（prop の型ディスクリプタ生成用）
    pub aliases: crate::types::AliasEnv,
}

/// 共通のフロントエンド（lex → parse → desugar → check）をソースごとに回し、診断を集める。
fn frontend(sources: &[SourceFile]) -> (Vec<Diagnostic>, Vec<Frontend>) {
    let mut diagnostics = Vec::new();
    let mut out = Vec::new();
    let mut cache = ModuleCache::default();
    for sf in sources {
        let file_s = sf.path.to_string_lossy().into_owned();
        let src = match std::fs::read_to_string(&sf.path) {
            Ok(s) => s,
            Err(e) => {
                diagnostics.push(read_diag(&file_s, format!("ファイルを読めません: '{file_s}': {e}")));
                continue;
            }
        };
        let lex = tokenize(&src, &file_s);
        let (program, parse_diags) = parse_program(&lex);
        diagnostics.extend(lex.diagnostics.iter().cloned());
        diagnostics.extend(parse_diags);
        let (defs, ds_diags) = desugar(&program);
        diagnostics.extend(ds_diags);
        let checked = check_file(&file_s, &mut cache);
        diagnostics.extend(checked.diagnostics.iter().cloned());
        let type_names: BTreeSet<String> = checked.type_aliases.keys().map(|k| k.to_string()).collect();
        out.push(Frontend {
            file: file_s,
            defs,
            type_names,
            impure_names: BTreeSet::new(),
            aliases: checked.type_aliases.clone(),
        });
    }
    // import 先モジュールも含めた defn! 名を集める（check の cache に全モジュールが入っている）
    let impure_names: BTreeSet<String> = cache
        .modules
        .values()
        .flat_map(|m| m.defn_purity.iter())
        .filter(|(_, p)| !**p)
        .map(|(n, _)| n.to_string())
        .collect();
    for fe in out.iter_mut() {
        fe.impure_names = impure_names.clone();
    }
    (diagnostics, out)
}

impl Backend for JsBackend {
    fn name(&self) -> &str {
        "js"
    }

    fn compile(&self, sources: &[SourceFile], out_dir: &Path, seed: u64) -> Result<Artifacts, Vec<Diagnostic>> {
        let _ = std::fs::create_dir_all(out_dir);
        let (mut diagnostics, frontends) = frontend(sources);
        let mut entries: Vec<String> = Vec::new();
        for (sf, fe) in sources.iter().zip(frontends.iter()) {
            let out_name = out_name_for(&sf.name);
            match emit(&EmitInput {
                defs: fe.defs.clone(),
                file: out_name.clone(),
                source_file: sf.name.clone(),
                type_names: fe.type_names.clone(),
                impure_names: fe.impure_names.clone(),
                seed,
                aliases: fe.aliases.clone(),
            }) {
                Ok(out) => {
                    let p = out_dir.join(&out_name);
                    let _ = std::fs::write(&p, &out.js);
                    let _ = std::fs::write(format!("{}.map", p.display()), &out.map);
                    if out.uses_prop {
                        let _ = std::fs::write(out_dir.join("prop-runtime.js"), crate::runtime::PROP_RUNTIME_JS);
                    }
                    entries.push(out_name);
                }
                Err(errs) => diagnostics.extend(errs),
            }
        }
        let runtime_path = out_dir.join("runtime.js");
        if !runtime_path.exists() {
            let _ = std::fs::write(&runtime_path, RUNTIME_JS);
        }
        if diagnostics.is_empty() {
            Ok(Artifacts {
                dir: out_dir.to_path_buf(),
                entry: entries.last().cloned().unwrap_or_default(),
            })
        } else {
            Err(diagnostics)
        }
    }

    fn run(&self, artifacts: &Artifacts) -> std::io::Result<RunOutput> {
        run_node_entry(&artifacts.dir, &artifacts.entry, &[]).map(run_output_of)
    }
}

/// Value をバックエンド出力と同じ JSON 形（Ok/Err は {tag, value}）に変換する（モック用）。
pub fn value_to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Num(n) => serde_json::Number::from_f64(*n).map_or(serde_json::Value::Null, serde_json::Value::Number),
        Value::Dec(s) => json!({ "tag": "Dec", "value": crate::decimal::fmt_dec(*s) }),
        Value::Str(s) => serde_json::Value::String(s.to_string()),
        Value::Bool(b) => serde_json::Value::Bool(*b),
        // char は JS 側で1要素文字列なので JSON でも素の文字列として扱う
        Value::Char(c) => serde_json::Value::String(c.to_string()),
        Value::Nil => serde_json::Value::Null,
        Value::Array(xs) => serde_json::Value::Array(xs.iter().map(value_to_json).collect()),
        Value::Obj(m) => serde_json::Value::Object(
            m.iter()
                .map(|(k, val)| (k.to_string(), value_to_json(val)))
                .collect(),
        ),
        Value::Ok(x) => json!({ "tag": "Ok", "value": value_to_json(x) }),
        Value::Err(x) => json!({ "tag": "Err", "value": value_to_json(x) }),
        Value::Ref(_) => serde_json::Value::String("<ref>".to_string()),
        // bytes は JS 側が Uint8Array なので $mjson と同じく数値配列として直列化
        Value::Bytes(b) => serde_json::Value::Array(b.iter().map(|x| serde_json::json!(x)).collect()),
        Value::Builder(parts) => serde_json::Value::Array(parts.iter().map(|p| serde_json::Value::String(p.clone())).collect()),
        Value::Map(m) => json!({
            "tag": "Map",
            "entries": m.entries().iter().map(|(k, v)| json!([value_to_json(&k.to_value()), value_to_json(v)])).collect::<Vec<_>>()
        }),
        Value::Set(s) => json!({
            "tag": "Set",
            "items": s.keys().iter().map(|k| value_to_json(&k.to_value())).collect::<Vec<_>>()
        }),
        Value::Closure(_) | Value::Builtin(_) => serde_json::Value::String("<fn>".to_string()),
    }
}

/// モックバックエンド。compile はフロントエンドだけ通し、run はインタープリタ結果を
/// `example N: ok <json>` 行としてそのまま返す。拡張性（トレイト経由で差分テストが機能すること）の立証用。
pub struct MockBackend {
    sources: std::cell::RefCell<Vec<PathBuf>>,
}

impl MockBackend {
    pub fn new() -> Self {
        MockBackend {
            sources: std::cell::RefCell::new(Vec::new()),
        }
    }
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for MockBackend {
    fn name(&self) -> &str {
        "mock"
    }

    fn compile(&self, sources: &[SourceFile], out_dir: &Path, _seed: u64) -> Result<Artifacts, Vec<Diagnostic>> {
        let (diagnostics, _frontends) = frontend(sources);
        *self.sources.borrow_mut() = sources.iter().map(|s| s.path.clone()).collect();
        let _ = std::fs::create_dir_all(out_dir);
        if diagnostics.is_empty() {
            Ok(Artifacts {
                dir: out_dir.to_path_buf(),
                entry: "mock".to_string(),
            })
        } else {
            Err(diagnostics)
        }
    }

    fn run(&self, _artifacts: &Artifacts) -> std::io::Result<RunOutput> {
        let to_io = |e: crate::eval::EvalError| std::io::Error::new(std::io::ErrorKind::Other, e.to_string());
        let env = global_env();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let paths = self.sources.borrow().clone();
        let mut own_defs: Vec<CoreDef> = Vec::new();
        let n = paths.len();
        for (i, p) in paths.iter().enumerate() {
            let defs = load_file(&p.to_string_lossy(), &env, &mut seen).map_err(to_io)?;
            if i + 1 == n {
                own_defs = defs;
            }
        }
        let mut interp = Interp::new();
        let results = run_examples(&mut interp, &own_defs, &env).map_err(to_io)?;
        let mut stdout = String::new();
        for r in &results {
            if r.pass {
                stdout.push_str(&format!("example {}: ok {}\n", r.index, value_to_json(&r.actual)));
            } else {
                stdout.push_str(&format!("example {}: FAIL {}\n", r.index, value_to_json(&r.actual)));
            }
        }
        Ok(RunOutput {
            stdout,
            stderr: String::new(),
            code: Some(0),
        })
    }
}

/// 登録済みバックエンド一覧。
pub fn registry() -> Vec<Box<dyn Backend>> {
    vec![Box::new(JsBackend), Box::new(MockBackend::new())]
}

/// 登録済みバックエンド名（`--backend` の候補）。
pub fn names() -> Vec<&'static str> {
    vec!["js", "mock"]
}

/// 名前からバックエンドを取得する。
pub fn lookup(name: &str) -> Option<Box<dyn Backend>> {
    registry().into_iter().find(|b| b.name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_js_and_mock() {
        assert_eq!(names(), vec!["js", "mock"]);
        assert!(lookup("js").is_some());
        assert!(lookup("mock").is_some());
        assert!(lookup("wasm").is_none());
    }

    #[test]
    fn value_to_json_matches_backend_shape() {
        let v = Value::Ok(Box::new(Value::Obj(
            [(
                Rc::from("amount"),
                Value::Num(3.0),
            )]
            .into_iter()
            .collect(),
        )));
        assert_eq!(
            value_to_json(&v),
            json!({ "tag": "Ok", "value": { "amount": 3.0 } })
        );
        assert_eq!(value_to_json(&Value::Nil), serde_json::Value::Null);
        assert_eq!(
            value_to_json(&Value::Err(Box::new(Value::Str(Rc::from("e"))))),
            json!({ "tag": "Err", "value": "e" })
        );
    }
}
