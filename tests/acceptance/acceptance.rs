use super::common::*;
use lists::check::{check_file, ModuleCache};
use lists::diagnostic::DiagKind;
use lists::emit::{emit, EmitInput};
use lists::eval::{global_env, load_file, Interp};
use lists::lexer::tokenize;
use lists::parser::parse_program;
use lists::value::{print_value, Value};
use std::collections::BTreeSet;

fn check_fixture(name: &str) -> lists::check::ModuleResult {
    let mut cache = ModuleCache::default();
    check_file(&fixture_path(name), &mut cache)
}

#[test]
fn t1_add_money_js_execution_value() {
    let dir = temp_dir("t1");
    compile_to_dir(&["money.lts"], &dir);
    std::fs::write(
        dir.join("driver.js"),
        "import { add_money } from \"./money.js\";\n\
         console.log(\"R1 \" + JSON.stringify(add_money({ amount: 1, currency: \"JPY\" }, { amount: 2, currency: \"JPY\" })));\n\
         console.log(\"R2 \" + JSON.stringify(add_money({ amount: 1, currency: \"JPY\" }, { amount: 2, currency: \"USD\" })));\n",
    )
    .expect("write driver");
    let r = run_node("driver.js", &dir, &[]);
    assert_eq!(r.status.code(), Some(0), "{}", String::from_utf8_lossy(&r.stderr));
    let stdout = String::from_utf8_lossy(&r.stdout);
    let r1 = stdout
        .lines()
        .find(|l| l.starts_with("R1 "))
        .map(|l| serde_json::from_str::<serde_json::Value>(&l[3..]).expect("json"))
        .expect("R1");
    let r2 = stdout
        .lines()
        .find(|l| l.starts_with("R2 "))
        .map(|l| serde_json::from_str::<serde_json::Value>(&l[3..]).expect("json"))
        .expect("R2");
    let ok_money = json_to_value(&serde_json::json!({"tag": "Ok", "value": {"amount": 3, "currency": "JPY"}}));
    assert!(values_equal(&json_to_value(&r1), &ok_money), "R1: {r1}");
    let err = json_to_value(&r2);
    assert!(
        matches!(&err, Value::Err(v) if matches!(v.as_ref(), Value::Str(s) if &**s == "currency mismatch: JPY vs USD")),
        "R2: {}",
        print_value(&err)
    );
}

// design/interp-capability.md §1.1: map/set のキーは str/sym/int のみ（現 SPEC では
// string/number）。それ以外は type-mismatch + fix テンプレート（キーを string に置換）。
#[test]
fn map_set_key_type_violation_is_type_mismatch_with_fix() {
    let mut cache = ModuleCache::default();
    let res = lists::check::check_source(
        "(defn bad (m : (map bool number)) : number 1)",
        "test.lts",
        &mut cache,
    );
    let d = res
        .diagnostics
        .iter()
        .find(|d| d.kind == DiagKind::TypeMismatch)
        .expect("type-mismatch");
    let data = d.data.as_ref().expect("data");
    assert_eq!(data["expected"], "(map string number)");
    assert_eq!(data["actual"], "bool");
    assert!(matches!(
        &d.fix,
        Some(lists::diagnostic::Patch::Replace { template, .. }) if template == "(map string number)"
    ));

    let mut cache = ModuleCache::default();
    let res = lists::check::check_source(
        "(type Bad (set (Array number)))",
        "test.lts",
        &mut cache,
    );
    let d = res
        .diagnostics
        .iter()
        .find(|d| d.kind == DiagKind::TypeMismatch)
        .expect("type-mismatch");
    assert!(matches!(
        &d.fix,
        Some(lists::diagnostic::Patch::Replace { template, .. }) if template == "(set string)"
    ));

    // string/number/リテラルキーは許可され、エラーなし
    let mut cache = ModuleCache::default();
    let res = lists::check::check_source(
        "(defn ok1 (m : (map string number)) : number 1)\n\
         (defn ok2 (s : (set number)) : number 1)\n\
         (type K string)\n\
         (defn ok3 (m : (map K number)) : number 1)",
        "test.lts",
        &mut cache,
    );
    assert!(
        res.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        res.diagnostics
    );
}

// obj->map は obj のフィールド値型の union を値型とする (map string (union ...)) になる。
#[test]
fn obj_to_map_infers_union_value_type() {
    let mut cache = ModuleCache::default();
    let res = lists::check::check_source(
        "(defn f (o : (obj (a : number) (b : string))) : (map string (union number string))\n\
         (obj->map o))",
        "test.lts",
        &mut cache,
    );
    assert!(res.diagnostics.is_empty(), "{:?}", res.diagnostics);
    let types = lists::check::type_of_all(&res);
    assert_eq!(
        types.get("f"),
        Some(&"(fn ((obj (a : number) (b : string))) : (map string (union number string)))".to_string())
    );

    // obj 以外に適用すると type-mismatch
    let mut cache = ModuleCache::default();
    let res = lists::check::check_source(
        "(defn g (x : number) : (map string number) (obj->map x))",
        "test.lts",
        &mut cache,
    );
    assert!(res
        .diagnostics
        .iter()
        .any(|d| d.kind == DiagKind::TypeMismatch));
}

// design/interp-capability.md §2.1: letrec の束縛対象は Lam のみ。それ以外は type-mismatch + fix。
#[test]
fn letrec_non_lam_binding_is_type_mismatch_with_fix() {
    let mut cache = ModuleCache::default();
    let res = lists::check::check_source(
        "(defn f () : number (letrec [x 1] x))",
        "test.lts",
        &mut cache,
    );
    let d = res
        .diagnostics
        .iter()
        .find(|d| d.kind == DiagKind::TypeMismatch)
        .expect("type-mismatch");
    assert!(matches!(
        &d.fix,
        Some(lists::diagnostic::Patch::Replace { template, .. }) if template == "(fn () : nil 1)"
    ));

    // Lam なら通る
    let mut cache = ModuleCache::default();
    let res = lists::check::check_source(
        "(defn f (n : number) : number (letrec [g (fn (k : number) : number k)] (g n)))",
        "test.lts",
        &mut cache,
    );
    assert!(res.diagnostics.is_empty(), "{:?}", res.diagnostics);
}

// design/interp-capability.md §1.5: ref! / ref-get! / ref-set! は組み込みエフェクト。
// 純粋 defn から呼ぶと effect-violation + fix（defn! へ）。defn! からは正常動作。
#[test]
fn ref_builtin_effect_violation_in_pure_defn_and_ok_in_defn_bang() {
    let mut cache = ModuleCache::default();
    let res = lists::check::check_source(
        "(defn bad () : (ref number) (ref! 0))",
        "test.lts",
        &mut cache,
    );
    let d = res
        .diagnostics
        .iter()
        .find(|d| d.kind == DiagKind::EffectViolation)
        .expect("effect-violation");
    assert!(matches!(
        &d.fix,
        Some(lists::diagnostic::Patch::Replace { template, .. }) if template == "defn!"
    ));

    let mut cache = ModuleCache::default();
    let res = lists::check::check_source(
        "(defn! ok (host : (obj)) : (ref number) (ref! 0))",
        "test.lts",
        &mut cache,
    );
    assert!(res.diagnostics.is_empty(), "{:?}", res.diagnostics);
    let types = lists::check::type_of_all(&res);
    assert_eq!(
        types.get("ok"),
        Some(&"(fn ((obj )) : (ref number))".to_string())
    );
}

// design/interp-capability.md §4（C9）: now!/elapsed! はモックで固定列（決定的）。
// bench: t0=now! → BASE、t1=now! → BASE+1000 なので (- t1 t0) = 1000。trace! は呼び出し記録へ。
#[test]
fn now_elapsed_bench_is_deterministic_and_trace_is_logged() {
    let src = "(defn! bench (host : (obj)) : (Result number nil)\n\
               (do [t0 (now!)]\n\
                 (do [_ (trace! \"bench\")]\n\
                   (do [t1 (now!)]\n\
                     (do [e (elapsed!)]\n\
                       (ok (- t1 t0)))))))\n\
               (example (bench (obj)) => (ok 1000))\n";
    let run_once = || {
        let mut cache = ModuleCache::default();
        let res = lists::check::check_source(src, "test.lts", &mut cache);
        assert!(res.diagnostics.is_empty(), "{:?}", res.diagnostics);
        let lex = lists::lexer::tokenize(src, "test.lts");
        let (program, _) = lists::parser::parse_program(&lex);
        let (defs, _) = lists::desugar::desugar(&program);
        let env = lists::eval::global_env();
        lists::eval::load_defs(&defs, &env);
        let mut interp = lists::eval::Interp::new();
        let results = lists::eval::run_examples(&mut interp, &defs, &env).expect("run");
        (results, interp)
    };
    let (r1, interp1) = run_once();
    assert!(r1.iter().all(|r| r.pass), "{:?}", r1.iter().map(|r| lists::value::print_value(&r.actual)).collect::<Vec<_>>());
    // trace! の呼び出し記録（effect/args 形式）
    let log = interp1.host_log.borrow().clone();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].effect.as_ref(), "trace");
    assert_eq!(lists::value::print_value(&log[0].args[0]), "bench");
    // 決定性: 同一入力を再実行すると同一出力
    let (r2, _) = run_once();
    assert_eq!(
        r1.iter().map(|r| lists::value::print_value(&r.actual)).collect::<Vec<_>>(),
        r2.iter().map(|r| lists::value::print_value(&r.actual)).collect::<Vec<_>>()
    );
    
}

// §4/§7.4: 宣言なしの呼び出しは unknown-effect、純粋 defn からの呼び出しは effect-violation（fix: defn!）。
#[test]
fn builtin_host_effect_diagnostics() {
    let mut cache = ModuleCache::default();
    let res = lists::check::check_source(
        "(defn! f (host : (obj)) : nil (foo! 1))",
        "test.lts",
        &mut cache,
    );
    assert!(res
        .diagnostics
        .iter()
        .any(|d| d.kind == DiagKind::UnknownEffect));

    let mut cache = ModuleCache::default();
    let res = lists::check::check_source(
        "(defn g () : nil (trace! \"x\"))",
        "test.lts",
        &mut cache,
    );
    let d = res
        .diagnostics
        .iter()
        .find(|d| d.kind == DiagKind::EffectViolation)
        .expect("effect-violation");
    assert!(matches!(
        &d.fix,
        Some(lists::diagnostic::Patch::Replace { template, .. }) if template == "defn!"
    ));

    // 組み込みエフェクト（now!/trace!）は宣言なしで defn! から使える
    let mut cache = ModuleCache::default();
    let res = lists::check::check_source(
        "(defn! ok-fn (host : (obj)) : (Result number nil) (do [_ (trace! \"x\")] (do [t (now!)] (ok t))))",
        "test.lts",
        &mut cache,
    );
    assert!(res.diagnostics.is_empty(), "{:?}", res.diagnostics);
}

// design/interp-capability.md §4.1: read-file! はモックの仮想ファイルから読む（決定的）。
// 無いファイルは (err (obj (tag "io-error")))。JS 差分テストからは除外（Node fs 依存）。
#[test]
fn read_file_effect_reads_virtual_files_and_io_error_on_missing() {
    let src = "(defn! load-len (host : (obj)) (p : string) : (Result number (obj (tag \"io-error\")))\n\
               (do [b (read-file! p)]\n\
                 (ok (bytes-len b))))\n\
               (example (load-len (obj) \"a.txt\") => (ok 3))\n\
               (example (load-len (obj) \"missing.txt\") => (err (obj (tag \"io-error\"))))\n";
    let mut cache = ModuleCache::default();
    let res = lists::check::check_source(src, "test.lts", &mut cache);
    assert!(res.diagnostics.is_empty(), "{:?}", res.diagnostics);
    let lex = lists::lexer::tokenize(src, "test.lts");
    let (program, _) = lists::parser::parse_program(&lex);
    let (defs, _) = lists::desugar::desugar(&program);
    let env = lists::eval::global_env();
    lists::eval::load_defs(&defs, &env);
    let mut interp = lists::eval::Interp::new();
    interp.virtual_files.insert("a.txt".to_string(), b"abc".to_vec());
    let results = lists::eval::run_examples(&mut interp, &defs, &env).expect("run");
    assert!(
        results.iter().all(|r| r.pass),
        "{:?}",
        results.iter().map(|r| lists::value::print_value(&r.actual)).collect::<Vec<_>>()
    );

    // 純粋 defn から read-file! → effect-violation（fix: defn!）
    let mut cache = ModuleCache::default();
    let res = lists::check::check_source(
        "(defn bad (p : string) : (Result bytes (obj (tag \"io-error\"))) (read-file! p))",
        "test.lts",
        &mut cache,
    );
    assert!(res
        .diagnostics
        .iter()
        .any(|d| d.kind == DiagKind::EffectViolation));

    // JS 生成は Node fs（node:fs/promises）へ写像される（実行は差分テスト対象外）
    let out = lists::emit::emit(&lists::emit::EmitInput {
        defs,
        file: "rf.js".to_string(),
        source_file: "test.lts".to_string(),
        type_names: BTreeSet::new(),
        impure_names: BTreeSet::from(["load-len".to_string()]),
        seed: lists::rng::DEFAULT_SEED,
        aliases: Default::default(),
    })
    .expect("emit");
    assert!(out.js.contains("node:fs/promises"), "{}", out.js);
    assert!(out.js.contains("$read_file("), "{}", out.js);
}

#[test]
fn t2_stock_non_exhaustive_with_fix_golden() {
    let res = check_fixture("stock.lts");
    let d = res
        .diagnostics
        .iter()
        .find(|d| d.kind == DiagKind::NonExhaustive)
        .expect("non-exhaustive");
    assert_eq!(d.severity, lists::diagnostic::Severity::Error);
    let data = d.data.as_ref().expect("data");
    assert_eq!(data["missing"], serde_json::json!(["(obj (tag \"OutOfStock\"))"]));
    assert!(matches!(
        &d.fix,
        Some(lists::diagnostic::Patch::Insert { path, template })
            if path == "(defn show-error).body.arms"
                && template.contains("[(obj (tag \"OutOfStock\")) \"...\"]")
    ));
    assert_golden_json("t2.stock.diagnostics.json", normalize_diags(&res.diagnostics));
}

#[test]
fn t3_do_early_return_total_not_called() {
    let mut cache = ModuleCache::default();
    let r = check_file(&fixture_path("cart-spy.lts"), &mut cache);
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);

    let env = global_env();
    let mut seen = BTreeSet::new();
    let defs = load_file(&fixture_path("cart-spy.lts"), &env, &mut seen).expect("load");
    let place_order = env.lookup("place-order").expect("place-order");
    let mut interp = Interp::new();
    let v = interp.apply(place_order, vec![Value::Array(vec![])]).expect("eval");
    assert!(matches!(v, Value::Err(_)), "{}", print_value(&v));
    assert!(
        !interp.calls().contains_key("total"),
        "total は呼ばれてはいけない: {:?}",
        interp.calls()
    );
    assert!(interp.calls().contains_key("check-items"));
    let _ = defs;

    let dir = temp_dir("t3");
    compile_to_dir(&["money.lts", "cart-spy.lts"], &dir);
    let r = run_node("cart-spy.js", &dir, &[]);
    assert_eq!(r.status.code(), Some(0), "{}", String::from_utf8_lossy(&r.stderr));
    let stdout = String::from_utf8_lossy(&r.stdout);
    assert!(stdout.contains("example 0: ok"), "{stdout}");
    assert!(stdout.contains("\"tag\":\"EmptyCart\""), "{stdout}");
    assert!(!stdout.contains("TOTAL-WAS-CALLED"), "{stdout}");
}

#[test]
fn t4_modules_import_and_cycle() {
    assert!(check_fixture("money.lts").diagnostics.is_empty());
    assert!(check_fixture("cart.lts").diagnostics.is_empty());
    let dir = temp_dir("t4");
    compile_to_dir(&["money.lts", "cart.lts"], &dir);
    let r = run_node("cart.js", &dir, &[]);
    assert_eq!(r.status.code(), Some(0), "{}", String::from_utf8_lossy(&r.stderr));
    let stdout = String::from_utf8_lossy(&r.stdout);
    assert!(stdout.contains("example 0: ok"), "{stdout}");
    assert!(stdout.contains("\"tag\":\"Err\""), "{stdout}");

    for name in ["cycle-a.lts", "cycle-b.lts"] {
        let res = check_fixture(name);
        assert!(
            res.diagnostics.iter().any(|d| d.kind == DiagKind::ModuleCycle),
            "{name}: {:?}",
            res.diagnostics
        );
    }
}

#[test]
fn t5_broken_lts_recovery() {
    let abs = fixture_path("broken.lts");
    let src = std::fs::read_to_string(&abs).expect("read");
    let lex = tokenize(&src, &abs);
    let (program, diags) = parse_program(&lex);
    assert_eq!(program.forms.len(), 2);
    let lists::ast::TopForm::Defn { name, body, .. } = &program.forms[0] else {
        panic!("forms[0] is not defn")
    };
    assert_eq!(&**name, "broken");
    assert!(matches!(body.as_ref(), lists::ast::Expr::Error { .. }));
    let lists::ast::TopForm::Defn { name, body, span, .. } = &program.forms[1] else {
        panic!("forms[1] is not defn")
    };
    assert_eq!(&**name, "ok");
    assert!(matches!(body.as_ref(), lists::ast::Expr::App { .. }));
    assert!(!diags.is_empty());
    assert!(diags.iter().all(|d| d.kind == DiagKind::ParseError));
    assert!(diags.iter().all(|d| d.span.end <= span.start));

    let (defs, _) = pipeline_file(&abs);
    let r = emit(&EmitInput {
        defs,
        file: "broken.js".to_string(),
        source_file: "broken.lts".to_string(),
        type_names: BTreeSet::new(),
        impure_names: BTreeSet::new(),
        seed: lists::rng::DEFAULT_SEED,
        aliases: Default::default(),
    });
    let errs = r.expect_err("broken は emit エラーのはず");
    assert!(errs.iter().any(|d| d.kind == DiagKind::ParseError));
}

#[test]
fn t6_effect_violation_with_defn_bang_fix_golden() {
    let res = check_fixture("effects.lts");
    let d = res
        .diagnostics
        .iter()
        .find(|d| d.kind == DiagKind::EffectViolation)
        .expect("effect-violation");
    assert!(d.message.contains("log!"));
    assert!(matches!(
        &d.fix,
        Some(lists::diagnostic::Patch::Replace { path, template })
            if path == "(defn pure-run)" && template == "defn!"
    ));
    assert_golden_json("t6.effects.diagnostics.json", normalize_diags(&res.diagnostics));
}
