use super::common::*;

// 差分テストはすべて Backend トレイト経由で実行する（M13）。
// デフォルトは js。LISTS_BACKEND=<name> で別バックエンド（例: mock）に切り替えられる。

#[test]
fn t7_diff_interp_vs_js_backend() {
    let backend = backend_from_env();
    for name in ["money.lts", "cart.lts", "cart-spy.lts"] {
        let names: Vec<&str> = if name == "money.lts" {
            vec!["money.lts"]
        } else {
            vec!["money.lts", name]
        };
        assert_diff(&*backend, &names, name);
    }
}

#[test]
fn t10_diff_deep_tail_recursion_interp_vs_js() {
    let backend = backend_from_env();
    for name in ["tail-self.lts", "tail-mutual.lts"] {
        assert_diff(&*backend, &[name], name);
    }
}

// モックバックエンド（インタープリタ結果をそのまま返す最小実装）でも
// トレイト経由の差分テストが機能することの証明。将来の WASM 追加時も同じ手順で乗る。
#[test]
fn t11_diff_via_mock_backend() {
    let backend = lists::backends::lookup("mock").expect("mock backend");
    assert_eq!(backend.name(), "mock");
    assert_diff(&*backend, &["money.lts"], "money.lts");
    assert_diff(&*backend, &["money.lts", "cart.lts"], "cart.lts");
}

// M16（v0.3 事前確定 #4）: SPEC §11.6 の規範的意味論（stdlib 5件 + 印字規約）の差分テスト。
// T7/T10 の対象ケースは不変のまま、追記のみ。
#[test]
fn stdlib_edge_diff_interp_vs_backend() {
    let backend = backend_from_env();
    assert_diff(&*backend, &["stdlib-edge.lts"], "stdlib-edge.lts");
}

// M19 (P1 defn! 完全実装): エフェクト呼び出しを含む defn! の差分テスト。
// host は defn! の第1引数として example 内で明示的に渡す（SPEC §12.6）。
#[test]
fn effect_run_diff_interp_vs_backend() {
    let backend = backend_from_env();
    assert_diff(&*backend, &["effect-run.lts"], "effect-run.lts");
}

// M16 採用済み判断: (a) (union T nil) は実行時に生値（find/nth は成功で生値 T、失敗で nil）、
// (b) split("") は Unicode スカラ値単位で空要素なし。example の `=> <生値>` で観測する。
// option-find.lts は cart.lts の find 成功アーム（NegativeQty）を実行時に通す。
#[test]
fn option_raw_and_split_diff_interp_vs_backend() {
    let backend = backend_from_env();
    assert_diff(&*backend, &["option-raw.lts"], "option-raw.lts");
    assert_diff(
        &*backend,
        &["money.lts", "cart.lts", "option-find.lts"],
        "option-find.lts",
    );
}

// M21 do-io (SPEC §3.5): 名前なし do 束縛 `(do e1 e2 ...)`。エフェクト連続実行と
// Result を返す impure 呼び出しの早期 return を interp/backend 両方で検証する。
#[test]
fn do_io_diff_interp_vs_backend() {
    let backend = backend_from_env();
    assert_diff(&*backend, &["do-io.lts"], "do-io.lts");
}

// M23 decimal: 固定小数点 SCALE=18。丸め（half-away）を含め interp と JS で同一結果になること。
#[test]
fn decimal_edge_diff_interp_vs_backend() {
    let backend = backend_from_env();
    assert_diff(&*backend, &["decimal-edge.lts"], "decimal-edge.lts");
}

// design/interp-capability.md §1.1（C1 map/set）: 基本操作・挿入順・構造的等価・永続性・
// obj->map・末尾再帰 fold（§8.5）・印字の interp/JS 差分一致。
#[test]
fn map_edge_diff_interp_vs_backend() {
    let backend = backend_from_env();
    assert_diff(&*backend, &["map-edge.lts"], "map-edge.lts");
}

// design/interp-capability.md §1.2/§3.1（C2 char + 文字列プリミティブ）: スカラー値境界（絵文字）・
// out-of-range/invalid-code の result 往復・位置ベース API のレキサー・union/match・印字の差分一致。
#[test]
fn char_edge_diff_interp_vs_backend() {
    let backend = backend_from_env();
    assert_diff(&*backend, &["char-edge.lts"], "char-edge.lts");
}

// design/interp-capability.md §2.1（letrec）/ §1.5（ref）/ §3.2（builder）:
// 自己再帰・親環境クロージャ・ref の参照等価と共有セル・10^4 builder 畳み込みの差分一致。
#[test]
fn letrec_ref_builder_diff_interp_vs_backend() {
    let backend = backend_from_env();
    assert_diff(&*backend, &["letrec-ref-builder.lts"], "letrec-ref-builder.lts");
}

// design/interp-capability.md §4（C9）: trace! の interp/JS 差分一致（JS では console.error=stderr）。
// now!/elapsed! は JS が Date.now 由来で非決定的のため、ここではなく interp 側のみテストする。
#[test]
fn hostfx_trace_diff_interp_vs_backend() {
    let backend = backend_from_env();
    assert_diff(&*backend, &["hostfx.lts"], "hostfx.lts");
}

// design/interp-capability.md §1.4（C8 bytes）: UTF-8 往復・invalid-utf8/invalid-code・
// codepoints（サロゲート境界）・#u8 印字・等価の interp/JS 差分一致。
// read-file! は Node fs 依存のため差分テストからは除外。
#[test]
fn bytes_edge_diff_interp_vs_backend() {
    let backend = backend_from_env();
    assert_diff(&*backend, &["bytes-edge.lts"], "bytes-edge.lts");
}

// M22 prop の JS バックエンド実行: 同一 seed で interp（prop_result_json）と compile+node の
// `prop i: ok|FAIL <json>` 行が同一バイト列になること（反例・縮小・seed を含む）。
#[test]
fn prop_diff_interp_vs_node_same_seed_byte_identical() {
    let seed: u64 = 0xDEAD_BEEF_1234_5678;
    let abs = fixture_path("props-edge.lts");
    let src = std::fs::read_to_string(&abs).expect("fixture");

    // interp 側: 各 prop を run_prop して prop_result_json をコンパクト直列化
    let mut cache = lists::check::ModuleCache::default();
    let res = lists::check::check_source(&src, "props-edge.lts", &mut cache);
    assert!(res.diagnostics.is_empty(), "{:?}", res.diagnostics);
    let lex = lists::lexer::tokenize(&src, "props-edge.lts");
    let (program, _) = lists::parser::parse_program(&lex);
    let (defs, _) = lists::desugar::desugar(&program);
    let env = lists::eval::global_env();
    lists::eval::load_defs(&defs, &env);
    let mut interp_jsons: Vec<String> = Vec::new();
    for (i, d) in defs.iter().enumerate() {
        if let lists::desugar::CoreDef::Prop {
            name, params, body, ..
        } = d
        {
            let outcome = lists::prop::run_prop(
                name,
                params,
                body,
                &env,
                &res.type_aliases,
                seed,
                lists::prop::DEFAULT_MAX_TRIALS,
            );
            let j = lists::prop::prop_result_json(
                serde_json::json!(i),
                name,
                seed,
                params,
                body,
                outcome,
            );
            interp_jsons.push(serde_json::to_string(&j).expect("ser"));
        }
    }
    assert_eq!(interp_jsons.len(), 4);

    // JS 側: --seed を埋め込んで compile+node
    let dir = temp_dir("prop-diff");
    let artifacts = compile_via_seed(&lists::backends::JsBackend, &["props-edge.lts"], &dir, seed);
    let run = lists::backends::Backend::run(&lists::backends::JsBackend, &artifacts).expect("run");
    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr);
    let stdout = run.stdout;
    let js_jsons: Vec<String> = stdout
        .lines()
        .filter_map(|l| {
            l.strip_prefix("prop ").and_then(|rest| {
                let (idx, rest) = rest.split_once(": ")?;
                let (status, json) = rest.split_once(' ')?;
                assert!(status == "ok" || status == "FAIL", "{l}");
                let _ = idx;
                Some(json.to_string())
            })
        })
        .collect();
    assert_eq!(js_jsons.len(), 4, "{stdout}");
    assert_eq!(interp_jsons, js_jsons, "同一 seed で反例 JSON が同一バイト列のはず");

    // 決定性: 同一 seed で再実行すると同一 stdout
    let run2 = lists::backends::Backend::run(&lists::backends::JsBackend, &artifacts).expect("run2");
    assert_eq!(run2.stdout, stdout, "同一 seed の再実行は同一結果（決定性）");

    // 反例が実際に出ていること（never equal は FAIL で shrunk x=0）
    assert!(stdout.contains("\"FAIL\"") || stdout.contains(": FAIL "), "{stdout}");
    assert!(stdout.contains("\"shrunk\":{\"x\":0.0}"), "{stdout}");
}
