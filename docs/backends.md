# バックエンド実装ガイド（M13〜）

差分テストは「インタープリタ vs 任意バックエンド」で走る。新しいバックエンド（例: WASM）を追加しても、
テスト側の変更はゼロで差分対象に乗る。規約は `src/backends.rs` の `Backend` トレイト。

## Backend トレイト（実装すべき3メソッド）

```rust
pub trait Backend {
    fn name(&self) -> &str;                                                    // --backend / LISTS_BACKEND の名前
    fn compile(&self, sources: &[SourceFile], out_dir: &Path)                  // コンパイル成果物
        -> Result<Artifacts, Vec<Diagnostic>>;
    fn run(&self, artifacts: &Artifacts) -> std::io::Result<RunOutput>;       // 成果物を実行し標準出力を得る
}
```

| メソッド | 契約 |
|---|---|
| `name()` | CLI `--backend <name>` とテスト環境変数 `LISTS_BACKEND` で指定される一意な名前（例: `"js"`, `"wasm"`） |
| `compile(sources, out_dir)` | sources（.lts）を out_dir にコンパイルする。フロントエンド（lex→parse→desugar→check）は `backends::frontend` 系の共通処理を使える。診断は `Vec<Diagnostic>` で返す（現行どおり JSON kind つき）。成功時は `Artifacts { dir, entry }` を返す。`entry` は実行エントリ（js なら最後のソースの .js ファイル名、wasm なら .wasm ファイル名） |
| `run(artifacts)` | 成果物を子プロセスで実行し、`RunOutput { stdout, stderr, code }` を返す。**環境変数は子プロセスに継承し、stderr は取り込む**（現行の Node 実行と同じ挙動。タイムアウト方針も現行どおり） |

## 登録手順（これだけで差分テストに参加できる）

1. `src/backends.rs` に構造体を実装して `Backend` を impl する（例: `JsBackend`, `MockBackend` を参照）。
2. `registry()` に `Box::new(YourBackend)` を追加し、`names()` に名前を追加する。
3. 以上。`tests/acceptance/diff.rs` の T7/T10 系は `backend_from_env()`（環境変数 `LISTS_BACKEND`、デフォルト `js`）経由で自動的にそのバックエンドを差分対象にする。

```sh
cargo test --test acceptance diff::                      # デフォルト: js
LISTS_BACKEND=wasm cargo test --test acceptance diff::   # 新バックエンドで同じケースを検証
lists compile foo.lts --out out --backend wasm           # CLI からも選択できる
```

## 差分テスト参加の約束（テスト側が期待する出力形式）

- `run()` の stdout には、各 `example` の結果を1行ずつ `example <index>: ok <JSON>` の形式で出すこと
  （現行の生成JSが `console.log("example N: ok " + JSON.stringify(actual))` で出す形と同一）。
  JSON の値は `Ok`/`Err` が `{ "tag": "Ok"|"Err", "value": ... }`、`nil` は `null`（付録C準拠）。
  参考: `backends::value_to_json`。
- 失敗時は exit code != 0 または `example N: FAIL ...` を出し、stderr に理由を書くこと。
- 比較はインタープリタの `Value` と `structural_equal` で行われる（tests/acceptance/common.rs の `assert_diff`）。

## host エフェクトのバックエンド写像（M20 + design/interp-capability.md §4）

組み込み host エフェクト（`host.rs::BUILTIN_HOST_EFFECTS` = now / elapsed / trace）の写像:

| エフェクト | interp（モック host） | JS バックエンド |
|---|---|---|
| `now!` | 固定列で決定的（`MOCK_NOW_BASE`=1700000000000 + 呼び出し回数*1000） | `Date.now()`（非決定的） |
| `elapsed!` | `now - 初回 now`（決定的） | `Date.now() - $BOOT_NOW`（モジュール先頭で1度だけ評価。非決定的） |
| `trace!` | 呼び出し記録（`host-log` op）へ蓄積 | `console.error(s)`（stderr。example の stdout には影響しない） |

- これらは effect 宣言不要・defn! の host ハンドラ不要（組み込みとして check をスキップ）。純粋 defn から呼ぶと effect-violation、未宣言エフェクトは unknown-effect（§7.4）。
- `now!`/`elapsed!` は JS 側が非決定的なので、差分テスト（T7 系）の対象外。`trace!` は stdout に影響しないため差分テスト可能。
- `read-file!` は bytes 型が未実装（M29）のため現時点では未対応。M29 で bytes を実装する際に追加され、**その時点で初めて JS バックエンドが Node 固有 API（`fs`）に依存する**。現状の now!/elapsed!/trace! は `Date.now()`/`console.error` のみで、Node 固有 API への依存はない。

## prop （P1/M22）のバックエンド実行とシード

- prop は JS バックエンドでも実行される。生成される `.js` に prop 本体の関数・型ディスクリプタ・本体 Core の JSON・`$SEED`・`$PROPS` を埋め込み、末尾で `await runProps($PROPS, $SEED, 100)` を呼ぶ。結果は stdout に `prop <i>: ok <json>` / `prop <i>: FAIL <json>` で出力され、interp（REPL の prop op）の応答 JSON と**同一バイト列**（反例・縮小・seed を含む）。
- 乱数は `src/rng.rs` の splitmix64 を BigInt でビット一致に写像（`src/backend/prop_runtime.js`）。DEFAULT_SEED=0x5EED1A7C2B219D4F は共通。同じ seed なら同じ列（決定性 MUST）。試行回数 100（DEFAULT_MAX_TRIALS）、境界値・文字種は interp と同一。
- `runtime.js` の export は §12.5 の 3つ（ok/err/structuralEqual）のまと1不変。prop 用ランタイムは別モジュール `prop-runtime.js` とし、prop を含む出力のときだけ `backends` が併置する。
- シード: `lists compile foo.lts --seed 42` で出力に `const $SEED = 42n;` が埋め込まれる（既定は rng::DEFAULT_SEED）。REPL は既存の --seed / prop リクエストの seed を使う。どちらも同じ seed なら interp と JS で同じ反例になる。

## 現状のバックエンド

| 名前 | compile | run |
|---|---|---|
| `js` | `.js` + `.js.map` + `runtime.js` を out_dir へ（SPEC §12） | `node <entry>` 子プロセス（cwd = out_dir） |
| `mock` | フロントエンドのみ（ファイル出力なし） | インタープリタで example を実行し結果をそのまま返す（拡張性の立証用） |

WASM バックエンドは本マイルストンでは未実装（P4ロードマップ）。上記の手順で追加するだけで T7/T10 系の差分に乗る。
