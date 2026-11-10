use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use lists::check::{check_file, ModuleCache};
use lists::desugar::desugar_expr;
use lists::eval::{eval_core, global_env, load_defs, Interp};
use lists::host::{inject_host, MockHost};
use lists::parser::parse_expr;
use lists::desugar::{desugar, CoreDef};
use lists::diagnostic::Diagnostic;
use lists::emit::{emit, EmitInput, EmitOutput};
use lists::lexer::tokenize;
use lists::parser::parse_program;
use lists::runtime::RUNTIME_JS;

fn fixture_path(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn pipeline(abs: &str) -> (Vec<CoreDef>, Vec<Diagnostic>) {
    let src = std::fs::read_to_string(abs).expect("fixture readable");
    let lex = tokenize(&src, abs);
    let (program, mut diags) = parse_program(&lex);
    let (defs, ds_diags) = desugar(&program);
    diags.extend(ds_diags);
    (defs, diags)
}

fn emit_fixture(base: &str) -> Result<EmitOutput, Vec<Diagnostic>> {
    let abs = fixture_path(&format!("{base}.lts"));
    let (defs, diags) = pipeline(&abs);
    assert!(diags.is_empty(), "{base}: {diags:?}");
    let mut cache = ModuleCache::default();
    let checked = check_file(&abs, &mut cache);
    assert!(
        checked.diagnostics.is_empty(),
        "{base}: {:?}",
        checked.diagnostics
    );
    let type_names: BTreeSet<String> = checked.type_aliases.keys().map(|k| k.to_string()).collect();
    emit(&EmitInput {
        defs,
        file: format!("{base}.js"),
        source_file: format!("{base}.lts"),
        type_names,
        impure_names: BTreeSet::new(),
        seed: lists::rng::DEFAULT_SEED,
        aliases: Default::default(),
    })
}

fn emit_src(src: &str, file: &str) -> Result<EmitOutput, Vec<Diagnostic>> {
    let lex = tokenize(src, "test.lts");
    let (program, _) = parse_program(&lex);
    let (defs, _) = desugar(&program);
    emit(&EmitInput {
        defs,
        file: file.to_string(),
        source_file: "test.lts".to_string(),
        type_names: BTreeSet::new(),
        impure_names: BTreeSet::new(),
        seed: lists::rng::DEFAULT_SEED,
        aliases: Default::default(),
    })
}

fn write_out(dir: &Path, name: &str, out: &EmitOutput) {
    std::fs::write(dir.join(format!("{name}.js")), &out.js).expect("write js");
    std::fs::write(dir.join(format!("{name}.js.map")), &out.map).expect("write map");
    std::fs::write(dir.join("runtime.js"), RUNTIME_JS).expect("write runtime");
    if out.uses_prop {
        std::fs::write(dir.join("prop-runtime.js"), lists::runtime::PROP_RUNTIME_JS).expect("write prop-runtime");
    }
}

fn run_node(dir: &Path, args: &[&str]) -> Output {
    Command::new("node")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("node を起動できること (Node.js >= 20 が必要)")
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lists-m8-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn assert_golden(label: &str, actual: &str) {
    let path = format!("{}/tests/golden/{label}", env!("CARGO_MANIFEST_DIR"));
    if std::env::var("UPDATE_SNAPSHOTS").is_ok() {
        std::fs::write(&path, actual).expect("write golden");
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("golden がありません: {path} (UPDATE_SNAPSHOTS=1 cargo test で生成)"));
    assert_eq!(expected, actual, "{path}");
}

#[test]
fn money_emit_is_deterministic_and_snapshot() {
    let r1 = emit_fixture("money").expect("emit money");
    let r2 = emit_fixture("money").expect("emit money again");
    assert_eq!(r1.js, r2.js, "JS は決定的であること");
    assert_eq!(r1.map, r2.map, "source map は決定的であること");
    // 生成JSの文字列表現への直接検査は行わない（M14）。実行挙動は money_js_runs_example_in_node で検査済み。
    let map_json: serde_json::Value = serde_json::from_str(&r1.map).expect("map json");
    assert_eq!(map_json["version"], 3);
    assert_eq!(map_json["sources"], serde_json::json!(["money.lts"]));
    assert_golden("money.js", &r1.js);
    assert_golden("money.js.map", &r1.map);
}

#[test]
fn cart_emit_imports_money_and_do_early_return() {
    let cart = emit_fixture("cart").expect("emit cart");
    // 挙動検査（M14: 生成JSの文字列ではなく実行結果で検証）: money.js と共にコンパイルして実行し、
    // import 解決と do 早期リターンが結果として Err(EmptyCart) になることを確認する
    let dir = temp_dir("cart-emit");
    let money = emit_fixture("money").expect("emit money");
    write_out(&dir, "money", &money);
    write_out(&dir, "cart", &cart);
    let r = run_node(&dir, &["cart.js"]);
    let stdout = String::from_utf8_lossy(&r.stdout);
    assert_eq!(r.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&r.stderr));
    assert!(stdout.contains("example 0: ok"), "{stdout}");
    assert!(stdout.contains("\"tag\":\"Err\""), "{stdout}");
    assert!(stdout.contains("\"tag\":\"EmptyCart\""), "{stdout}");
    assert_golden("cart.js", &cart.js);
    assert_golden("cart.js.map", &cart.map);
}

#[test]
fn money_js_runs_example_in_node() {
    let out = emit_fixture("money").expect("emit money");
    let dir = temp_dir("money");
    write_out(&dir, "money", &out);
    let r = run_node(&dir, &["money.js"]);
    let stdout = String::from_utf8_lossy(&r.stdout);
    assert_eq!(
        r.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    assert!(stdout.contains("example 0: ok"), "{stdout}");
    assert!(stdout.contains("\"tag\":\"Ok\""), "{stdout}");
    assert!(stdout.contains("\"amount\":3"), "{stdout}");
    assert!(stdout.contains("\"currency\":\"JPY\""), "{stdout}");
}

#[test]
fn cart_js_runs_empty_cart_err_in_node() {
    let money = emit_fixture("money").expect("emit money");
    let cart = emit_fixture("cart").expect("emit cart");
    let dir = temp_dir("cart");
    write_out(&dir, "money", &money);
    write_out(&dir, "cart", &cart);
    let r = run_node(&dir, &["cart.js"]);
    let stdout = String::from_utf8_lossy(&r.stdout);
    assert_eq!(
        r.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    assert!(stdout.contains("example 0: ok"), "{stdout}");
    assert!(stdout.contains("\"tag\":\"Err\""), "{stdout}");
    assert!(stdout.contains("\"tag\":\"EmptyCart\""), "{stdout}");
}

    #[test]
    fn defn_bang_module_emits_async_and_runs_in_node() {
        let src = "(effect log : (fn (s : string) : nil))\n(effect db : (fn (op : string) (key : string) : (Option string)))\n(type Host (obj (log : (fn (s : string) : nil)) (db : (fn (op : string) (key : string) : (Option string)))))\n(defn! run (host : Host) (x : number) : (Result number string) (do [_ (log! (str x))] [v (db! \"get\" (str x))] (match v [nil (ok x)] [s (ok (+ x 1))])))\n(example (run (obj (log (fn (s : string) : nil nil)) (db (fn (op : string) (key : string) : (Option string) nil))) 1) => (ok 1))\n(example (run (obj (log (fn (s : string) : nil nil)) (db (fn (op : string) (key : string) : (Option string) \"v\"))) 1) => (ok 2))";
        let out = emit_src(src, "effect-run.js").expect("defn! は P1 で emit できる");
        assert!(out.js.contains("async function run(host, x)"), "{}", out.js);
        assert!(out.js.contains("await host.log("), "{}", out.js);
        assert!(out.js.contains("const actual = (await run("), "{}", out.js);
        let dir = temp_dir("defn-bang");
        write_out(&dir, "effect-run", &out);
        let r = run_node(&dir, &["effect-run.js"]);
        let stdout = String::from_utf8_lossy(&r.stdout);
        assert_eq!(
            r.status.code(),
            Some(0),
            "stderr: {}",
            String::from_utf8_lossy(&r.stderr)
        );
        assert!(stdout.contains("example 0: ok"), "{stdout}");
        assert!(stdout.contains("example 1: ok"), "{stdout}");
    }

#[test]
fn t8_deep_recursion_stack_trace_points_to_lts_line() {
    let out = emit_fixture("fact").expect("emit fact");
    let dir = temp_dir("t8");
    write_out(&dir, "fact", &out);
    std::fs::write(dir.join("run.js"), "import { fact } from \"./fact.js\";\nfact(100000);\n")
        .expect("write run.js");
    let r = run_node(&dir, &["--enable-source-maps", "run.js"]);
    let stderr = String::from_utf8_lossy(&r.stderr);
    assert_ne!(r.status.code(), Some(0), "スタックオーバーフローで失敗するはず: {stderr}");
    // M14: JSエンジンのエラーメッセージ文言（"Maximum call stack size exceeded" 等）には依存しない。
    // 検査対象は「スタックトレースが fact.lts の行を指すこと」のみ。
    let hit = stderr.find("fact.lts:").map(|i| {
        stderr[i..]
            .chars()
            .nth("fact.lts:".len())
            .is_some_and(|c| c.is_ascii_digit())
    });
    assert_eq!(hit, Some(true), "トレースに fact.lts:<行> が必要: {stderr}");
}

#[test]
fn tail_self_recursion_is_constant_stack_in_node() {
    let out = emit_fixture("tail-self").expect("emit tail-self");
    let dir = temp_dir("tail-self");
    write_out(&dir, "tail-self", &out);
    std::fs::write(
        dir.join("run.js"),
        "import { count_down } from \"./tail-self.js\";\nconst r = count_down(1000000, 0);\nif (r !== 1000000) throw new Error(\"unexpected: \" + r);\nconsole.log(\"count_down(1000000) = \" + r);\n",
    )
    .expect("write run.js");
    let r = run_node(&dir, &["run.js"]);
    assert_eq!(
        r.status.code(),
        Some(0),
        "10^6 の末尾自己再帰がスタックオーバーフローした: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    let stdout = String::from_utf8_lossy(&r.stdout);
    assert!(stdout.contains("count_down(1000000) = 1000000"), "{stdout}");
}

#[test]
fn tail_mutual_recursion_is_constant_stack_in_node() {
    let out = emit_fixture("tail-mutual").expect("emit tail-mutual");
    let dir = temp_dir("tail-mutual");
    write_out(&dir, "tail-mutual", &out);
    std::fs::write(
        dir.join("run.js"),
        "import { is_even } from \"./tail-mutual.js\";\nconst r = is_even(1000000);\nif (r !== true) throw new Error(\"unexpected: \" + r);\nconsole.log(\"is_even(1000000) = \" + r);\n",
    )
    .expect("write run.js");
    let r = run_node(&dir, &["run.js"]);
    assert_eq!(
        r.status.code(),
        Some(0),
        "10^6 の相互再帰がスタックオーバーフローした: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    let stdout = String::from_utf8_lossy(&r.stdout);
    assert!(stdout.contains("is_even(1000000) = true"), "{stdout}");
}


// M14: 生成JSの文字列表現への依存を排し、Backend トレイト経由の挙動検査に置き換える。
#[test]
fn tail_call_codegen_behaves_via_backend() {
    let backend = lists::backends::lookup("js").expect("js backend");
    for (fixture, expect_line) in [
        ("tail-self", "count_down(1000000) = 1000000"),
        ("tail-mutual", "is_even(1000000) = true"),
    ] {
        let dir = temp_dir(&format!("tail-behavior-{fixture}"));
        let sources = vec![lists::backends::SourceFile {
            path: PathBuf::from(fixture_path(&format!("{fixture}.lts"))),
            name: format!("{fixture}.lts"),
        }];
        let artifacts = backend.compile(&sources, &dir, lists::rng::DEFAULT_SEED).expect("compile");
        let driver = match fixture {
            "tail-self" => "import { count_down } from \"./tail-self.js\";\nconsole.log(\"count_down(1000000) = \" + count_down(1000000, 0));\n",
            _ => "import { is_even } from \"./tail-mutual.js\";\nconsole.log(\"is_even(1000000) = \" + is_even(1000000));\n",
        };
        std::fs::write(dir.join("driver.js"), driver).expect("write driver");
        let out = lists::backends::run_node_entry(&dir, "driver.js", &[]).expect("node");
        assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains(expect_line), "{stdout}");
        let _ = artifacts;
    }
}

// M14 作業2: runtime.js の ok/err/structuralEqual が付録CのRust型定義と一致するかを、
// JS実行結果とRust実装の突き合わせで検証する（空obj・ネスト・数値比較を含むエッジケース）。
#[test]
fn runtime_primitives_match_appendix_c_across_backends() {
    let dir = temp_dir("runtime-prims");
    std::fs::write(dir.join("runtime.js"), lists::runtime::RUNTIME_JS).expect("write runtime");
    let script = r#"
import { ok, err, structuralEqual } from "./runtime.js";
const cases = [
  ["empty-obj", {}, {}],
  ["empty-vs-nonempty", {}, { a: 1 }],
  ["nested", [[1, { a: [true] }]], [[1, { a: [true] }]]],
  ["nested-diff", [[1, { a: [true] }]], [[1, { a: [false] }]]],
  ["nan", NaN, NaN],
  ["minus-zero", -0, 0],
  ["num", 1, 1.0],
  ["ok-ok", ok(1), ok(1)],
  ["ok-err", ok(1), err(1)],
  ["nil-nil", null, null],
  ["nil-obj", null, {}],
  ["str", "a", "a"],
];
for (const [name, a, b] of cases) console.log(name + " " + JSON.stringify(structuralEqual(a, b)));
console.log("ok-shape " + JSON.stringify(ok(1)));
console.log("err-shape " + JSON.stringify(err("e")));
"#;
    std::fs::write(dir.join("check.js"), script).expect("write check.js");
    let out = run_node(&dir, &["check.js"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let got: std::collections::BTreeMap<String, String> = stdout
        .lines()
        .filter_map(|l| l.split_once(' '))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    use lists::value::{structural_equal, Value};
    use std::rc::Rc;
    let obj = |pairs: Vec<(&str, Value)>| Value::Obj(pairs.into_iter().map(|(k, v)| (Rc::from(k), v)).collect());
    let num = |n: f64| Value::Num(n);
    let se = |a: &Value, b: &Value| structural_equal(a, b).unwrap_or(false);
    let expected: Vec<(&str, bool)> = vec![
        ("empty-obj", se(&obj(vec![]), &obj(vec![]))),
        ("empty-vs-nonempty", se(&obj(vec![]), &obj(vec![("a", num(1.0))]))),
        (
            "nested",
            se(
                &Value::Array(vec![num(1.0), obj(vec![("a", Value::Array(vec![Value::Bool(true)]))])]),
                &Value::Array(vec![num(1.0), obj(vec![("a", Value::Array(vec![Value::Bool(true)]))])]),
            ),
        ),
        (
            "nested-diff",
            se(
                &Value::Array(vec![num(1.0), obj(vec![("a", Value::Array(vec![Value::Bool(true)]))])]),
                &Value::Array(vec![num(1.0), obj(vec![("a", Value::Array(vec![Value::Bool(false)]))])]),
            ),
        ),
        ("nan", se(&num(f64::NAN), &num(f64::NAN))),
        ("minus-zero", se(&num(-0.0), &num(0.0))),
        ("num", se(&num(1.0), &num(1.0))),
        ("ok-ok", se(&Value::Ok(Box::new(num(1.0))), &Value::Ok(Box::new(num(1.0))))),
        ("ok-err", se(&Value::Ok(Box::new(num(1.0))), &Value::Err(Box::new(num(1.0))))),
        ("nil-nil", se(&Value::Nil, &Value::Nil)),
        ("nil-obj", se(&Value::Nil, &obj(vec![]))),
        ("str", se(&Value::Str(Rc::from("a")), &Value::Str(Rc::from("a")))),
    ];
    for (name, want) in expected {
        let js = got.get(name).unwrap_or_else(|| panic!("missing {name} in {stdout}"));
        assert_eq!(js, &want.to_string(), "{name}: JS={js} Rust={want}");
    }
    // ok/err の形状は §12.5/付録C#6 の {tag, value} MUST
    assert_eq!(got.get("ok-shape").map(String::as_str), Some("{\"tag\":\"Ok\",\"value\":1}"));
    assert_eq!(got.get("err-shape").map(String::as_str), Some("{\"tag\":\"Err\",\"value\":\"e\"}"));
}

#[test]
fn source_map_covers_loop_converted_tail_call() {
    let out = emit_fixture("tail-self").expect("emit tail-self");
    let map = sourcemap::SourceMap::from_slice(out.map.as_bytes()).expect("parse map");
    let js_lines: Vec<&str> = out.js.lines().collect();
    let arg_line = js_lines
        .iter()
        .position(|l| l.contains("const $a1 ="))
        .expect("arg temp line");
    let mut found = false;
    for token in map.tokens() {
        if token.get_dst_line() as usize == arg_line {
            assert_eq!(token.get_src_line(), 4, "tail call は lts 5行目(0始まりで4)に対応するはず");
            found = true;
        }
    }
    assert!(found, "ループ変換された引数評価行に source map マッピングが必要: {}", out.map);

    let mut_out = emit_fixture("tail-mutual").expect("emit tail-mutual");
    let mmap = sourcemap::SourceMap::from_slice(mut_out.map.as_bytes()).expect("parse map");
    let thunk_line = mut_out.js.lines().position(|l| l.contains("return $thunk(")).expect("thunk line");
    let mut found = false;
    for token in mmap.tokens() {
        if token.get_dst_line() as usize == thunk_line {
            found = true;
        }
    }
    assert!(found, "トランポリンの thunk 行に source map マッピングが必要");
}

// M16（v0.3 事前確定 #2）: runtime.js の structuralEqual は循環構造を検出して実行時エラー（throw）にする（SPEC §8.3/§8.6）。
#[test]
fn structural_equal_throws_on_cyclic_structures() {
    let dir = temp_dir("cyclic-eq");
    std::fs::write(dir.join("runtime.js"), lists::runtime::RUNTIME_JS).expect("write runtime");
    let script = r#"
import { structuralEqual } from "./runtime.js";
const a = [1];
a.push(a);
const o = { x: 1 };
o.self = o;
function expectThrow(name, f) {
  try {
    f();
    console.log(name + " NO-THROW");
  } catch (e) {
    console.log(name + " threw: " + (e && e.message ? e.message : String(e)));
  }
}
expectThrow("cyclic-array", () => structuralEqual(a, a));
expectThrow("cyclic-obj", () => structuralEqual(o, o));
const x = {};
const y = { x };
x.y = y;
expectThrow("cyclic-indirect", () => structuralEqual(x, x));
const c = [9];
console.log("acyclic " + structuralEqual([1, [2]], [1, [2]]));
console.log("dag " + structuralEqual([c, c], [[9], [9]]));
console.log("fn " + structuralEqual(() => 1, () => 1));
"#;
    std::fs::write(dir.join("check.js"), script).expect("write check.js");
    let out = run_node(&dir, &["check.js"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("cyclic-array threw:"), "{stdout}");
    assert!(stdout.contains("cyclic-obj threw:"), "{stdout}");
    assert!(stdout.contains("cyclic-indirect threw:"), "{stdout}");
    assert!(stdout.contains("cyclic structure"), "{stdout}");
    assert!(!stdout.contains("NO-THROW"), "{stdout}");
    assert!(stdout.contains("acyclic true"), "{stdout}");
    assert!(stdout.contains("dag true"), "{stdout}");
    assert!(stdout.contains("fn false"), "{stdout}");
}

// M16（v0.3 事前確定 #4）: stdlib-edge.lts の生成JSは決定論的で、§11.6 のローカル補助関数（$print/$slice_bounds）を含む。
// runtime.js の export は ok/err/structuralEqual の3つのまま（§12.5 MUST）。
#[test]
fn stdlib_edge_emit_is_deterministic_and_uses_local_helpers() {
    let r1 = emit_fixture("stdlib-edge").expect("emit stdlib-edge");
    let r2 = emit_fixture("stdlib-edge").expect("emit stdlib-edge again");
    assert_eq!(r1.js, r2.js, "生成JSは決定論的であること");
    assert_eq!(r1.map, r2.map, "source map も決定論的であること");
    assert!(r1.js.contains("function $print("), "{}", r1.js);
    assert!(r1.js.contains("function $print_num("), "{}", r1.js);
    assert!(r1.js.contains("function $slice_bounds("), "{}", r1.js);
    assert!(r1.js.contains("const $stdlib_str = (a) => $print(a);"), "{}", r1.js);
    assert!(r1.js.contains("const $stdlib_len = (s) => [...s].length;"), "{}", r1.js);
    assert!(!r1.js.contains("$stdlib_split"), "{}", r1.js);
}

#[test]
fn effect_run_js_golden_and_source_map_kept() {
    let out = emit_fixture("effect-run").expect("emit effect-run");
    let again = emit_fixture("effect-run").expect("emit again");
    assert_eq!(out.js, again.js, "defn! を含む生成 JS も決定的であること");
    let map_json: serde_json::Value = serde_json::from_str(&out.map).expect("map json");
    assert_eq!(map_json["version"], 3);
    assert_eq!(map_json["sources"], serde_json::json!(["effect-run.lts"]));
    assert!(!map_json["mappings"].as_str().expect("mappings").is_empty());
    assert!(out.js.contains("await host.log("), "{}", out.js);
    assert_golden("effect-run.js", &out.js);
    assert_golden("effect-run.js.map", &out.map);
}

#[test]
fn mock_host_call_log_diff_interp_vs_node() {
    // M21: モック host の呼び出し記録が interp と JS（node）で一致することを検証する。
    // JS 側は driver がモック host を注入して run を呼び、記録を印字する。
    let out = emit_fixture("effect-mock").expect("emit effect-mock");
    let dir = temp_dir("effect-mock");
    write_out(&dir, "effect-mock", &out);
    let driver = r#"import { run } from "./effect-mock.js";
const calls = [];
const host = {
  log: async (s) => { calls.push(["log", s]); return null; },
  db: async (op, key) => { calls.push(["db", op, key]); return null; },
};
const r = await run(host, 1);
console.log("result " + JSON.stringify(r));
console.log("calls " + JSON.stringify(calls));
"#;
    std::fs::write(dir.join("run-mock.js"), driver).expect("write driver");
    let run1 = run_node(&dir, &["run-mock.js"]);
    let run2 = run_node(&dir, &["run-mock.js"]);
    let stdout = String::from_utf8_lossy(&run1.stdout).into_owned();
    assert_eq!(run1.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&run1.stderr));
    assert_eq!(stdout, String::from_utf8_lossy(&run2.stdout).into_owned(), "JS 側も決定的");
    let result_line = stdout.lines().find(|l| l.starts_with("result ")).expect("result line");
    let calls_line = stdout.lines().find(|l| l.starts_with("calls ")).expect("calls line");
    let js_result: serde_json::Value = serde_json::from_str(result_line.strip_prefix("result ").expect("p")).expect("json");
    let js_calls: serde_json::Value = serde_json::from_str(calls_line.strip_prefix("calls ").expect("p")).expect("json");

    // interp 側: 同じ fixture をモック host（host.rs）で実行し、結果と記録を比較する
    let abs = fixture_path("effect-mock.lts");
    let (defs, diags) = pipeline(&abs);
    assert!(diags.is_empty(), "{diags:?}");
    let mut cache = ModuleCache::default();
    let checked = check_file(&abs, &mut cache);
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let mock = MockHost::from_effects(&checked.effects, &checked.type_aliases);
    let env = global_env();
    load_defs(&defs, &env);
    inject_host(&env, "host", &mock.obj());
    let (expr, ediags) = parse_expr("(run host 1)", "t.lts");
    assert!(ediags.is_empty(), "{ediags:?}");
    let core = desugar_expr(&expr.expect("expr"));
    let mut interp = Interp::new();
    let result = eval_core(&mut interp, &core, &env).expect("interp eval");
    let interp_result = lists::backends::value_to_json(&result);
    let interp_calls = serde_json::Value::Array(
        mock.calls()
            .iter()
            .map(|c| {
                let mut a = vec![serde_json::Value::String(c.effect.to_string())];
                a.extend(c.args.iter().map(lists::backends::value_to_json));
                serde_json::Value::Array(a)
            })
            .collect(),
    );
    fn json_eq_numbers_as_f64(a: &serde_json::Value, b: &serde_json::Value) -> bool {
        match (a, b) {
            (serde_json::Value::Number(x), serde_json::Value::Number(y)) => {
                x.as_f64() == y.as_f64()
            }
            (serde_json::Value::Object(x), serde_json::Value::Object(y)) => {
                x.len() == y.len()
                    && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| json_eq_numbers_as_f64(v, w)))
            }
            (serde_json::Value::Array(x), serde_json::Value::Array(y)) => {
                x.len() == y.len() && x.iter().zip(y.iter()).all(|(u, v)| json_eq_numbers_as_f64(u, v))
            }
            _ => a == b,
        }
    }
    assert!(json_eq_numbers_as_f64(&interp_result, &js_result), "結果が一致: interp vs node: {interp_result} vs {js_result}");
    assert_eq!(interp_calls, js_calls, "モック host の呼び出し記録が一致: interp vs node");
    assert_eq!(
        js_calls,
        serde_json::json!([["log", "run:1"], ["db", "get", "1"]]),
        "記録内容の確認: {js_calls}"
    );
}

#[test]
fn decimal_edge_js_golden_and_error_parity_with_interp() {
    // golden 追記（生成 JS の BigInt lowering を固定）
    let out = emit_fixture("decimal-edge").expect("emit decimal-edge");
    assert!(out.js.contains("1500000000000000000n"), "{}", out.js);
    assert!(out.js.contains("$dec_mul"), "{}", out.js);
    assert!(out.js.contains("$json(actual)"), "{}", out.js);
    assert_golden("decimal-edge.js", &out.js);
    assert_golden("decimal-edge.js.map", &out.map);

    // 実行時エラーの機械可読な一致: 0 除算とオーバーフローで interp と node が同じ文字列を出す
    let src = "(export dv ov)\n(defn dv (a : decimal) (b : decimal) : decimal (/ a b))\n(defn ov (a : decimal) (b : decimal) : decimal (* a b))";
    let out2 = emit_src(src, "dec-err.js").expect("emit");
    let dir = temp_dir("dec-err");
    write_out(&dir, "dec-err", &out2);
    let driver = r#"import { dv, ov } from "./dec-err.js";
const tryCall = (f, ...args) => { try { return "ok " + String(f(...args)); } catch (e) { return "err " + e.message; } };
console.log(tryCall(dv, 1n * 10n ** 18n, 0n));
console.log(tryCall(ov, 100000000000n * 10n ** 18n, 100000000000n * 10n ** 18n));
"#;
    std::fs::write(dir.join("run.js"), driver).expect("write driver");
    let r = run_node(&dir, &["run.js"]);
    let stdout = String::from_utf8_lossy(&r.stdout).into_owned();
    assert_eq!(r.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&r.stderr));

    // interp 側
    let lex = lists::lexer::tokenize(src, "t.lts");
    let (program, _) = lists::parser::parse_program(&lex);
    let (defs, _) = lists::desugar::desugar(&program);
    let env = lists::eval::global_env();
    lists::eval::load_defs(&defs, &env);
    let mut interp = lists::eval::Interp::new();
    let dv = env.lookup("dv").expect("dv");
    let ov = env.lookup("ov").expect("ov");
    let s = lists::value::Value::Dec(lists::decimal::S);
    let zero = lists::value::Value::Dec(0);
    let big = lists::value::Value::Dec(100_000_000_000 * lists::decimal::S);
    let e1 = interp.apply(dv, vec![s.clone(), zero]).expect_err("div by zero must error");
    let e2 = interp.apply(ov, vec![big.clone(), big]).expect_err("overflow must error");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], format!("err {}", e1.to_string().replace("runtime error: ", "")), "JS: {stdout}");
    assert_eq!(lines[1], format!("err {}", e2.to_string().replace("runtime error: ", "")), "JS: {stdout}");
    assert!(lines[0].contains("decimal-division-by-zero"), "{stdout}");
    assert!(lines[1].contains("decimal-overflow"), "{stdout}");
}
