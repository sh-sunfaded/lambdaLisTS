# REPORT.md — codestral（本エージェント）による判断・決定事項のまとめ

- 対象: PR [#5](https://github.com/KennedySilverDollar/lambdaLisTS/pull/5)（ブランチ `vibe/v03-pre-audit-349e09`）の4コミット
  - `6175de8` v0.3 pre-audit（分析専用: `analysis/v0.3-pre-audit.md`）
  - `f14d50e` M12: proper tail call の定数スタック保証
  - `fab7e63` M13: Backend トレイトによる差分テスト一般化
  - `8cb8b38` M14: バックエンド可搬性監査（`audit/backend-portability.md`）
- 規範: SPEC.md v0.2（言語規定）と GUIDE.md（実装規約）。SPEC の文言の最終採用権はユーザーにあり、意味論変更は差分提案のみ行い採用指示を待った。

---

## 1. 事前監査（pre-audit）での推奨決定（`analysis/v0.3-pre-audit.md`）

| 項目 | 推奨案 | 却下した案 | 理由（抜粋） |
|---|---|---|---|
| 末尾再帰 | (B) MUST「proper tail call は定数スタック」を v0.3 で導入 | (A) SHOULD据え置き / (C) internal-error区別のみ | バックエンド交換で意味論を変えないための核心約束。WASM `return_call` に直結 |
| 数値 | (a) 全数値 IEEE754 f64 と明記（現状の成文化） | (b) Int=i64/Float=f64（JSでBigInt化コスト大） / (c) i53（レンジ規定コスト） | 両バックエンドの単一表現・T7差分一致を最優先。2^53超の丸めは仕様上の値として受容 |
| 循環 `=` | (b) 循環検出で実行時エラー（§8.6） | (a) 非終了の明文化（同値関係を失う） / (c) 余帰納的等価（意味が変わる） | P0では循環は構成不能なので全案が観測等価。(b) なら将来の host 混入時も「止まらない」でなく定義済みエラー |
| 実行時プリミティブ | 「実行時意味論の規範は付録CのRust型、各バックエンドはその射影」という条文を提案 | — | runtime.js の3 export は §12.5 と一致することを実装と文書の両面で確認済み |

## 2. M12（末尾再帰保証）での実装決定

- **SPEC.md §8.5**: ユーザー採用済みの差分のみを反映（末尾位置の MUST 列挙 + proper tail call 定数スタック MUST）。非末尾のスタック制限条項は不変。
- **インタープリタ** (`src/eval.rs`): `eval_core` = 深さガード + 末尾ループ `eval_loop`。末尾の `App` はフレーム無消費。深さ上限 `MAX_CALL_DEPTH=10000` は非末尾ネストのみに適用（ガードを eval_core 入口に移動）。決定: 深さ上限テストは末尾再帰フィクスチャ（deep-add）から非末尾（fact）へ付け替え（監査で予見した回帰）。
- **JSバックエンド** (`src/emit.rs`): 末尾呼び出し先を解析（末尾ウォーク+SCC）。自己再帰 → `while (true)` + 引数を一時変数に評価してからの매개변수再代入 + `continue`（引数の逐次評価順序のため一時変数化）。相互再帰（SCC>1）→ `$inner` + `$thunk`/`$run` トランポリン。決定: thunk 印は `Symbol("lists.tail")` としユーザーobjの `tag` 文字列と衝突しない方式を採用。非再帰関数の生成コードは不変（golden無更新で緑）。ラムダは Normal 出力のまま（高階末尾呼び出しの定数スタック保証は対象外と明記）。
- **型検査** (`src/check.rs`): 同一ファイル内の相互再帰 defn を表現可能にするため defn 署名の事前登録を追加（import は先行処理のまま。duplicate/unbound 診断は既存テストどおり維持）。SPEC はファイル内順序を不問のため実装ギャップの修正と判断。
- **検証**: 10^6 自己/相互再帰が interp/JS 双方で成功、diff一致（新T10）、source map はループ化箇所も .lts へマップ。Node v22.23.4 実測: `count_down(1e8)` 成功、対照の非末尾 `fact(1e4)` は RangeError。

## 3. M13（Backend トレイト）での設計決定

- トレイトは仕様どおり3メソッド: `name()` / `compile(sources, out_dir) -> Result<Artifacts, Vec<Diagnostic>>` / `run(artifacts) -> io::Result<RunOutput>`。`run` は子プロセス実行で環境変数継承・stderr取り込み・exit code返却（現行の Node 実行と同一挙動を維持。タイムアウトは現行どおり設けない）
- レジストリ方式（`registry()`/`names()`/`lookup()`）で `js` と `mock` を収録。CLI は `compile --backend <name>`（デフォルト `js`、未知名は exit 2 + 診断JSON）。テストランナーは環境変数 `LISTS_BACKEND`（デフォルト `js`）
- 差分テスト（T7/T10系）は `compile_via`/`assert_diff` に集約しトレイト経由へ移行。**対象ケース・アサートは不変**
- モックバックエンド（compile=フロントエンドのみ、run=インタープリタ結果を `example N: ok <json>` で返す）で拡張性を立証（`t11` + `LISTS_BACKEND=mock` で t7/t10 が mock に対して緑を実測）
- ガイドは `docs/backends.md` に1ページ（3メソッドの契約・登録手順・差分参加手順・stdout形式の約束）

## 4. M14（可搬性監査）での決定（`audit/backend-portability.md`）

- **問題なしと確認**: Core IR/直列化の JS 非依存、runtime.js と付録Cの一致（テストで突き合わせ）、診断JSONへのJS文言混入なし、golden スナップショット方式
- **挙動不変リファクタで修正**: `js_slice_bounds`→`slice_bounds` 改名、diagnostic.rs の死コード `unimplemented!` 削除、生成JS文字列を直接検査していたテスト4箇所を挙動検査へ置換（T8のエンジンメッセージリテラルも除去）、付録C突き合わせテスト2本・診断ガードテスト1本を追加
- **M12へ分離（未変更）**: §8.6「実行時エラー＝JSエンジンの例外」のバックエンド中立化、stdlib の5件のバックエンド間食い違い（min/max NaN、len/str-slice UTF-16、str 数値表記、nth 小数添字）、`str` 印字規約

## 5. ユーザーに委ねた未決定事項（本セッションでは提案のみ）

- 数値意味論の (b) i64 / (c) i53 の採否（推奨は (a) f64現状維持）
- 循環 `=` の (c) 余帰納的等価の採否（推奨 (b)）
- §8.6 の実行時エラー定義のバックエンド中立化の差分採用
- stdlib 5件の食い違いの修正方針（実装バグ修正 vs 仕様変更）
- `str` の数値印字規約の明文化
- P1 ロードマップ（defn!完全実装、prop検査等）の着手順

## 6. 制約遵守の記録

- 依存は serde/serde_json/thiserror/clap/sourcemap のみ（Cargo.toml無変更）。`unsafe` は `#![forbid(unsafe_code)]` のまま。ライブラリ経路に unwrap/expect 追加なし（M14で死コードの `unimplemented!` をむしろ除去）
- 決定性: 新規コードは BTreeMap/BTreeSet のみで反復順に依存せず、生成JSの決定性テストは緑
- Node ≥ 20 は生成JSの実行・差分テスト専用（処理系本体は非依存）
- 外部I/F（REPL JSON Lines、診断JSON形式、CLI出力形式）は無変更
- 回帰: テスト件数 162（開始時）→ 169（M12）→ 173（M13）→ **176 全パス**（M14時点。T1–T7/T10/T11受入 + backend + golden + tools含む）

---

## 7. M16（v0.3 事前確定4項目の SPEC 反映と両バックエンド実装）

対象はユーザーにより**事前確定済みの次の4判断のみ**であり、この範囲内で SPEC.md へ直接反映した（範囲外の意味論変更は従来どおり差分提案のまま未変更）。T1–T11 は文言無変更で回帰実行した。

- **判断ゲート採用（数値f64・循環(b)・§8.6中立化）**: 上記3判断を SPEC.md 条文と実装に反映した。§0 に §0.3「v0.2 内の追補（バージョン番号は据え置き）」として変更点リストを記録し、版番号は v0.2 のままとした。

| # | 確定事項 | SPEC 反映 | 実装 |
|---|---|---|---|
| (1) | 数値は IEEE754 f64 のまま確定 | §8.6 は現行文言「IEEE754 倍精度浮動小数点数（f64。JS準拠）」を確定済み（MUST）と明記し、§14 に「i64/i53 への変更は P5（ネイティブAOT）で性能要件が発生した場合に再検討」の一行を追記（丸め・±Infinity・NaN≠NaN の明示は維持） | 変更不要（既存の f64 実装が既に規範に適合。unit test で print 規約を固定） |
| (2) | 循環構造上の `=` は循環検出で実行時エラー | §8.3 に「P0 では循環構造を作る構文がないので `=` は常に停止（MUST）。将来 host 等で循環が現れた場合は循環検出時点で実行時エラー（§8.6）」を追記し、署名を `Result<bool, EqualError>` に変更。§8.6 にも実行時エラー条項を追記 | `src/value.rs`: 比較経路上の同一値対の再訪を検出する `EqualError::Cyclic`（thiserror）付き `structural_equal`。`src/eval.rs`: `=` builtin・`run_examples` は `EvalError::Runtime` へ変換、match リテラルは従来どおり false 扱い。`src/backend/runtime.js`: `structuralEqual` に経路ベース循環検出（throw）。DAG 共有では false positive にならないことも test 済み |
| (3) | §8.6 の実行時エラー定義のバックエンド中立化 | §8.6 を書換:「P0 の実行時エラー = 実行が継続できない意味論上の事態。各バックエンドの実行時例外として観測される（JS: スタック枯渇等の実行時例外、WASM: trap 等）。例外の型名・メッセージ・スタック表現はバックエンド依存で規範外。source map が .lts 行への対応付け（T8）」 | 文言のみ（T8 の source map 検査は維持） |
| (4) | stdlib 5件+印字規約は §8.6・付録C・Rust実装を規範としJS側を修正 | §11.6 を新設（min/max は NaN でない側を返す minNum/maxNum、len は Unicode スカラ値数、str-slice/slice は char/要素単位の slice_bounds 規則、nth は添字切捨て、`str` の印字規約を全kind記述）。付録C に #10（print_value が印字の規範的写像、f64 は指数表記なし）と #11（$stdlib_* は §11.6 と一致すること）を追記。§12.5 に structuralEqual の循環検出とローカル補助関数の方針を注記。§0.2 に4点の反映済みを追記 | `src/emit.rs` の `$stdlib_*` を修正: min/max（NaN 時は他方）、len（`[...s].length`）、str-slice/slice（char 単位 + 生成コード側 `$slice_bounds`）、nth（`Math.trunc`）、str（生成コード側ローカル `$print`/`$print_num`/`$cmp_str` で Rust `print_value` 互換。f64 は `String(n)` の指数表記を位置al 展開し `-0`/`NaN`/`±Infinity` を規約どおり印字）。runtime.js の export は ok/err/structuralEqual の**3つのまま**（§12.5 MUST） |

- **テスト（すべて追記のみ）**: `tests/fixtures/stdlib-edge.lts` 新設（example 36件: stdlib 5件 + 印字規約 + 等価性）→ `tests/acceptance/diff.rs` に `stdlib_edge_diff_interp_vs_backend` を追記（T7/T10 の対象ケースは不変）。`tests/backend.rs` に (a) runtime.js の循環検出 node テスト（cyclic array/obj は throw、「DAG 共有は false positive にしない」「関数は参照等価」control 込み）、(b) stdlib-edge の emit 決定性 + 補助関数出力テストを追記。`tests/tools.rs` の診断JSONスキャン対象に stdlib-edge.lts を追記。`src/value.rs` unit test に循環検出（経路再訪で Cyclic）・同一参照は true・print_value の数値/コンテナ規約を追記。
- **回帰**: `cargo test` **183件全パス**（旧176 + 新7 = unit 4 + acceptance 1 + backend 2）。`LISTS_BACKEND=mock cargo test --test acceptance` も全パス（mock 経由の差分にも stdlib-edge が乗る）。golden（money.js / cart.js / 診断JSON 等）は**不変** — 修正した stdlib 実装を使うのが新規 fixture のみのため。
- **制約遵守**: 依存は serde/serde_json/thiserror/clap/sourcemap のみ（Cargo.toml 無変更）。`#![forbid(unsafe_code)]` のまま。ライブラリ経路に unwrap/expect 追加なし（src 走査で確認。新規コードは map_err / unwrap_or のみ）。決定性: 出力順は BTreeMap/BTreeSet のみに依存（header の補助関数は stdlib_used に応じた固定順で出力、emit 2回一致をテストで確認）。Node ≥ 20 は生成JSの実行・差分テストのみ。
- **検証コマンド**: `cargo test`（183 passed）、`LISTS_BACKEND=mock cargo test --test acceptance`、`cargo run -q -- compile tests/fixtures/stdlib-edge.lts --out <dir>` → node で全 example ok（diff テスト内で実施済み）。

- **SPEC 条文の詳細（§0.3 の3判断）**:
  - §8.6 数値: 現行文言「IEEE754 倍精度浮動小数点数（f64。JS準拠）」を確定済み（MUST）と明記し、§14 に「i64/i53 への変更は P5（ネイティブAOT）で性能要件が発生した場合に再検討」の一行を追記。
  - §8.3 に「循環構造における構造的等価」を追加: `=` は共有・循環を追跡し比較経路上の同一値対の再訪（循環）で実行時エラー（MUST）。バックエンド間対応を表で列挙（Rust interp: `Value` は所有ベースの木構造で循環表現不可能のため検出不要・発火しない／JS: 参照共有可能のため `structuralEqual` が訪問済み同一性ペア集合（WeakMap）で検出し未処理実行時例外として報告）。「P0 では言語内から循環データを構成できないため観測されない」旨を明記。検出は比較経路上のため DAG 共有は循環とみなさない。
  - §8.6 の実行時エラー定義をバックエンド中立化し対応を表で規定（JS: 未処理の実行時例外／Rust interp: internal-error 診断。例外の型名・メッセージ・スタック表現は規範外）。T8 の source map 検証は JS バックエンドにおける具体例である旨を維持。§12.5 も WeakMap による検出へ文言更新。
- **実装**: `src/backend/runtime.js` の `structuralEqual` を訪問済み同一性ペアの集合（`WeakMap<a, WeakSet<b>>`、退去時に除去して比較経路のみ保持）による循環検出へ変更し、検出時は回復不能な実行時エラー（throw）を送出。export は ok/err/structuralEqual の3つのまま（§12.5 MUST）。Rust 側は循環不可能のため実装変更不要の旨を `src/value.rs` の `structural_equal` にコメントで記録（検出コード自体は発火しない）。
- **ホスト側テスト**（言語経由では循環構成不能のためホスト側に配置）: `tests/backend.rs` の `structural_equal_throws_on_cyclic_structures` で runtime.js を直接 import し、手作りの循環（配列の自己参照・obj フィールドの自身参照・間接循環 x→y→x）で throw されることを検証。control として非循環 true・DAG 共有 true（誤検出なし）・関数は参照等価 false を確認。
- **性能**: 循環検出の通常データへのオーバーヘッドを差分テスト一式（`cargo test --test acceptance diff::`、3回計測中央値）で確認。main 8.44s → 本ブランチ 8.65s（約 +2.5%）。全 acceptance Suite でも main 約8.6s → 約8.8s（約 +2〜3%）。2割超の劣化ではないため、深さ閾値超過時に訪問集合を遅延生成する緩和案は採用せず報告のみとする。
- **回帰**: `cargo test` 183件全パス（T1–T11 文言無変更）、`LISTS_BACKEND=mock cargo test --test acceptance` も全パス。golden 無変更。依存・unsafe 禁止・ライブラリ経路の unwrap/expect 禁止は継続して遵守（Cargo.toml 無変更、`#![forbid(unsafe_code)]` 健在）。

### 残課題（差分提案のみ。本マイルストーンでは未変更）

- `split` の空区切り: Rust `"abc".split("")` は `["","a","b","c",""]`、JS は `["a","b","c"]`。§11.6 の5件の範囲外のため未修正。提案: slice_bounds と同様に §11.6 へ規約を追記して JS 側を修正する。
- `str` 印字の corner: JS lowering では Result の `{tag,value}` 形状と、`tag`/`value` の2キーのみを持つ obj が区別できない（§11.6 に注記済み）。型検査上は `=` で混同されないが、`str` では印字が一致しない場合がある。
- pre-audit 作業1の (C)（非末尾再帰の深さ上限超過を internal-error から runtime error にするか）は未採用のまま。§8.5/§8.6 は現行どおり（Rust interp は internal-error）。
- `nth`/`find` は実行時に `Ok` で包んだ値を返す一方、型は `(union T nil)` であり、cart.lts の find 成功アーム（`i.id`）は現行実行時に失敗し得る。Option の実行時表現（raw T か Ok 包みか）の規範化は別途差分提案が必要（本範囲外のため未変更・観測のみ）。
- P1 ロードマップ（defn! 完全実装、prop 検査、host DI 等）は §14 のまま。

---

## 8. M16 採用(4)詳細: stdlib 5件＋印字規約の interp/JS 食い違い修正（規範 = §8.6・§11.6・付録C・Rust実装）

M14 監査で発見された食い違いを、規範（§8.6・付録C・Rust実装）に合わせて JS 側（`src/emit.rs` の `$stdlib_*` とローカル補助関数）を修正した。仕様文言が曖昧だった箇所は §11.6 を明確化し（採用(4)の委任範囲）、§0.3 の変更点リストにも追記済み。runtime.js の export は ok/err/structuralEqual の3つのまま（§12.5 MUST）。

| 項目 | 採用規範（文言の所在） | Rust 実装の適合確認 | JS 側の修正 | 差分テスト追加 |
|---|---|---|---|---|
| 1. min/max の NaN・±0・±Infinity | §11.6（明確化）: 片方 NaN → 他方、両方 NaN → NaN。それ以外は `a<b ? a : b`（max は `a>b ? a : b`）。同値（±0含む）なら第2引数 | `src/eval.rs` の `f64::min`/`f64::max`。±0 同士の比較は LLVM 上規格未規定で debug/release で符号が異なることを実測したため、規範は「同値なら第2引数」と明記し JS も統一 | `Math.min/max`（NaN 伝搬・±0順序が不一致）をやめ、NaN チェック後に `a < b ? a : b` / `a > b ? a : b` の数値専用パスで明示実装 | `stdlib-edge.lts` に NaN・±0（4ケース）・±Infinity の str 観測を追加 |
| 2. len / str-slice の単位 | §11.6: Unicode スカラ値（char）数。サロゲートペアは 1 | `chars().count()` / `chars().collect()` で適合 | `s.length` → `[...s].length`（コードポイント換算）、str-slice も char 配列経由 + `$slice_bounds` | 絵文字 😀 の len/str-slice 境界（先頭・中間・NaN end）を追加 |
| 3. str の数値表記 | §11.6/付録C #10: 最短往復10進・指数表記なし、整数値に `.0` 無し、NaN/±Infinity/-0 は専用表記 | `print_value` は Rust `f64` Display（指数表記なし）に適合 | `String(n)` の指数表記（`1e+21` 等）を `$print_num` で展開するローカル補助関数を生成コードに埋め込み | 0, 1.0→"1", 1e15/1e16/1e20/1e22, -1e21, 2^53, 0.5, 1e-6 の境界を追加 |
| 4. nth の小数添字（＋ slice/str-slice の小数 bounds） | §11.6: `i >= 0 && trunc(i) < len` なら `ok(arr[trunc(i)])`、他は `nil`。slice_bounds の start/end も 0 方向切り捨て。nth は配列のみ（文字列は str-slice） | `i as usize`（0方向切り捨て）と `slice_bounds` の `as usize` で適合 | `Math.trunc` による切り捨て＋`$slice_bounds`（`Math.trunc` 込み） | nth に 0.5/2.99/-0.5、str-slice/slice に小数 bounds（1.9, 4.2 等）を追加 |
| 5. str の印字規約 | §11.6/付録C #10: 文字列は引用符・エスケープなしで生印字。配列 `[a b c]`、obj はキーを UTF-8 バイト順に `{k: v, ...}`、Result `(ok v)`/`(err v)`、関数 `<fn>`。REPL の結果印字（actual/expected）も同規約。診断は span/kind で示し値を示す場合は同規約 | `src/value.rs` > `print_value` が規範通り。REPL は `print_value` を使用（repl.rs） | 生成コードにローカル `$print`/`$print_num`/`$cmp_str` を埋め込み（キーはコードポイント順ソート） | 文字列の生印字（`"`・改行を含む）、ネスト構造（配列内文字列・obj 内 Result/文字列）を追加 |

- **検証**: `tests/fixtures/stdlib-edge.lts` は example 70件（旧36 + 新規34）すべて interp vs JS backend（`--backend js`）で一致し、期待値とも一致（`diff::stdlib_edge_diff_interp_vs_backend`）。`cargo test` 183件全パス、`LISTS_BACKEND=mock` でも全パス。T1–T11・golden は無変更。
- **残課題（持ち越し）**: `split("")` の空区切りの食い違い（Rust は `["","a","b","c",""]`、JS は `["a","b","c"]`）は §11.6 の5件の範囲外のため未修正。`str` 印字で JS の `{tag,value}` 形状と2キー obj が区別不能な点は §11.6 に注意として記載済み。

---

## 9. name-convention 検査の実装（スモーク S3 で発見された仕様・実装ギャップ）

- **発見経緯**: 実用確認スクリプト `scripts/lists-smoke.sh` の S3（name-convention）で、camelCase の defn 名が診断 `[]`・exit 0 で受理されることが判明（再現: `(export badName)` + `(defn badName (x : number) : number x)`）。SPEC は §2.3（シンボル名は kebab-case MUST）、§9.1（kind name-convention、フェーズ=検査）、§9.3（fix テンプレート MUST、kebab-case 変換）を要求しており、実装が追いついていなかった。なお `scripts/lists-smoke.sh` はリポジトリに存在しなかったため本件で新設した（S1–S20、S3 が name-convention: kind + `fix.template === "bad-name"` を検査）。
- **根拠条文**: §2.3（kebab-case MUST、`_` 単体はワイルドカード専用）、§9.1（name-convention は検査フェーズ）、§9.3（fix は kebab-case 変換テンプレート MUST）、§10.2（path は AST パス）、§6.6（型名前空間と値名前空間の分離）。
- **実装** (`src/names.rs`, `check_source` から検査フェーズ（AST 構築後）で実行。パーサ回復（Error ノード）とは独立に動作し、1ファイル内の全違反を報告):
  - 検出規則（値名前空間）: 大文字を含むか `_` を含む（`_` 単体は除く）場合に severity=error の name-convention。`-`・数字・末尾 `?` は正当。
  - 型名前空間（type 定義名・型位置の alias・import/export の型名）: PascalCase が規範（§6.6 と付録A/B の型名の例）なので大文字自体は違反にせず、snake_case（`_` 含む）と camelCase（小文字始まりで大文字含む）のみ違反とする。import/export 名は「大文字を含むか type_aliases に存在すれば型名」と分類し、import 失敗時の誤報を回避。
  - 対象位置: defn/defn! 名、type 名、effect 名、param 名、let/do bind 名、fn param 名、obj フィールド名（構築・パターン・型の両方）、match パターン変数名、import/export シンボル、dotted-symbol の各セグメント。
  - fix: `Patch::Replace { path, template }`、path は §10.2 の AST パス（例: `(defn badName)`、`(export).names.0`）、template は kebab 変換名（badName→bad-name、bad_name→bad-name）。
- **テスト**: unit 8件（`src/names.rs`: camelCase/PascalCase/snake_case 検出、`_` 許可、kebab 名（add-money/contains?/x2）許可、全位置の網羅検出、dotted セグメント、check_source 経由）、ゴールデン `tests/golden/name-convention.diagnostics.json`（再現ケースの診断 JSON、fix 含む）、CLI テスト `cli_compile_camel_case_exits_1_with_name_convention_and_fix`（exit 1 + kind + template）、スモーク `bash scripts/lists-smoke.sh` → **20/20（S3 PASS）**。
- **既存ソースの確認（報告事項）**: 全17 fixtures と付録A/B の全 lisp ソースを compile し name-convention 診断 0件を確認。型名の PascalCase（`Money`/`CartItem` 等）は §6.6 の型名前空間の分離と付録A/B の規範例により仕様上正当と判断したため、型名前空間には大文字ルールを適用していない（fixture は無変更）。もし型名も kebab-case に統一したい場合は別途採用指示を乞う（差分提案レベルの意味論変更のため本件では未変更）。
- **回帰**: `cargo test` 193件全パス（T1–T11 文言無変更、golden 既存ファイル無変更）、`LISTS_BACKEND=mock` acceptance 全パス、warning 0。依存・unsafe・unwrap/expect 制約は継続遵守。

---

## 10. M19 (P1 defn! 完全実装)

対象は SPEC.md がすでに P1 として規定する範囲（§7.2 eff 集合、§7.4 検査規則、§7.5、§12.3 末尾 `!`、§12.4 lowering、§12.6 host DI）。本マイルストーンでは「採用済み」と明記された新規文言がないため **SPEC.md は無変更**。T1–T11・既存ゴールデンも無変更（t6.effects.diagnostics.json を含む全ゴールデンが byte 一致で通ることを確認）。

### 実装

- **IR** (`src/ir.rs`): `Core::EffectCall` に `host: Rc<str>` を追加（囲んでいる defn! の第1引数名。defn! の外では空文字）。`src/desugar.rs` が defn! 本体の脱糖時にのみ記録する。
- **検査** (`src/effects.rs`, `src/check.rs`):
  - eff の不动点を計算し、純粋 defn がエフェクトへ到達可能なら `effect-violation`。直接の EffectCall は従来どおり span=呼び出し位置・fix=`defn!` 置換（t6 ゴールデン不変）。defn→defn! の直接呼び出しと、純粋経由の間接到達（§7.4「逆は禁止」＋§7.2 eff(defn)=∅ MUST）も呼び出し位置で `effect-violation`（同じ fix）。
  - エフェクト呼び出しを含むのに引数のない defn! → host 不足の `effect-violation`。example 等 defn! 外の EffectCall → host が無いため `effect-violation`。
  - defn! の第1引数（host）の型が obj なら、各 EffectCall のハンドラフィールドを effect 宣言シグネチャと unify で検査（`src/check.rs` `check_host_handlers`）。
  - do>bind の EffectCall は defn! 内のみ Result 要件を免除（§7.4 rule 5 の括弧書き「エフェクト呼び出しは P1 で例外的に許可」どおり）。純粋 defn では従来どおり type-mismatch も出る（t6 不変）。束縛値は interp と同じく「Ok なら中身、それ以外はそのまま」。
  - purity をモジュール間で伝播: `ModuleResult.defn_purity` を追加し import で merge（`src/check.rs`）。
- **インタープリタ** (`src/eval.rs`): EffectCall は host obj の `effect` フィールドのハンドラを apply（§12.4 と同じ host 経由）。host 未束縛・フィールド欠落は runtime error。
- **JS 生成** (`src/emit.rs`): defn! → `async function`、host は第1引数のまま渡る（§12.6「グローバル注入禁止」を維持）。EffectCall → `await host.<js_name(effect)>(args)`（§12.4）。async 文脈では App/EffectCall を `(await ...)` で包む（非同期でない値への await は恒等）。エフェクトまたは impure 呼び出しを含む lam のみ `async` arrow。do の EffectCall bind は nil-safe な Err 早期 return と Ok 時のみ `.value` 取り出し（interp と一致）。example は「モジュールが async を含むときだけ」`await` 付き（純粋モジュールの生成 JS はバイト無変更＝ money.js/cart.js ゴールデン不変）。トランポリンは async モジュールのみ `async $run`。末尾再帰の defn! は async `while(true)` ループで定数スタック（count-down 1e5 を node で実行成功）。
- **backend** (`src/backends.rs`): frontend の check cache を共有し、全モジュールの defn_purity から `impure_names` を `EmitInput` に渡す（import 先の defn! 呼び出しも await 対象）。

### 検証

- `cargo test` **201 全パス**（新規 +8: emit 3, effects 5, backend 1, diff 1。P0 時代の「defn! はエラー」系テスト 2 本を P1 挙動に書き換え）。warning 0。
- `LISTS_BACKEND=mock cargo test --test acceptance` 11/11（effect-run の diff は mock=interp 経由でも緑）。
- `bash scripts/lists-smoke.sh` **20/20**。
- `tests/fixtures/effect-run.lts`（新規: log!/db! を含む defn!、do 内エフェクト束縛、match、末尾再帰 count-down 1e5、example 3 本）を CLI `compile` → `node` で **3/3 ok**。生成 JS を 2 回コンパイルして一致（決定性）。interp vs JS の構造的一致は `diff::effect_run_diff_interp_vs_backend` で確認。
- 依存・unsafe・ライブラリ経路の unwrap/expect 追加なし（新規コードは unwrap_or_else のみ）。

### 残課題（差分提案。本マイルストーンでは SPEC 未変更・実装のみ）

1. defn! 第1引数の命名を `host` に強制する MUST 文言（現状は任意名で、その名前がそのまま JS の host 変数になる）。
2. host キーの命名: JS 側は obj フィールドと同じ `js_name`（kebab→snake_case。`db-write!` → `host.db_write`）。§12.3「効果名自体が host キー」を逐語にするなら `host["db-write"]` 形式への変更を提案。
3. example/top-level での EffectCall 禁止を明文化する文言（現状は実装が `effect-violation` を出すのみ）。
4. host の型が obj でない（型変数・別名など）場合にハンドラ検査をスキップする扱いの明文化。
5. async モジュールの example は top-level await に依存すること（ESM 前提。export/import を一切含まない純粋 async モジュールでは Node のモジュール構文検出に依存）。

---

## 11. M16 残課題の採用済み判断 (a)(b) の実装

背景: §8 の残課題「nth/find の Ok 包み vs (union T nil)」「split("") の空要素」。ユーザー確定の採用済み判断を SPEC 反映＋実装した。

- **(a) `(union T nil)` の実行時表現は生値 T（Ok/Err で包まない。nil は Nil）。MUST。**
  - SPEC: §11.6 に「(union T nil)（Option T）の実行時表現」bullet を追記し、`nth` bullet の `ok(arr[trunc(i)])` を `arr[trunc(i)]`（生値）へ修正。§0.3 変更点リストに row 4 として記録。
  - interp (`src/eval.rs`): `nth` 成功時に `Value::Ok` で包んでいたのを生値 `arr[i]` に変更。`find` も同様に生値 `x` へ変更（失敗は従来どおり `Value::Nil`）。
  - JS (`src/emit.rs`): 従来も `{ok}(arr[k])` / `{ok}(x)` で包んでいた（ユーザー想定「JS側は生値のはず」は未確認だったが実際は包んでいた）ため、差分テストで確認のうえ `arr[k]` / `x`（失敗 `null`）へ変更。結果として **tests/golden/cart.js の `$stdlib_find` 1行のみ**が `ok(x)` → `x` に変わった（UPDATE_SNAPSHOTS で再生成。他のゴールデンは無変更。money.js は find/nth/split を使わないため不変）。
  - 型（`src/stdlib.rs`）は従来から `(Option T)` = `(union T nil)` だったため、検査側の変更は不要。
- **(b) `split("")` は各 Unicode スカラ値文字を1要素とし、空文字列要素を含まない。MUST。**
  - SPEC: §11.6 に `split` bullet を追記（例: `split("a😀b","")` = `["a" "😀" "b"]`、`split("","")` = `[]`）。§0.3 に row 5 として記録。
  - interp: Rust の `str::split("")` は空要素を含むため不使用。空区切りのときのみ `chars()` 単位の分割に切り替え（非空区切りは従来どおり `split(t)`）。
  - JS: `s.split("")` は UTF-16 コード単位に分割されてしまう（絵文字が2要素になる）ため、空区切りのみ `[...s]`（コードポイント単位）に切り替え: `(s, t) => t === "" ? [...s] : s.split(t)`。非空区切りは不変。
- **テスト（追記のみ）**:
  - 新規 fixture `tests/fixtures/option-raw.lts`: nth の生値観測（`=> 20`。Ok 包みなら不一致で失敗）、範囲外・NaN は nil、find 成功/不発、split("") の空文字列・1文字・絵文字・非空区切りの example。
  - 新規 fixture `tests/fixtures/option-find.lts`: cart.lts の find 成功アーム（NegativeQty）を実行時に通す（place-order に qty=-1 の item を渡す）＋空カートの不発アーム。
  - `tests/acceptance/diff.rs` に `option_raw_and_split_diff_interp_vs_backend` を追記（interp vs js/mock で構造的一致）。
  - 単体: eval.rs に nth/find/split の生値・スカラ値単体テスト 3本、emit.rs に lowering 文のテスト 1本。
  - stdlib-edge.lts はスモーク S20 が件数 70 固定のため無変更（新規 example は新規 fixture 側に集約）。
- **検証**: `cargo test` 206全パス（+5）、`LISTS_BACKEND=mock` acceptance 12/12、スモーク 20/20、warning 0。CLI compile+node で option-find.lts は example 2/2 ok（NegativeQty アームが生値で通ることを確認）。
- **残課題**: なし（(a)(b) とも SPEC 反映済み。旧「Ok 包み」実装は跡形もなく除去済み）。

---

## 12. host DI とモック host（M21 基盤）

目的: M21（effect 実行）の基盤となる host DI を設計・実装。言語機能の追加は行わず（defn! 完全実装自体は M19 で済）。設計書は `design/host-di.md`。

- **設計（採用済み前提どおり）**: host は能力インターフェース。effect 宣言 `(effect name : sig)` の name→sig が実装へ 1対1 写像（interp: λ 関数値、将来 WASM: ホスト関数、JS: host オブジェクトのメソッド）。リソース系エフェクト（fuel 等）は effect 宣言を増やすだけで拡張され host 設計は不変。処理系本体は特定 host に依存せず、実装は外部から注入（§12.6 と同方針）。
- **Rust 型** (`src/host.rs` 新規): ラン時 host は `Value::Obj`（effect 名 → ハンドラ）。`host_type(effects)` が effect 宣言から host の obj 型を導出（defn! 第1引数の型と一致）。ハンドラは `BuiltinKind::HostMock { effect, default, log }`（`log` は serde skip。BuiltinKind から Copy/Eq を外した）またはユーザー Closure。
- **モック host**: `MockHost::from_effects` が effect ごとにハンドラを構築。呼び出しは `HostCall { effect, args }` を共有ログに記録し、戻りは `default_value(ret)`（nil→Nil, number→0, string→"", bool→false, Array→[], obj→各フィールド既定値, union→nil優先で先頭, Result→Err(既定E), fn 型→モックに含めない）。乱数・時刻を使わないので決定性 MUST を満たす。
- **注入点**: (1) CLI/REPL: `lists repl --host none|mock`（未知名は exit 2）。(2) interp: `host::inject_host(&env, "host", &obj)`。REPL は effect 宣言を含む eval/reload のたびに `refresh_host` でモックを再構築して `host` 全局へ注入し、検査側は `ModuleCache.host_bindings` に `host : host_type(effects)` を登録して既知の全局として扱う（`Checker::new` が読み込む）。(3) テストランナー: `inject_host` を呼ぶだけ（現状の example は host リテラル明示渡しのため必須でない）。新 REPL op `host-log` が記録を JSON Lines で返す。
- **JS 側は設計のみ**（M21へ引き継ぎ）: 生成 JS は host を defn! 第1引数のまま渡し `await host.<name>(args)`、グローバル host 宣言は禁止のまま。モックはテスト用ランナーが `host.mock.js` を差し込む形を提案（`design/host-di.md` §5）。差分提案として報告済み。
- **テスト**: host.rs 単体 4本（host_type、default_value の決定性、モックの記録＋固定応答＋反復決定性、fn 戻りのスキップ）、tools.rs に REPL 経由の注入確認 3本（`--host mock` で example がモック host 経由で通り host-log に記録される、未知 `--host` は exit 2、既定では host なし）。
- **検証**: `cargo test` 213全パス、`LISTS_BACKEND=mock` acceptance 12/12、スモーク 20/20、warning 0。SPEC.md・ゴールデン無変更（REPL の既存 op・ゴールデンは不変、host-log は追記のみ）。
- **M21 への引き継ぎ**: design/host-di.md §6（JS 側モック host とログ差分テスト、`lists run` への host 注入、fuel 系エフェクト追加時の拡張確認、`host` 命名衝突の明文化）。

---

## 13. M21: SPEC §7/§14 の P1 スコープ（effect 実行・defn!・do-io）を host DI 上に実装

### §7/§14 P1 スコープの確定（精査結果）
- §14 の P1 行「defn! 完全実装、effect、do-io、decimal、prop 検証、host DI、モック host」のうち、本マイルストーンで対象なのは **effect 実行 / defn! 完全実装 / do-io / host DI・モック host**。decimal と prop 検証（反復生成・縮小、M22 の自前乱数）は別マイルストーンで対象外。
- 範囲外（既定どおり）: 高階エフェクト多相（§7.5「高階エフェクト多相は実装しない」）、P2 の condition system 以降。
- エフェクト検査（effect-violation / unknown-effect）は P0 実装のまま無変更（§7.4 の規則は不変）。本マイルストーンで変えたのは実行系のみ。

### 実装
- **do-io（§3.5 の `(do e1 e2 ...)` 名なし束縛）**: `src/parser.rs` で do の非終端要素のうち `[...]` でないもの（`(...)` や原子）を名前なし束縛として `_` に糖衣する。束縛は文法どおり `[...]` のみとし、ListRaw に `bracket` フラグを追加して区別（従来は `(name expr)` も偶然束縛と解釈されていた。do-io 以降 `(f x)` は式）。desugar/check/eval/emit は既存の `_` 束縛をそのまま使うので、§7.4 rule 5（Result 要件。defn! 内の EffectCall は免除）が名なし束縛にもそのまま適用される。
- **effect 実行 / defn!**: M19 で実装済み（interp は host obj 経由、JS は `async function` + `await host.<name>(args)`、source map は map_at で維持）。本マイルストーンでは M20 の host DI 上で動くことを検証。
- **モック host の呼び出し記録の一致（差分）**: `tests/fixtures/effect-mock.lts`（export のみ）を JS 側は driver（モック host を注入して `await run(host, 1)`、結果と記録を印字）で node 実行、interp 側は `MockHost` を `host` に注入して `(run host 1)` を eval。結果 JSON と呼び出し記録 JSON が一致することを検証（`tests/backend.rs::mock_host_call_log_diff_interp_vs_node`。JS は 2 回実行して決定性も確認）。
- **REPL**: `--host` なしでも example が host リテラルを defn! 第1引数に渡せば eval/test で動くことを確認（`tests/tools.rs::repl_defn_bang_example_runs_with_explicit_host_literal`）。`--host mock` 経由は M20 のテストで確認済み。
- **ゴールデン追記**: `tests/golden/effect-run.js` / `effect-run.js.map` を新規追加（UPDATE_SNAPSHOTS で生成。既存ゴールデンは無変更）。source map の sources/mappings が維持されることもテストで確認。
- **スモーク追記**: S21「`repl --host mock` で defn! の example が pass し、host-log に呼び出しが記録される」を追加。合計は動的に `SMOKE: <pass>/<全件数>` で表示（S1–S20 は不変）。

### テスト追加
- parser: do-io の糖衣（`_` 束縛化）単体テスト
- fixtures: `do-io.lts`（エフェクト連続＋impure 早期 return の example）→ `diff::do_io_diff_interp_vs_backend`
- `effect-mock.lts` → モック host 記録一致テスト（backend.rs）
- backend.rs: effect-run の JS/マップ golden + source map 検証、モック host diff
- tools.rs: REPL で defn! example 実行（host リテラル明示渡し）

### 検証
- `cargo test` 218全パス、`LISTS_BACKEND=mock` acceptance 13/13、スモーク **21/21**（S21 追記。S1–S20 は従来どおり PASS）、warning 0。
- SPEC.md は本マイルストーンでは無変更（§3.5 の do-io 文言は既存の P1 宣言どおりに実装したため追記不要）。

### 残課題（差分提案）
1. do-io の名前なし束縛が §7.4 rule 5 の Result 要件を引き継ぐことの明文化（現状: `_` 束縛と同一に適用。defn! 内の EffectCall のみ免除。`(do (impure-non-result host) ...)` は type-mismatch）。
2. do の束縛が `[...]` のみであることの明文化（パーサは P0 で `(name expr)` も受理していたが、do-io との曖昧さ解消のため `[...]` のみを束縛とみなす実装に変更）。
3. JS 側モック host（`host.mock.js` 形状、design/host-di.md §5）の実装は次マイルストーンへ（今回は driver 注入で記録一致を検証）。

---

## 14. M22: prop 実評価（反例生成・縮小、fast-check 相当）

SPEC §10.1 の prop op を unsupported-in-p0 から実評価へ移した（ユーザー指示どおり。§10.1 を採用済み範囲で更新し、§13 T9 は本文無変更＋追記のみ）。

### 採用済み設計判断の実装
- **自前 PRNG**: `src/rng.rs` に splitmix64 を自前実装（依存追加なし・unsafe なし）。外部乱数源・システム時刻は使わない。
- **明示シード**: 既定はドキュメント化された定数 `rng::DEFAULT_SEED = 0x5EED_1A7C_2B21_9D4F`。REPL prop リクエストに `"seed": <u64>`、CLI に `lists repl --seed <u64>`（不正値は clap が exit 2）。**同じシード → 同じ生成列・同じ反例（決定性 MUST）** — テストで2回実行して応答が一致することを確認。

### 実装（src/prop.rs）
- **生成器** `gen_value(ty, aliases, rng, depth)`: number は境界値 {0,1,-1,±2^53} を半分の確率で選び、他は [-100,100] の整数。string は長さ 0..=8・文字種 `ab01 あ😀`(空白含む)。Array は長さ 0..=6（0 に偏らせる）。obj/union は再帰（union は tag を一様ランダム選択）。Result は Ok/Err 50/50。再帰深さ上限 MAX_GEN_DEPTH=6 では最小値（nil/空/0）へ。関数型・型変数は生成不可（`is_generatable` で検査時に type-mismatch）。
- **評価** `run_prop`: 既定 **100試行**（`DEFAULT_MAX_TRIALS`）。失敗（false または実行時エラー）で反例を記録し縮小へ。
- **縮小** `shrink_candidates` + 貪欲ループ: number は 0 と半分へ、string は空/半分/末尾削除へ、Array は空/末尾削除/先頭削除/半分＋要素ごと、bool は false へ、obj はフィールドごと。失敗し続ける候補のみ採用。
- **失敗出力** `prop_result_json`: 機械可読 JSON で `{ok:false, prop, seed, trials, counterexample:{param: value}, shrunk:{param: value}, fix:{op:"insert", path:"(prop <name>)", template:"(example <body with shrunk literals> => false)"}}`（`value_to_lit` は文字列を引用符つきで、`print_core`/`subst_params` で本体をリテラル埋め込み）。成功時は `{ok:true, prop, seed, trials}`。
- **検査**: forall パラメータが生成不可の型（関数等）なら check 時に type-mismatch。prop 本体の EffectCall は effect-violation（defn! 外のため host 無し）。
- **REPL**: prop op が実評価（seed はリクエスト優先、なければ REPL のシード）。未定義 prop は unbound-symbol。

### テスト
- rng 単体（同シード同列・範囲）、prop 単体（生成規則、縮小規則、真 prop pass、偽 prop で反例＋number→0/Array→[] へ縮小、同シード決定性、money.lts の prop が 100試行 pass、失敗 JSON の機械可読性と fix 形状）
- tools.rs: `repl_prop_money_lts_passes_with_seed`（reload money.lts → prop が seed=42・trials=100 で pass）、`repl_prop_false_prop_reports_counterexample_shrink_seed`（偽 prop の反例・shrunk x=0・seed・trials・fix template、同一シードで応答一致）、未定義 prop と不正 --seed
- golden: `repl.responses.json` の id:3 応答のみが unsupported-in-p0 → unbound-symbol（prop 未読込時の not-found）に変わったことを diff で確認。他のゴールデン無変更
- スモーク追記: S22（money.lts の prop が seed=42・100試行で pass）、S23（偽 prop に反例・縮小 x=0・シードが機械可読で返る）。S1–S21 不変、合計は動的表示

### 検証
- `cargo test` 229全パス、`LISTS_BACKEND=mock` acceptance 13/13、スモーク **23/23**、warning 0。

### 残課題（SPEC への追記「提案」— 本マイルストーンでは SPEC には採用済み部分（§10.1 の prop 実評価・シード意味論）のみ反映）
1. 生成規則の規範化（§3.7 または §11.6 追記案）: number の境界値集合と範囲、string の長さ上限と文字種、Array のサイズ分布、union の tag 選択分布、再帰深さ上限と最小値の規約、生成対象外の型（関数・型変数）の扱い。現状は実装どおり（上記）を規範案とする。
2. 試行回数の規範化: 既定 100（`DEFAULT_MAX_TRIALS`）を MUST/SHOULD で明記するか。
3. 失敗時出力の規範化（§10.1）: counterexample/shrunk/trials/seed/fix の各フィールドの機械可読性と、fix template を「そのまま example として貼れる形」にする規約（現状は body をリテラル埋め込みした `(example <body> => false)`。Match/Lam 等は `<expr>` にフォールバックすることの明記）。
4. 縮小アルゴリズムの規範化（どの程度まで縮小するか: 現状は貪欲に size が減る限り。最小反例の定義を詰めるか）。
5. GUIDE「乱数・システム時刻を使わない」の P1 での拡張案: 「非決定的な乱数を使わない（自前の決定的 PRNG と明示シードのみ）」への文言変更を GUIDE への追記提案として報告（本マイルストーンでは GUIDE 未変更）。
6. JS バックエンドでの prop 実行は未実装（REPL/interp のみ。§13 T9 は REPL 中心のため現状のまま）。差分テスト対象外。

---

## 15. decimal 設計ゲート（提案のみ・実装は採用指示待ち）

SPEC には「decimal は P1」（§8.6/§11/§14）としかないため、本マイルストーンは設計ゲートとし、**提案までで完了**とした。実装・テスト・SPEC反映はユーザーの採用指示を待つ。

- 提案書: `design/decimal.md`
- 推奨: **固定小数点（SCALE=18 固定、値 = スケール済み i128 整数）**。f64 の教訓（§0.3/§8.6）どおり「単一表現＋バックエンド差分一致」を最優先し、i128(Rust) / BigInt(JS) / i128(WASM) でビット一致する方式を選定。10進浮動小数点案は正規化の一致が難しいため見送り（動的レンジが必要なら後日別型で）
- リテラル `1.50d`（接尾辞 d。既存の `1.5` は number のまま後方互換）、型名 `decimal`、演算子 `+ - * / < <= > >= =` は同型どうしのみ（混在は type-mismatch＝暗黙変換なし）、明示変換 `to-dec`/`to-num` のみ
- 丸めは round-half-away-from-zero（trunc+剰余で i128/BigInt とも完全一致）、0 除算とオーバーフローは §8.6 のバックエンド中立な実行時エラーとして追加
- 印字は小数点以下 SCALE 桁固定の平書き（§11.6）、差分テストは新規 fixture decimal-edge.lts を既存の assert_diff（interp vs js/mock）＋ golden JS で検証する方針
- 未決: SCALE の値、接尾辞の文字、演算子多重定義 vs 専用演算子、オーバーフロー時エラー vs 飽和 —詳細は design/decimal.md §8

コード無変更のため `cargo test` 229/229、スモーク 23/23 は維持（確認済み）。

---

## 16. M23: decimal 実装（design/decimal.md の採用済み設計どおり）

ユーザーが design/decimal.md の推奨案を採用したため実装した。SPEC には採用済み設計を直接反映（§2 atom、§3.3 type、§8.6 decimal 段落＋中立エラー名、§11.1 演算子/to-dec/to-num、§11.6 印字、§12.4 lowering、§0.3 row 6、§14 注記）。

### 実装
- **表現**: 固定小数点 SCALE=18。実行時の値はスケール済み整数（interp: i128、JS: BigInt、WASM 将来: i128）で全バックエンドビット一致。`src/decimal.rs` に SCALE/S/DEC_MAX、パース（`parse_dec_scientific`）、印字（`fmt_dec`、SCALE 桁固定）、演算（`dec_add/sub` は checked、`dec_mul` は 256bit ワイド積＋half-away 除算、`dec_div` は 256bit 長除算＋half-away、範囲超過は `decimal-overflow`、0 除算は `decimal-division-by-zero`）、`dec_from_num`/`dec_to_num` を実装。
- **字句/構文**: `1.50d`（number 直後の `d` 接尾辞。`1.5` は number のまま後方互換）→ `TokenKind::Dec` → `LitValue::Dec` → `Value::Dec(i128)`。型名 `decimal`（Prim::Dec / Type::Dec）。
- **型検査**: `lit_type`/パターンリテラルに Dec。`+ - * / < <= > >=` は両引数 Dec なら Dec/Bool、両 number なら従来どおり、混在は type-mismatch（`check.rs` の App で特殊処理）。`=` は構造的等価（Dec は整数等価）。
- **interp**: 算術 builtin が (Num,Num)/(Dec,Dec) を振り分け、混在は runtime error。`to-dec`/`to-num` builtin 追加。
- **JS 生成**: decimal 使用モジュールのみ `DEC_JS` ヘルパ（`$dec_add/sub/mul/div`・`$print_dec`・`$to_dec`・`$json`）を出力し、`+ - * /` と `str` の stdlib を dual-mode 化（`typeof a === "bigint"` で振り分け）。純粋 number モジュールの出力は無変更（money.js 等のゴールデン不変）。リテラルは `1500000000000000000n`。example の直列化は `$json`（BigInt は JSON.stringify できないため `{tag:"Dec",value:"..."}` 形式）。runtime.js の structuralEqual に bigint 分岐（export は3つのまま）。
- **prop**: forall の decimal パラメータも生成可（境界 {0,±1,±1.0,±DEC_MAX} + 範囲内ランダム、縮小は 0 へ）。

### テスト
- decimal.rs 単体（パース/印字往復、丸め、オーバーフロー、変換の決定性）、lexer（`d` 接尾辞）、eval（四則・丸め・エラー・変換・混在エラー）、emit（BigInt lowering と純 number モジュールの無変更）
- `tests/fixtures/decimal-edge.lts`（18 example: 四則・half-away 丸め・比較・`=`・to-dec/to-num・印字・obj フィールド）→ `diff::decimal_edge_diff_interp_vs_backend`（interp vs js/mock で一致）
- golden 追記: `tests/golden/decimal-edge.js` / `.map`
- エラー名のバックエンド一致: `backend.rs::decimal_edge_js_golden_and_error_parity_with_interp`（0除算・オーバーフローで interp と node が同一文字列を出すことを検証）
- スモーク S24（decimal-edge.lts を compile+node で 18/18 ok、SCALE桁印字を確認）。S1–S23 不変、合計は動的表示

### 検証
- `cargo test` 238全パス、`LISTS_BACKEND=mock` acceptance 14/14、スモーク **24/24**、warning 0。既存ゴールデン（money.js/cart.js 等）無変更。

### 残課題
- 10進浮動小数点（動的レンジ）が必要になった場合は別型として追加検討（P5 付近）。現状の固定小数点の範囲は実値 ±1.7e20。
- JS バックエンドでの prop 実行は依然として未実装（REPL/interp のみ）。

---

## 17. P1 実用スモーク検査5のギャップ処置（host-log 文書化）と v0.3 版上げ

背景: 独立実用スモーク `lists-p1-smoke.sh`（25/26）の検査5で判明したギャップ: REPL `host-log` op が SPEC §10.1 に未記載、および P1 完了に伴う版上げが未実施。採用済み判断: (a) host-log の args は §11.6 の REPL 印字規約（`print_value`）による文字列形式のまま（MUST。数値 5 と文字列 "5" が同一記録 "5" になり得ることは既知の制約として条文に明記。型を保持する機械可読 JSON 値への変更は不採用）、(b) SPEC を v0.3 へ版上げ（§16 の規定）。**実装変更なし**（文書が実装に追いつく形。条文と実装の食い違いはなかったため差分提案もなし）。

- **SPEC §10.1**: op 一覧と入出力例に `host-log` を追記。応答形式 `{"id","ok":true,"host":"none"|"mock","calls":[{"effect":<effect名>,"args":[<印字文字列>,...]}]}`（MUST）、host 未注入（`--host none`/既定）時は `"host":"none"`・`"calls":[]`、args は §11.6 の REPL 印字規約（`print_value`）による文字列化で型を保持しないことと数値/文字列の同一表記の既知の制約、セッション内で呼び出し順に蓄積され決定的（同一入力→同一記録）（MUST）を明記。
- **SPEC §10.5**: CLI 一覧を `lists repl [--host none|mock] [--seed <u64>]` へ更新し、未知 host 名・不正 seed 値は exit 2 と明記（`--seed` は §10.1 に既述あり）。
- **v0.3 版上げ（§16）**: 表題・Status・Scope・§16 ・末尾「End of Specification」の版表記を v0.3 (P0+P1) へ更新。§0.3 を「v0.2 内への追補（据え置き）」から「v0.2→v0.3 の変更点」へ改題し、表の通し番号 1–6 と条項参照（§8.3/§8.6/§11.1/§11.6/§12.4/§12.5/§14）の整合を確認済み。本文中の「v0.2追補」参照は §0.3 表記へ整理。§0 冒頭は「P1 は §14 に記し v0.3 で実装済み」へ更新。
- **テスト（追記のみ）**: `tests/tools.rs::repl_host_log_matches_documented_shape_and_is_deterministic` を新規追加。`--host mock` で `log! "5"` と `tick! 5` を実行して host-log を2回要求し、応答が文書化した形式と一致すること（calls は effect/args、args は全要素が文字列。数値 5 と文字列 "5" はともに "5" と記録されることを観測）、2回目の host-log も同一内容（呼び出し順の蓄積）、同一入力の2回実行で stdout が一致（決定性 MUST）を検査。ゴールデン `tests/golden/repl.host-log.responses.json` を新規追加（UPDATE_SNAPSHOTS=1 で生成）。既存テスト・実装（`src/repl.rs` の `host_log`）は無変更。
- **検証**: `cargo test` 239全パス（+1）、warning 0、`bash scripts/lists-smoke.sh` 24/24。`lists-p1-smoke.sh` はリポジトリ外の独立スクリプトのため本環境では実行不可（検査5該当の host-log 文書化と v0.3 版上げは本変更で解消）。
- **残課題**: README.md / GUIDE.md 内の「SPEC v0.2」表記は本マイルストーンでは据え置き（SPEC のみ版上げ）。別途更新する。

---

## 18. interp-capability §1.1（C1 map/set）の実装判断（design/interp-capability.md 採用時。SPEC.md は未変更）

対象: design/interp-capability.md §1.1（`(map K V)` / `(set K)`、stdlib 11 関数 + `obj->map`）。条文は SPEC §3.3（型構文）§6（型システム）§8.2/§8.3（値・等価）§9.3（fix）§11（標準ライブラリ）§12.4/§12.5（JS lowering/runtime）に写像して実装。

### 実装
- **型**: `Type::Map(K,V)` / `Type::Set(K)` を追加。 surface 構文は design 通り小文字 `(map K V)` / `(set K)`（parser/desugar/fmt/names に対応）。print は `(map K V)` / `(set K)`。union 正規化（§6.2 の正規表現バイト列ソート）は print 経由で自動対応、unify は K・V をそれぞれ unify（§6.4）。obj との部分型関係は持たせない（別 discriminant → unify 失敗。obj との相互変換は `obj->map` のみ単方向）。
- **キー制約**: design は `str/sym/int` だが現 SPEC には sym/int 型が存在しないため **string（リテラル含む）/ number のみ許可**（ギャップは下記参照）。違反は defn/lam/alias/effect/prop の型注釈すべてで type-mismatch + fix テンプレート（型中の map/set キーをすべて string に置換したもの。例: `(map bool number)` → `(map string number)`）（§9.3: type-mismatch には fix MUST）。実行時も map/set キーは str/num 以外を runtime error。
- **値（interp）**: `Value::Map(Rc<PersistentMap>)` / `Value::Set(Rc<PersistentMap>)`。`src/pmap.rs` の自前永続 AVL 木（キーで平衡、挿入順は各ノードの seq で保持）。更新は path-copying で新しい木を返し既存の共有構造を壊さない（design §1.1「永続構造」）。O(log n) 更新・検索。number キーは JS Map の SameValueZero と一致させるため正規化（-0→+0、NaN→正規ビット）。
- **等価（§8.3）**: map はキー集合と各値の再帰的等価で決定、**挿入順は無視**。set はキー集合のみ。map と set は別の値種（`=` では不一致、型検査でも unify されない）。循環は構築不可能（不変値のみ）だが既存の path-based 検出も map/set に適用。
- **stdlib**（純粋関数として scheme 登録）: map-empty / map-get / map-set / map-del / map-keys / map-has? / map-size / set-empty / set-add / set-has? / set->array。map-keys / set->array は挿入順で返す（決定性）。map-get の失敗は `(err (obj (tag "not-found")))`、型は `(Result V (obj (tag "not-found")))`（design の `(result V not-found)` を現 SPEC の Result + obj tag 方式へ写像）。
- **obj->map**: 単方向のみ。obj（固定キー構造体型）は存続。戻り値型は obj フィールド値型の union を値型とする `(map string (union ...))` で、check.rs の App 直下の特殊処理で組み立てる（rank-1 多相では「フィールド値の union」を直接表現できないため）。高階位置に渡した場合の fallback scheme は `(fn (T) : (map string U))`（値型 U は文脈で unify）。実行時は obj の BTreeMap キー順（UTF-8 バイト順）= ソート順で挿入。
- **JS バックエンド（§12.4/§12.5）**: map は内部 `Map`（design §7 の写像表通り）。**set は内部 `Set`**（design §7 の「set も Map」からの逸脱: map と set を `=` で区別するために Set が必要。型検査上は比較し得ないが値レベルで区別を維持）。挿入順反復は JS Map/Set の挿入順で interp と一致（上書きは元の順序を維持 = seq に相当）。構造的等価は runtime.js の structuralEqual に Map/Set 分岐を追加（deep-equal、順序無視、WeakMap で循環検出）。obj->map はキーを codepoint 順にソートしてから Map に入れる（Rust の BTreeMap と一致）。
- **印字（§11.6 は map/set を未定義のため新規決定）**: 挿入順の s 式 `(map (k v) ...)` / `(set k ...)`、空は `(map)` / `(set)`。interp `print_value` と JS `$print_m` で同一形式。
- **JSON（example 出力・機械可読性）**: `{"tag":"Map","entries":[[k,v],...]}` / `{"tag":"Set","items":[...]}`（挿入順）。JS 側は map 系使用時のみ `$mjson` を出力（既存の DEC_JS/PRINT_JS は無改変）、Rust 側は `backends.rs::value_to_json`、テストの `json_to_value` にも対応。
- **lexer**: 名前中の `->`（`obj->map`、`set->array` 等）を単一シンボルとして許可するよう scan_name を拡張。単独の `->` は従来通りパイプ（`is_symbol_start('-')==false` なので不変）。`js_name` は `->` → `_to_`（§12.3 拡張）。
- **既存出力への影響**: map/set 系 stdlib を使わないプログラムのコンパイル出力はバイト不変（golden テストで検証）。map 使用時のみ `$mjson`/`$print_m`/`$obj_to_map` を追加出力し、2 回コンパイルでバイト一致を確認。

### テスト追加
- `tests/fixtures/map-edge.lts`（41 example: 基本操作・挿入順・上書き順序維持・map-del・number キー/-0・set・構造的等価（順序無視）・永続性・obj->map・印字・2000 要素の末尾再帰 fold（§8.5）＋ `diff::map_edge_diff_interp_vs_backend`（interp vs compile+node で全 example 一致）
- `acceptance::map_set_key_type_violation_is_type_mismatch_with_fix`（キー型違反の type-mismatch + fix テンプレート、string/number/alias キーの許可）
- `acceptance::obj_to_map_infers_union_value_type`（obj->map の union 値型・非 obj への適用は type-mismatch）
- 単体: pmap.rs（AVL 永続性・挿入順・-0/NaN 正規化・serde 往復）、types.rs（print/normalize/structural_equal）、unify.rs（map/set unify と occurs）、value.rs（map/set 等価・印字）、lexer.rs（`->` in name）、emit.rs（js_name）、fmt.rs（map/set 型の冪等・AST 保存。map-edge.lts を fixture 一覧に追加）

### 検証
- `cargo test` 254 全パス（既存 239 から +15、壊れなし）。warning 0。
- `lists compile map-edge.lts` を 2 回実行し出力ディレクトリがバイト一致（決定的）。node で 41/41 example ok。
- `lists fmt` in-place を 2 回適用しても安定（冪等）。golden（cart.js 等）は無改変でパス。
- `LISTS_BACKEND=mock` でも acceptance パス（t11 は既存 fixture のみ）。

### ギャップ・残課題（SPEC には報告のみ。独断で直していない）
1. design §1.1 のキー型 `str/sym/int`: 現 SPEC（v0.3）には `sym` も `int`（i64）も存在しない（i64 は §14 P5）。実装では string/number のみ許可し、sym/int は将来の P5/C2/C4 で吸収する方針。
2. design の `int` 戻り値（map-size 等）は現 SPEC では `number` のみのため `number` で実装。
3. design の `(result V not-found)` は現 SPEC の構文 `(Result V (obj (tag "not-found")))` に写像。判断 kind 名は tag リテラルとして表現。
4. design §7 の写像表「set は内部 Map」からは逸脱し JS では `Set` を使用（map/set の `=` 区別のため）。意味論は design 通り（挿入順・deep-equal）。
5. SPEC §3.3 の型コンストラクタ命名規則（Array/Result/Option は大文字、union/obj/fn は小文字）と design §1.1 の小文字 `map`/`set` の混在は、SPEC 側の統一が未決定のため design に従った。SPEC 改訂時に統一を検討。
6. SPEC §11.6 の印字規約は map/set を未定義のため、挿入順 s 式で新規決定（上記）。SPEC 改訂時に追記候補。
7. unify 後（型変数が具体化された後）の map/set キー制約の再検証は未実施。現状は注釈時の検証＋実行時エラーで担保。

---

## 19. interp-capability §1.2/§3.1（C2 char + 文字列プリミティブ）の実装判断（SPEC.md は未変更）

対象: design/interp-capability.md §1.2（char 型・`\<文字>` リテラル・char-code）と §3.1（位置ベース文字列 API）。条文は SPEC §2（字句）§3.3（型構文）§3.6（パターン）§6（型）§8.3（等価）§11.6（印字）§12.4/§12.5（JS）に写像。

### 実装
- **型/値**: `Type::Char` / `Value::Char(char)`。surface 型は `char` プリム。サロゲートは存在しない（`char::from_u32` が拒否。lexer の `\uD800` 等もエラー）。
- **リテラル**: `\<文字>` を lexer に追加。エスケープ規則は str リテラルと共通（`\" \\ \n \t \r \uXXXX`、いずれも単一スカラー値へ）。fmt は `print_char_lit` で逆写像（制御文字は `\uXXXX`）し冪等・AST 保存を確認。
- **lexer の名前規則拡張**: 名前中の `->`（既存）に加え、演算子接尾辞 `<=? >=? =? <? >?` をシンボルの一部として許可（§3.1 の `char<=?` / `char=?` のため）。単独の `<` `>` `=` `->` は従来通り Punct（演算子・パイプ）のまま。`js_name` はこれらを `_le_`/`_ge_`/`_lt_`/`_gt_`/`_eq_`/`_to_` へ無害化（既存名には影響なし）。
- **stdlib**（§3.1 を現 SPEC 型へ写像。`str`→string、`int`→number（i64 は P5）、`result`→`(Result T (obj (tag ...)))`）:
  - `char-at : (fn (string number) : (Result char (obj (tag "out-of-range"))))` — 添字はスカラー値単位・小数は切り捨て（nth と同じ §11.6）、NaN/負/範囲外は err out-of-range
  - `char-code : (fn (char) : number)`（スカラー値）、`code->char : (fn (number) : (Result char (obj (tag "invalid-code"))))` — 非整数・範囲外・サロゲートは err invalid-code
  - `str-take-while : (fn (string number (fn (char) : bool)) : (obj (acc : string) (pos : number)))` — 開始位置は slice_bounds と同じクランプ（NaN→0・負は len+x・超過は len）
  - `str-skip-while : (fn (string number (fn (char) : bool)) : number)` — 位置のみ返す（部分文字列を作らない設計）
  - `str-split-at : (fn (string number) : (Result (obj (left : string) (right : string)) (obj (tag "out-of-range"))))` — 0..len の範囲のみ ok、NaN/負/超過は err
  - `str-contains? : (fn (string string) : bool)`（contains? と同じ）
  - `char<=? char=? : (fn (char char) : bool)`、`char-alphabetic? char-numeric? char-whitespace? : (fn (char) : bool)`
- **述語の意味論（interp/JS 一致）**: alphabetic = Unicode `Alphabetic`（Rust `char::is_alphabetic` = JS `/\p{Alphabetic}/u`）、numeric = カテゴリ N（`is_numeric` = `/\p{N}/u`）、whitespace = `White_Space`（`is_whitespace` = `/\p{White_Space}/u`）。`char<=?` はコードポイント（スカラー値）比較（JS の文字列 `<=` は UTF-16 コード単位順なので `codePointAt` で比較。U+1F600 vs U+FFFF で差が出ることをテストで確認）。
- **等価・パターン**: `(= \a \a)` は構造的等価（Char 同士のみ。char と string は unify されない）。match の char リテラルパターンを追加（網羅性検査は char 型全体を1ケースとして扱うため `_` が必要。既存の number リテラルと同じくリテラル1つでは型全体をカバーしない）
- **印字（§11.6）**: char は string と同じく**生文字**（引用符・エスケープなし）で印字。リテラル構文のみ `\<文字>`。JS 側で char = 1要素文字列なので `$print` でも同一バイト列になる（`str \a` → `"a"`）。
- **JS バックエンド（§7）**: char は1要素文字列（コードポイント単位）。`char-at` は `[...s][k]`、`char-code` は `codePointAt(0)`、`code->char` は `String.fromCodePoint`＋範囲検査。リテラルは通常の JS 文字列リテラルへ lowering。
- **JSON**: char は素の JSON 文字列として直列化（JS 側と同一。char と string は JSON 上区別されないため、example の actual に生の char を出さない方針で fixture を設計）。

### テスト追加
- `tests/fixtures/char-edge.lts`（63 example: リテラル/述語/スカラー値境界（U+1F600・U+FFFF）・code->char の invalid-code 往復（サロゲート 55296/57343・U+110000・負・小数・NaN）・char-at の out-of-range ・位置ベース API の簡単なレキサー（空白スキップ+数字列読み取り。絵文字またぎ）str-split-at・str-contains?・char を含む union と match・印字）→ `diff::char_edge_diff_interp_vs_backend`（interp vs compile+node で全一致）
- 単体: lexer（char リテラル・サロゲート拒否・演算子接尾辞名）、fmt（char リテラル往復・冪等・AST 保存。char-edge.lts を fixture 一覧へ）、types/unify（char）、js_name

### 検証
- `cargo test` 257 全パス（254 + 3）、warning 0。`LISTS_BACKEND=mock` でも acceptance 18/18。
- `lists compile char-edge.lts` を2回実行しバイト一致、node で 63/63 example ok。`lists fmt` in-place 2回で安定。golden 無改変。`git diff -- SPEC.md` 空。

### ギャップ・残課題（SPEC には報告のみ）
1. design の `int`（char-code の戻り値など）は現 SPEC では `number` のみ（i64 は §14 P5）のため number で実装。char-code の値は 0..0x10FFFF なので f64 で正確に表現できる。
2. design の `(result char out-of-range)` は `(Result char (obj (tag "out-of-range")))` に写像（map-get の not-found と同じ方式）。
3. char の印字は §11.6 に未定義のため「string と同じく生文字」と決定（JS の 1要素文字列表現と一致させるため）。`\<文字>` はリテラル構文のみ。SPEC 改訂時に追記候補。
4. char-at の小数添字は切り捨て（nth と同じ §11.6 规則）。str-split-at は範囲外を err とする（slice_bounds のクランプとは異なる。位置ベース API は「失敗を result で返す」設計のため）。開始位置のクランプのみ slice_bounds と共通。
5. code->char は非整数を invalid-code とする（往復 code->char(char-code c)=ok c を優先）。trunc 方針もあり得たが記録のみ。
6. JS で char（1要素文字列）と string は実行時同一なので、`=` は型検査でのみ区別（char vs string は unify 失敗）。JSON 直列化でも区別されない（上記）。
7. 網羅性検査: char/number のリテラルパターン1つでは型全体をカバーしない（既存の P0 単純アルゴリズム §6.5 のまま）。char リテラルも同様。

---

## 20. interp-capability §3.2（builder）/ §2.1（letrec）/ §1.5（ref）の実装判断（SPEC.md は未変更）

### builder（§3.2 C3）
- `Type::Builder` / `Value::Builder(Rc<Vec<String>>)`。stdlib: builder-empty / builder-push / builder->str / builder-join（`(fn ((Array string) string) : string)`。design の `str` は string へ写像）。
- 不変値: push は新しい builder を返す。実装は `Rc::make_mut` で「共有されていないときは in-place、共有時のみ複製」とし、線形利用では償却 O(1)（10^4 要素の畳み込みテストで確認）。JS 側は配列 `[...b, s]` + join（design §7 通り）。
- 印字: JS 側が配列なので Rust も配列と同じ `[a b c]` 形式で統一（§11.6 に builder は未定義のための新規決定。REPORT 記録）。等価は要素列の構造等価。
- JSON: 配列として直列化（JS と同一。example の actual に生の builder は出さない方針）。

### letrec（§2.1 C5）
- 構文 `(letrec [name lam-expr] body...)`（parser/ast/desugar/fmt/names）。body が複数ある場合は let _ の連鎖に脱糖し最後の式が値。
- Core IR に `Core::Letrec { name, lam, body }` を追加。束縛対象が Lam でない場合は check で type-mismatch + fix（`(fn () : nil <expr>)` で包むテンプレート、§9.3）。
- 型付け: Lam は注釈から関数型が分かるので、lam 本体の検査前に束縛名へその型を入れてから検査（自己参照が fn 型で検査できる）。
- 評価: 自己参照環境を作り named closure を束縛。JS 生成は名前付き関数式 `const f = function f(...) {...}`（design §2.1 の直接写像）。letrec 内の自己末尾呼び出しは defn のような self-loop 最適化は行わず通常の再帰（深い letrec 再帰は JS でスタック制限あり。残課題として記録）。
- **§8.5 との相互作用（ギャップ報告）**: SPEC §8.5 の末尾位置リストに Letrec は未記載。Letrec.body を Let.body と同じく末尾位置として実装（collect_tail_callees で in_tail 伝播）。SPEC への追記は行わず報告のみ。
- 相互再帰（2つ以上の letrec 束縛の相互参照）は design 通り対象外（単一束縛のため外側の束縛名は内側 lam から見えない）。

### ref（§1.5 C6）
- `Type::Ref(Box<Type>)` / `Value::Ref(Rc<RefCell<Value>>)`。surface 型 `(ref T)`。
- ref! / ref-get! / ref-set! は組み込みエフェクトとして EffectCall 経由で実装（effect 名 ref / ref-get / ref-set に組み込み scheme を登録、check で instantiate）。純粋 defn から呼ぶと既存のエフェクト検査により effect-violation + fix（defn→defn!）が自動で出る（テスト済み）。defn! の host ハンドラ検査は組み込みエフェクトをスキップ（host 能力ではなく言語内完結）。
- design の `unit` 戻り値は現 SPEC に unit 型が無いため `nil` に写像（記録）。
- `=` は参照等価（Rust は `Rc::ptr_eq`、JS は `{value}` オブジェクトに非列挙マーカ `__lists_ref__` を付けて structuralEqual で `a === b`）。設計 §1.5 の「循環 `=` は存在しない」は ref-set! で自己参照セルが作れるため反例あり。ただし参照等価なので循環検出は不要で両バックエンド一致。
- JS 写像は design §7 通り `{ value }` オブジェクト（+ 非列挙マーカ）。ref の印字は Rust `<ref>`（JS 側の $print は `{value: ...}` となる諸元あり。example では ref を直接 str しない方針で回避。残課題として記録）。
- ref! は defn! の do-bind 内で Result でなく生の値として束縛できる（既存の §7.4 rule 5 の仕組みをそのまま利用）。

### テスト追加
- `tests/fixtures/letrec-ref-builder.lts`（17 example: letrec 自己再帰 fact・親環境クロージャ make-adder・複数 body、builder 基本/不変性/印字/10^4 畳み込み len=38890、ref の set/get 往復・参照等価 self-eq/diff-ref・共有セル share）→ `diff::letrec_ref_builder_diff_interp_vs_backend`（interp vs compile+node 全一致、mock でも一致）
- `acceptance::letrec_non_lam_binding_is_type_mismatch_with_fix`、`acceptance::ref_builtin_effect_violation_in_pure_defn_and_ok_in_defn_bang`
- fmt: `fmt_letrec_and_ref_roundtrip` + fixture 一覧に letrec-ref-builder.lts 追加（冪等・AST 保存）

### 検証
- `cargo test` 261 全パス（257 + 4）、warning 0。`LISTS_BACKEND=mock` acceptance 21/21。
- `lists compile letrec-ref-builder.lts` を2回実行しバイト一致、node で 17/17 example ok。`lists fmt` in-place 2回で安定。golden 無改変。`git diff -- SPEC.md` 空。

### ギャップ・残課題（SPEC には報告のみ）
1. §8.5 の末尾位置リストに Letrec.body が未記載 → Let.body と同じく末尾位置として実装（報告のみ、SPEC 未変更）。
2. design の `unit`（ref-set! の戻り値）は現 SPEC に無いため nil に写像。
3. letrec の自己末尾呼び出しは JS で self-loop 最適化せず通常再帰（defn の §8.5 最適化との差。深さに注意）。
4. ref の印字: Rust `<ref>` / JS `{value: ...}` の差（$print は golden 都合で無改変のため）。`=` は参照等価で一致。
5. 相互再帰 letrec は単一束縛のため不可（design §2.1「対象外」に一致）。

---

## 21. interp-capability §4（C9 host エフェクト now!/elapsed!/trace!）の実装判断（SPEC.md は未変更）

対象: design/interp-capability.md §4。**read-file! は bytes 型が未実装（M29）のため本マイルストーンから除外し M29 に回した**（プロンプトの指示通り）。

### 実装
- `host.rs::BUILTIN_HOST_EFFECTS = ["now", "elapsed", "trace"]` を host 能力インターフェースの組み込みとして追加（read-file! は M29 後）。`effects.rs::builtin_effect_scheme` に言語側宣言済みのスキームを追加: `now! : (fn () : number)`、`elapsed! : (fn () : number)`（design の `(-> unit num)`。unit は現 SPEC に無いので 0 引数 fn に写像）、`trace! : (fn (string) : nil)`（unit→nil）。
- エフェクト検査: 宣言なしの呼び出しは unknown-effect（既存どおり）、純粋 defn からの呼び出しは effect-violation + fix `defn!`（既存どおり。組み込みエフェクトも EffectCall なので自動）。defn! の host ハンドラ検査は組み込みエフェクトをスキップ（`is_builtin_effect` で ref 系と統合）。
- interp（モック）: `now!` は固定列 `MOCK_NOW_BASE=1700000000000 + 呼び出し回数*1000`（Interp 内クロック、同一入力で同一出力=決定的）、`elapsed!` は `now - 初回 now`、`trace!` は `Interp.host_log`（`HostCall{effect,args}` 形式）へ蓄積し nil を返す。repl --host mock では `Interp.host_log` をモック host のログと共有するため、**host-log op に trace! も載る**（host-log は trace! の糖衣として存続。既存 golden 不変）。
- JS バックエンド: `now!` → `Date.now()`、`elapsed!` → `Date.now() - $BOOT_NOW`（モジュール先頭で1度だけ評価する `$BOOT_NOW` を clock 使用時のみ出力）、`trace!` → `(console.error(s), null)`（console.error は undefined を返すので do の Err 検査で壊れないよう null にする。stderr に出るので example の stdout には影響しない）。
- docs/backends.md に写像表と「read-file!（M29）で初めて Node `fs` 依存が生じる」旨を明記。現状 now!/elapsed!/trace! は Date.now/console.error のみで Node 固有 API 不要。

### テスト追加
- `tests/fixtures/hostfx.lts`（trace! のみ差分安全: run-trace/twice-trace）→ `diff::hostfx_trace_diff_interp_vs_backend`（interp vs compile+node で一致。stderr の trace は stdout 比較に影響しない）
- `acceptance::now_elapsed_bench_is_deterministic_and_trace_is_logged`: ベンチ関数（now!→trace!→now!→elapsed!）が interp で決定的（`(ok 1000)`）＆同一入力の再実行で同一出力＆trace! が effect/args 形式で記録されること
- `acceptance::builtin_host_effect_diagnostics`: 未宣言 `foo!` → unknown-effect、純粋 defn で `trace!` → effect-violation + fix `defn!`、defn! で now!/trace! はエラーなし
- fmt fixture に hostfx.lts 追加（冪等）

### 検証
- `cargo test` 264 全パス（261 + 3）、warning 0。`LISTS_BACKEND=mock` acceptance 24/24。
- `lists compile hostfx.lts` を2回実行しバイト一致、node で 3/3 example ok（trace は stderr）。`lists fmt` in-place 2回で安定。golden（repl.host-log 含む）無改変。`git diff -- SPEC.md` 空。

### ギャップ・残課題（SPEC には報告のみ）
1. read-file! / read_file: bytes 型未実装（M29）のため除外。M29 で `(effect read-file! : (fn (string) : (Result bytes (obj (tag "io-error")))))` を追加し、JS は Node fs 依存となる（docs/backends.md に明記済み）。
2. design の `(-> unit num)` は現 SPEC に unit が無いため `(fn () : number)` に写像。同様に trace! の unit → nil。
3. now!/elapsed! は JS（Date.now）が非決定的のため差分テスト対象外。interp のみ決定性テスト。
4. モック now の固定列は呼び出し回数ベース（シード非依存）。「固定列 or シード由来」のうち固定列を採用。

---

## 22. interp-capability §1.4（C8 bytes）+ §4.1（read-file!）の実装判断（SPEC.md は未変更）

### bytes（§1.4 C8）
- `Type::Bytes` / `Value::Bytes(Rc<Vec<u8>>)`。surface 型プリム `bytes`。不変バイト列（変更操作は stdlib に存在しない）。
- stdlib: `bytes-get : (fn (bytes number) : (Option number))`（nth と同じく小数添字は切り捨て・範囲外は nil。design は戻り値型を明記しないため配列アクセスの nth に合わせた）、`bytes-len : (fn (bytes) : number)`、`bytes-slice : (fn (bytes number number) : bytes)`（slice と同じ slice_bounds クランプ）、`str->bytes : (fn (string) : bytes)`（UTF-8、常に成功）、`bytes->str : (fn (bytes) : (Result string (obj (tag "invalid-utf8"))))`、`str->codepoints : (fn (string) : (Array number))`、`codepoints->str : (fn ((Array number)) : (Result string (obj (tag "invalid-code"))))`（各要素は整数・0..0x10FFFF・サロゲート以外）。design の `int` は number に写像（i64 は P5）。
- `=` はバイト列の構造等価。循環は構築不可。
- **印字規約（新規決定。§11.6 に bytes は未定義）**: `#u8[1 2 3]` 形式（空は `#u8[]`）。Rust `print_value` と JS `$print`（Uint8Array 分岐を PRINT_JS に追加）で同一バイト列。PRINT_JS 変更に伴い golden の decimal-edge.js / effect-run.js は $print 1行追加のみ（UPDATE_SNAPSHOTS で再生成し diff を確認）。
- **JS 写像**: `Uint8Array`。`str->bytes` → `TextEncoder().encode`、`bytes->str` → `TextDecoder("utf-8", {fatal:true})`（不正なら throw → err invalid-utf8。Rust の from_utf8 と一致）、`bytes-slice` → `slice`（slice_bounds 共有）、codepoints は codePointAt/fromCodePoint。
- JSON: bytes は数値配列として直列化（$mjson に Uint8Array 分岐追加、value_to_json も同一）。example の actual に生の bytes は出さない方針（char と同じく JSON 上は配列と区別されないため）。

### read-file!（§4.1。M28 で bytes 未実装のため M29=本マイルストーンに回した）
- `(effect read-file! : (fn (string) : (Result bytes (obj (tag "io-error")))))` を組み込みエフェクトとして登録（`is_builtin_effect` に "read-file" 追加。effect 宣言不要、host ハンドラ検査スキップ、純粋 defn からは effect-violation + fix）。
- interp/モック: `Interp.virtual_files`（登録された仮想ファイル）から読み `(ok bytes)`、無ければ `(err (obj (tag "io-error")))`。
- JS: `import * as $fs from "node:fs/promises"` + `$read_file`（read-file! 使用時のみ出力）。**これが初めての Node 固有 API 依存**（docs/backends.md に明記済み）。差分テストからは除外（FS は非決定的）。

### テスト追加
- `tests/fixtures/bytes-edge.lts`（30 example: 基本操作・UTF-8 往復（絵文字=4バイト）・invalid-utf8（絵文字途中バイト等）・codepoints とサロゲート境界 invalid-code・#u8 印字・等価）→ `diff::bytes_edge_diff_interp_vs_backend`（interp vs compile+node 全一致、mock でも一致）
- `acceptance::read_file_effect_reads_virtual_files_and_io_error_on_missing`（仮想ファイルからの読み取り・missing で io-error・純粋 defn での effect-violation・JS 出力が node:fs/promises を含むこと）
- fmt fixture に bytes-edge.lts 追加（冪等）

### 検証
- `cargo test` 266 全パス（264 + 2）、warning 0。`LISTS_BACKEND=mock` acceptance 26/26。
- `lists compile bytes-edge.lts` を2回実行しバイト一致、node で 30/30 example ok。`lists fmt` in-place 2回で安定。golden は $print 1行追加のみ（意図的変更、diff 確認済み）。`git diff -- SPEC.md` 空。

### ギャップ・残課題（SPEC には報告のみ）
1. bytes の印字規約 `#u8[...]` は §11.6 に未定義のため新規決定（プロンプトの例示形式を採用）。SPEC 改訂時に追記候補。
2. bytes-get の戻り値型は design が明記しないため nth と同じ `(Option number)` に決定（範囲外は nil）。char-at の `Result out-of-range` とは流儀が異なる（配列=Option、位置ベース=Result）点を記録。
3. bytes にはソース上のリテラル構文が無い（prop の counterexample 印字は print_value 形式）。
4. read-file! の JS 実行は差分テスト対象外（Node fs）。interp のみ仮想ファイルで決定的にテスト。
5. $json（decimal時）の Uint8Array 分岐は $mjson のみに追加。decimal と bytes を同時に example 出力する組合せは未対応（実害なし、記録のみ）。

---

## 23. M22 prop の JS バックエンド実行（SPEC.md は未変更）

### 方針（runtime.js の export 3つを崩さない）
- SPEC §12.5 は runtime.js の export を ok/err/structuralEqual の3つのまま増やさない（MUST）。よって prop 用ランタイムは**別モジュール `src/backend/prop_runtime.js`** とし、prop を含む生成 .js のみが `import { runProps } from "./prop-runtime.js"` し、JsBackend はそのときだけ out_dir に prop-runtime.js を併置する（EmitOutput.uses_prop）。runtime.js は無改変。

### 実装
- **RNG**: src/rng.rs の splitmix64 を BigInt でビット一致に写像。DEFAULT_SEED=0x5EED1A7C2B219D4F は共通。同じ seed → 同じ列。
- **codegen**: prop 本体を `$prop_<i>` 関数として出力し `$PROPS` に登録（型ディスクリプタ JSON = alias 展開済み、本体 Core は serde JSON で埋め込み）。末尾に `const $SEED = <seed>n; await runProps($PROPS, $SEED, 100);`。結果は stdout に `prop <i>: ok <json>` / `prop <i>: FAIL <json>`。
- **生成・縮小**: gen_value/min_value/value_size/shrink_candidates を写像（境界値 NUM_BOUNDARIES・STR_CHARSET・試行100回・深さ6・縮小順序すべて interp と同一。文字列長比較は TextEncoder でバイト長に合わせた）。
- **結果 JSON のバイト一致**: オブジェクトキーは serde_json と同じくソート（BTreeMap 既定）。数値は ryu 互換に合わせ整数 float に `.0`、NaN/±Inf は null（value_to_json と同じ）。seed は u64 なので BigInt のまま桁落ちなく直列化。id/trials は整数のまま。fix テンプレートは埋め込み core JSON 上で subst_params/print_core を写像（serde 形 Value は平坦化してから value_to_lit）。
- **--seed**: `lists compile --seed N` を追加し Backend::compile に seed を通し EmitInput.seed → `$SEED` に埋め込む（既定 DEFAULT_SEED）。REPL は既存の --seed / prop リクエスト seed をそのまま使う（継承確認済みテスト）。

### テスト追加
- `tests/fixtures/props-edge.lts`（pass 3 + fail 1）→ `diff::prop_diff_interp_vs_node_same_seed_byte_identical`: seed=0xDEADBEEF12345678 で interp の prop_result_json と node の `prop i:` 行の JSON が**同一バイト列**（反例 {"x":1.0}・縮小 {"x":0.0}・seed・fix template を含む）、かつ同一 seed の再実行で同一 stdout（決定性）
- `tools.rs::cli_compile_seed_is_embedded_into_codegen`: --seed 42 で `const $SEED = 42n;` が埋め込まれ prop-runtime.js が配置されること、既定は DEFAULT_SEED
- fmt fixture に props-edge.lts 追加

### 検証
- `cargo test` 268 全パス（266 + 2）、warning 0。`LISTS_BACKEND=mock` acceptance 27/27。
- golden: money.js のみ prop 追加分（import/$prop_0/$SEED/$PROPS/runProps）が変わる意図的変更。それ以外の golden 無改変。compile を2回実行しバイト一致。fmt in-place 冪等。`git diff -- SPEC.md` 空。

### ギャップ・残課題（SPEC には報告のみ）
1. prop の fix テンプレート（`(example ... => false)`）は埋め込み core JSON 経由の写像。Match/Do 等のフォームは print_core のフォールバック `<expr>` となる（interp と同じ）。
2. 数値の JSON 形式は ryu/serde_json 互換をテストで担保（整数 float の `.0`、NaN/Inf→null）。指数表記（1e21 系）の ryu と V8 の差は未対応（テスト範囲外）。
3. prop 本体が defn! を呼ぶ場合（async）は runProps が await で評価する。それ以外の差分はなし。
4. mock バックエンドは従来通り example のみ実行（prop は REPL/interp と js で実行）。
