use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn fixture_path(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn lists_bin() -> &'static str {
    env!("CARGO_BIN_EXE_lists")
}

fn run_lists(args: &[&str], stdin_data: Option<&str>) -> Output {
    let mut cmd = Command::new(lists_bin());
    cmd.args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match stdin_data {
        Some(_) => {
            cmd.stdin(Stdio::piped());
        }
        None => {
            cmd.stdin(Stdio::null());
        }
    }
    let mut child = cmd.spawn().expect("lists を起動できること");
    if let Some(input) = stdin_data {
        child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("write stdin");
    }
    child.wait_with_output().expect("wait")
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lists-m9-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn assert_golden_json(label: &str, actual: serde_json::Value) {
    let path = format!("{}/tests/golden/{label}", env!("CARGO_MANIFEST_DIR"));
    if std::env::var("UPDATE_SNAPSHOTS").is_ok() {
        let pretty = serde_json::to_string_pretty(&actual).expect("pretty");
        std::fs::write(&path, format!("{pretty}\n")).expect("write golden");
    }
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("golden がありません: {path} (UPDATE_SNAPSHOTS=1 cargo test で生成)"));
    let expected: serde_json::Value = serde_json::from_str(&content).expect("golden json");
    assert_eq!(actual, expected, "{path}");
}

#[test]
fn repl_json_lines_golden() {
    let cart = fixture_path("cart.lts");
    let input = format!(
        "{{\"id\": 1, \"op\": \"eval\", \"src\": \"(defn add (a : number) (b : number) : number (+ a b))\"}}\n\
         {{\"id\": 2, \"op\": \"test\"}}\n\
         {{\"id\": 3, \"op\": \"prop\", \"name\": \"add-money commutative\"}}\n\
         {{\"id\": 4, \"op\": \"reload\", \"file\": {}}}\n\
         {{\"id\": 5, \"op\": \"type-of\", \"src\": \"add\"}}\n\
         {{\"id\": 6, \"op\": \"type-of\", \"src\": \"place-order\"}}\n\
         {{\"id\": 7, \"op\": \"test\"}}\n\
         {{\"id\": 8, \"op\": \"eval\", \"src\": \"(defn f () : number (ok \\\"a\\\"))\"}}\n\
         {{\"id\": 9, \"op\": \"quit\"}}\n",
        serde_json::to_string(&cart).expect("json")
    );
    let out = run_lists(&["repl"], Some(&input));
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let responses: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("1行1JSON"))
        .collect();
    assert_eq!(responses.len(), 8, "quit は応答しない: {stdout}");

    assert_eq!(
        responses[0],
        serde_json::json!({"id": 1, "ok": true, "defined": "add", "type": "(fn (number number) : number)"})
    );
    assert_eq!(
        responses[1],
        serde_json::json!({"id": 2, "ok": true, "results": []})
    );
    // M22: prop op は実評価へ移行したので、この時点では prop が未定義のため unbound-symbol
    assert_eq!(
        responses[2]["errors"][0]["kind"],
        serde_json::json!("unbound-symbol")
    );
    assert_eq!(responses[2]["ok"], serde_json::json!(false));
    assert_eq!(responses[3], serde_json::json!({"id": 4, "ok": true}));
    assert_eq!(
        responses[4],
        serde_json::json!({"id": 5, "ok": true, "type": "(fn (number number) : number)"})
    );
    assert!(responses[5]["type"].as_str().expect("type").contains("(Result"));
    assert_eq!(
        responses[6],
        serde_json::json!({"id": 7, "ok": true, "results": [{"example": "example 0", "pass": true}]})
    );
    assert_eq!(responses[7]["ok"], serde_json::json!(false));
    assert_eq!(responses[7]["errors"][0]["kind"], serde_json::json!("type-mismatch"));

    assert_golden_json("repl.responses.json", serde_json::Value::Array(responses));
}

#[test]
// compile --seed は prop のシードを codegen 出力へ $SEED として埋め込む（REPL --seed と同じ既定値）
#[test]
fn cli_compile_seed_is_embedded_into_codegen() {
    let dir = temp_dir("compile-seed");
    let props = fixture_path("props-edge.lts");
    let out_dir = dir.join("out");
    let out = run_lists(
        &[
            "compile",
            &props,
            "--out",
            out_dir.to_str().expect("utf8"),
            "--seed",
            "42",
        ],
        None,
    );
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let js = std::fs::read_to_string(out_dir.join("props-edge.js")).expect("js");
    assert!(js.contains("const $SEED = 42n;"), "{js}");
    assert!(js.contains("prop-runtime.js"), "{js}");
    assert!(out_dir.join("prop-runtime.js").exists(), "prop-runtime.js が配置される");
    // 既定（--seed なし）は DEFAULT_SEED
    let dir2 = temp_dir("compile-seed-default");
    let out_dir2 = dir2.join("out");
    let out2 = run_lists(&["compile", &props, "--out", out_dir2.to_str().expect("utf8")], None);
    assert_eq!(out2.status.code(), Some(0));
    let js2 = std::fs::read_to_string(out_dir2.join("props-edge.js")).expect("js");
    let default_seed = lists::rng::DEFAULT_SEED;
    assert!(js2.contains(&format!("const $SEED = {default_seed}n;")), "{js2}");
}

fn cli_compile_money_cart_out_dir_and_node_run() {
    let dir = temp_dir("compile");
    let money = fixture_path("money.lts");
    let cart = fixture_path("cart.lts");
    let out_dir = dir.join("out");
    let out = run_lists(
        &["compile", &money, &cart, "--out", out_dir.to_str().expect("utf8")],
        None,
    );
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let diags: serde_json::Value = serde_json::from_str(stdout.trim()).expect("stdout は Diagnostic[] の JSON");
    assert_eq!(diags, serde_json::json!([]), "{stdout}");
    for f in ["money.js", "money.js.map", "cart.js", "cart.js.map", "runtime.js"] {
        assert!(out_dir.join(f).exists(), "{f} が出力されていること");
    }
    let run = Command::new("node")
        .arg("cart.js")
        .current_dir(&out_dir)
        .output()
        .expect("node を起動できること");
    let so = String::from_utf8_lossy(&run.stdout);
    assert_eq!(run.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&run.stderr));
    assert!(so.contains("example 0: ok"), "{so}");
}

#[test]
fn cli_compile_broken_exits_1_with_parse_error_json() {
    let dir = temp_dir("compile-bad");
    let broken = fixture_path("broken.lts");
    let out = run_lists(&["compile", &broken, "--out", dir.to_str().expect("utf8")], None);
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let diags: serde_json::Value = serde_json::from_str(stdout.trim()).expect("stdout は Diagnostic[] の JSON");
    let arr = diags.as_array().expect("array");
    assert!(arr.iter().any(|d| d["kind"] == "parse-error"), "{stdout}");
}

#[test]
fn cli_compile_camel_case_exits_1_with_name_convention_and_fix() {
    let dir = temp_dir("compile-name-convention");
    let target = dir.join("bad-name.lts");
    std::fs::write(&target, "(export badName)\n(defn badName (x : number) : number x)\n")
        .expect("write");
    let out = run_lists(
        &["compile", target.to_str().expect("utf8"), "--out", dir.to_str().expect("utf8")],
        None,
    );
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let diags: serde_json::Value = serde_json::from_str(stdout.trim()).expect("stdout is Diagnostic[] JSON");
    let arr = diags.as_array().expect("array");
    let nc: Vec<&serde_json::Value> = arr.iter().filter(|d| d["kind"] == "name-convention").collect();
    assert_eq!(nc.len(), 2, "{stdout}");
    assert!(nc.iter().all(|d| d["fix"]["template"] == "bad-name"), "{stdout}");
}

#[test]
fn cli_fmt_in_place_idempotent() {
    let dir = temp_dir("fmt");
    let target = dir.join("money.lts");
    std::fs::copy(fixture_path("money.lts"), &target).expect("copy");
    let original = std::fs::read_to_string(&target).expect("read");

    let out1 = run_lists(&["fmt", target.to_str().expect("utf8")], None);
    assert_eq!(out1.status.code(), Some(0), "{}", String::from_utf8_lossy(&out1.stderr));
    assert_eq!(String::from_utf8_lossy(&out1.stdout).trim(), "[]");
    let once = std::fs::read_to_string(&target).expect("read");
    assert_ne!(once, original, "fmt は in-place で書き換える");

    let out2 = run_lists(&["fmt", target.to_str().expect("utf8")], None);
    assert_eq!(out2.status.code(), Some(0));
    assert_eq!(std::fs::read_to_string(&target).expect("read"), once, "fmt は冪等");

    let golden = format!("{}/tests/golden/money.fmt.lts", env!("CARGO_MANIFEST_DIR"));
    if std::env::var("UPDATE_SNAPSHOTS").is_ok() {
        std::fs::write(&golden, &once).expect("write golden");
    }
    let expected = std::fs::read_to_string(&golden).expect("golden (UPDATE_SNAPSHOTS=1 で生成)");
    assert_eq!(once, expected);
}

#[test]
fn cli_unknown_subcommand_is_usage_error() {
    let out = run_lists(&["nope"], None);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn cli_compile_backend_flag_js_default_and_unknown() {
    let dir = temp_dir("backend-flag");
    let money = fixture_path("money.lts");
    let out_dir = dir.join("out");
    // デフォルト（--backend 省略）と --backend js は同じ挙動
    let out = run_lists(
        &["compile", &money, "--out", out_dir.to_str().expect("utf8"), "--backend", "js"],
        None,
    );
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "[]");
    for f in ["money.js", "money.js.map", "runtime.js"] {
        assert!(out_dir.join(f).exists(), "{f} が出力されていること");
    }
    // 未知のバックエンド名はエラー（exit 2 + 診断JSON）
    let out = run_lists(&["compile", &money, "--backend", "wasm"], None);
    assert_eq!(out.status.code(), Some(2));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("unknown backend"), "{stdout}");
}

#[test]
fn diagnostics_contain_no_js_engine_text() {
    // M14 作業4: 診断JSON（kind/message/data）にJSエンジン由来の文言が混入していないことを全フィクスチャで検査する
    let markers = [
        "RangeError",
        "TypeError",
        "SyntaxError",
        "Maximum call stack",
        "is not defined",
        "undefined",
        "prototype",
    ];
    for name in [
        "money.lts",
        "cart.lts",
        "stock.lts",
        "effects.lts",
        "broken.lts",
        "cycle-a.lts",
        "mod-missing.lts",
        "tail-self.lts",
        "tail-mutual.lts",
        "stdlib-edge.lts",
    ] {
        let abs = fixture_path(name);
        let src = std::fs::read_to_string(&abs).expect("read fixture");
        let lex = lists::lexer::tokenize(&src, &abs);
        let (program, mut diags) = lists::parser::parse_program(&lex);
        diags.extend(lex.diagnostics.iter().cloned());
        let (_, ds) = lists::desugar::desugar(&program);
        diags.extend(ds);
        let mut cache = lists::check::ModuleCache::default();
        let checked = lists::check::check_file(&abs, &mut cache);
        diags.extend(checked.diagnostics.iter().cloned());
        let json = serde_json::to_string(&diags).expect("serialize diagnostics");
        for m in markers {
            assert!(
                !json.contains(m),
                "{name}:診断JSONにJS由来の文言 '{m}' が混入しています: {json}"
            );
        }
    }
}

#[test]
fn repl_host_mock_injection_records_effect_calls() {
    // host DI: `lists repl --host mock` でモック host が `host` 全局に注入され、
    // example から defn! 経由でエフェクトが呼ばれると host-log に記録される。
    let input = "{\"id\": 1, \"op\": \"eval\", \"src\": \"(effect log : (fn (s : string) : nil))\\n(defn! f (host : (obj (log : (fn (s : string) : nil)))) (s : string) : nil (log! s))\"}\n         {\"id\": 2, \"op\": \"eval\", \"src\": \"(example (f host \\\"hi\\\") => nil)\"}\n         {\"id\": 3, \"op\": \"test\"}\n         {\"id\": 4, \"op\": \"host-log\"}\n         {\"id\": 5, \"op\": \"quit\"}\n";
    let out = run_lists(&["repl", "--host", "mock"], Some(input));
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let responses: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("1行1JSON"))
        .collect();
    assert_eq!(responses.len(), 4, "quit は応答しない: {stdout}");
    assert_eq!(responses[0]["ok"], true, "effect+defn! の eval: {responses:?}");
    assert_eq!(responses[1]["ok"], true, "host を使う example の eval: {responses:?}");
    assert_eq!(
        responses[2],
        serde_json::json!({"id": 3, "ok": true, "results": [{"example": "example 0", "pass": true}]}),
        "注入されたモック host 経由で example が通る: {responses:?}"
    );
    let hi_arg = "hi".to_string(); // print_value は文字列を引用符なしで印字（§11.6）
    assert_eq!(
        responses[3],
        serde_json::json!({
            "id": 4, "ok": true, "host": "mock",
            "calls": [{"effect": "log", "args": [hi_arg]}]
        }),
        "モック host の呼び出し記録: {responses:?}"
    );
}

#[test]
fn repl_host_option_rejects_unknown_name() {
    let out = run_lists(&["repl", "--host", "bogus"], None);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown --host"), "{stderr}");
}

#[test]
fn repl_without_host_has_no_host_binding() {
    let input = "{\"id\": 1, \"op\": \"host-log\"}\n{\"id\": 2, \"op\": \"quit\"}\n";
    let out = run_lists(&["repl"], Some(input));
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"host\":\"none\"") && stdout.contains("\"calls\":[]"),
        "{stdout}"
    );
}

#[test]
fn repl_defn_bang_example_runs_with_explicit_host_literal() {
    // M21: --host なしでも、example が host リテラルを defn! 第1引数に明示渡しすれば
    // eval/test で defn! の example が動く（host-log は none/空）
    let input = concat!(
        r#"{"id": 1, "op": "eval", "src": "(effect log : (fn (s : string) : nil))\n(defn! f (host : (obj (log : (fn (s : string) : nil)))) (s : string) : nil (log! s))\n(example (f (obj (log (fn (s : string) : nil nil))) \"hi\") => nil)"}"#, "\n",
        r#"{"id": 2, "op": "test"}"#, "\n",
        r#"{"id": 3, "op": "host-log"}"#, "\n",
        r#"{"id": 4, "op": "quit"}"#, "\n",
    );
    let out = run_lists(&["repl"], Some(input));
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let responses: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("1行1JSON"))
        .collect();
    assert_eq!(responses.len(), 3, "quit は応答しない: {stdout}");
    assert_eq!(responses[0]["ok"], true, "eval: {responses:?}");
    assert_eq!(
        responses[1],
        serde_json::json!({"id": 2, "ok": true, "results": [{"example": "example 0", "pass": true}]}),
        "defn! の example が test で通る: {responses:?}"
    );
    assert_eq!(
        responses[2],
        serde_json::json!({"id": 3, "ok": true, "host": "none", "calls": []}),
        "host 未注入なら記録なし: {responses:?}"
    );
}

fn repl_responses(out: &std::process::Output) -> Vec<serde_json::Value> {
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("1行1JSON"))
        .collect()
}

#[test]
fn repl_prop_money_lts_passes_with_seed() {
    // M22: money.lts の prop（従来は受理のみ）が実評価で通る
    let input = concat!(
        r#"{"id": 1, "op": "reload", "file": "tests/fixtures/money.lts"}"#, "\n",
        r#"{"id": 2, "op": "prop", "name": "add-money commutative", "seed": 42}"#, "\n",
        r#"{"id": 3, "op": "quit"}"#, "\n",
    );
    let out = run_lists(&["repl", "--seed", "42"], Some(input));
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let rs = repl_responses(&out);
    assert_eq!(rs[0]["ok"], true, "{rs:?}");
    assert_eq!(
        rs[1],
        serde_json::json!({"id": 2, "ok": true, "prop": "add-money commutative", "seed": 42, "trials": 100}),
        "prop が 100試行・seed=42 で pass: {rs:?}"
    );
}

#[test]
fn repl_prop_false_prop_reports_counterexample_shrink_seed() {
    // 偽 prop: 反例・縮小後の値・施行回数・シードが機械可読 JSON で返る
    let input = concat!(
        r#"{"id": 1, "op": "eval", "src": "(prop \"neg\" (forall [(x : number)] (< x 0)))"}"#, "\n",
        r#"{"id": 2, "op": "prop", "name": "neg", "seed": 7}"#, "\n",
        r#"{"id": 3, "op": "quit"}"#, "\n",
    );
    let out = run_lists(&["repl"], Some(input));
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let rs = repl_responses(&out);
    assert_eq!(rs[0]["ok"], true, "{rs:?}");
    let p = &rs[1];
    assert_eq!(p["ok"], false);
    assert_eq!(p["prop"], "neg");
    assert_eq!(p["seed"], 7);
    assert!(p["trials"].as_u64().is_some_and(|t| (1..=100).contains(&t)), "{p}");
    assert!(p["counterexample"]["x"].is_number(), "反例が機械可読: {p}");
    assert_eq!(p["shrunk"]["x"], serde_json::json!(0.0), "number は 0 へ縮小: {p}");
    let tmpl = p["fix"]["template"].as_str().expect("fix template");
    assert!(tmpl.starts_with("(example (< 0 0)") && tmpl.ends_with("=> false)"), "貼れる example 形状: {tmpl}");
    // 決定性: 同一シードで同一応答
    let out2 = run_lists(&["repl"], Some(input));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out2.stdout),
        "同じシードなら同じ反例・同じ応答（決定性 MUST）"
    );
}

#[test]
fn repl_prop_unknown_name_is_error_and_bad_seed_exits_2() {
    let input = concat!(
        r#"{"id": 1, "op": "prop", "name": "nope"}"#, "\n",
        r#"{"id": 2, "op": "quit"}"#, "\n",
    );
    let out = run_lists(&["repl"], Some(input));
    let rs = repl_responses(&out);
    assert_eq!(rs[0]["ok"], false);
    assert_eq!(rs[0]["errors"][0]["kind"], "unbound-symbol");
    let bad = run_lists(&["repl", "--seed", "notanumber"], None);
    assert_eq!(bad.status.code(), Some(2), "不正な --seed は exit 2");
}

#[test]
fn repl_host_log_matches_documented_shape_and_is_deterministic() {
    // SPEC §10.1（v0.3 追記）: host-log 応答は {"id","ok":true,"host":"mock","calls":[{"effect","args":[...]}]}
    // と一致する。args は §11.6 の REPL 印字規約（print_value）による文字列化なので、
    // 数値 5 と文字列 "5" はともに "5" と記録される（文書化した既知の制約）。
    // 記録はセッション内で呼び出し順に蓄積され、同一入力に決定的（MUST）。
    let defn = r#"(effect log : (fn (s : string) : nil))
(effect tick : (fn (n : number) : nil))
(defn! h (host : (obj (log : (fn (s : string) : nil)) (tick : (fn (n : number) : nil)))) : (Result nil string) (do [_ (log! "5")] [_ (tick! 5)] (ok nil)))"#;
    let input = format!(
        "{{\"id\": 1, \"op\": \"eval\", \"src\": {}}}\n\
         {{\"id\": 2, \"op\": \"eval\", \"src\": \"(example (h host) => (ok nil))\"}}\n\
         {{\"id\": 3, \"op\": \"test\"}}\n\
         {{\"id\": 4, \"op\": \"host-log\"}}\n\
         {{\"id\": 5, \"op\": \"host-log\"}}\n\
         {{\"id\": 6, \"op\": \"quit\"}}\n",
        serde_json::to_string(defn).expect("json")
    );
    let out1 = run_lists(&["repl", "--host", "mock"], Some(&input));
    assert_eq!(out1.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out1.stderr));
    let out2 = run_lists(&["repl", "--host", "mock"], Some(&input));
    assert_eq!(out1.stdout, out2.stdout, "host-log 記録は同一入力に決定的（同一入力→同一記録 MUST）");
    let responses = repl_responses(&out1);
    assert_eq!(responses.len(), 5, "quit は応答しない: {responses:?}");
    assert_eq!(responses[0]["ok"], true, "effect+defn! eval: {responses:?}");
    assert_eq!(responses[1]["ok"], true, "example eval: {responses:?}");
    assert_eq!(responses[2]["ok"], true, "test: {responses:?}");
    let expected_calls = serde_json::json!([
        {"effect": "log", "args": ["5"]},
        {"effect": "tick", "args": ["5"]}
    ]);
    for (i, id) in [(3usize, 4u64), (4usize, 5u64)] {
        let r = &responses[i];
        assert_eq!(r["id"], serde_json::json!(id));
        assert_eq!(r["ok"], serde_json::json!(true));
        assert_eq!(r["host"], serde_json::json!("mock"));
        assert_eq!(r["calls"], expected_calls, "文書化した応答形式と一致し呼び出し順に蓄積: {r}");
        for c in r["calls"].as_array().expect("calls は配列") {
            assert!(c["effect"].is_string());
            assert!(
                c["args"].as_array().expect("args は配列").iter().all(|a| a.is_string()),
                "args の各要素は §11.6 print_value による文字列: {c}"
            );
        }
    }
    assert_golden_json("repl.host-log.responses.json", serde_json::Value::Array(responses));
}
