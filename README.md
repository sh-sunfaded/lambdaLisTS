# lambdaLisTS

λ LisTS（SPEC.md v0.2, P0）のプロトタイプ実装です（Rust/cargo 製）。レキサ・パーサ・脱糖・双方向型検査・網羅性検査・エフェクト/モジュール検査・インタプリタ・JS バックエンド・CLI/REPL/フォーマッタを含みます。

## 動作環境

- Rust toolchain（edition 2021、MSRV 1.75+）。**ビルドに Node.js / npm は不要**です
- `js` バックエンドが生成した JS の実行（およびそのテスト）には Node.js ≥ 20 が必要です（ESM + source map v3）
- 成果物は単一バイナリ `lists` で、外部ランタイムは不要です。生成JS用の `runtime.js` はバイナリに埋め込まれており、出力時に展開されます

## セットアップ

```sh
cargo build            # デバッグビルド → target/debug/lists
cargo build --release  # リリースビルド → target/release/lists
```

依存クレート（`serde` / `serde_json` / `thiserror` / `clap` 4 / `sourcemap` 9）は `Cargo.lock` に固定され、初回ビルド時に自動で取得されます。

## テスト

```sh
cargo test   # 全183件（単体 146 + tests/ の統合 37）。すべて成功するのが正常です
```

テストの構成（GUIDE.md §7）:

| 層 | 場所 | 内容 |
|---|---|---|
| 単体テスト | 各モジュール内の `#[cfg(test)]` | レキサ・unify・末尾再帰など規則の細部（142件） |
| ゴールデン | `tests/golden.rs` | fixtures から診断JSON / Core IR / 型 / JS出力 / fmt結果を生成し `tests/golden/` と比較 |
| バックエンド | `tests/backend.rs` | 生成JSを `node --enable-source-maps` で実行し example の結果を検証（T8 のスタック対応も） |
| CLI/REPL | `tests/tools.rs` | ビルド済みバイナリを直接起動して compile / fmt / repl を検証 |
| 受け入れ | `tests/acceptance/` | T1–T11（SPEC §13）。差分テストはインタプリタとバックエンド実行の両方で評価して一致を検査 |

ゴールデンの再生成: `UPDATE_SNAPSHOTS=1 cargo test`

差分テストのバックエンドは環境変数 `LISTS_BACKEND=<name>`（デフォルト `js`）で切り替えられます。例: `LISTS_BACKEND=mock cargo test --test acceptance diff::`

## 使い方

CLI は `cargo run -- <command> ...`（ビルド後は `target/debug/lists <command> ...`）です。サブコマンドは `compile` / `fmt` / `repl`（§10.5）。

### compile — 検査 + バックエンド出力（§10.5）

```sh
cargo run -- compile tests/fixtures/money.lts tests/fixtures/cart.lts --out /tmp/out
# 別バックエンド（登録されているもの）を使う場合:
cargo run -- compile --backend js tests/fixtures/money.lts --out /tmp/out
```

パイプライン（全バックエンド共通のフロントエンド、`src/backends.rs` > `frontend`）: lexer（§2）→ parser（§3、ベストエフォート回復）→ desugar（§4–§5）→ 双方向型検査（§6）+ 網羅性検査（§6.5）→ エフェクト/モジュール検査（§7, §3.8）→ バックエンドの `compile()`（js なら §12 の emit）。

- 診断は `Diagnostic[]` の JSON を **stdout に1行**で出力します（エラーなしは `[]`）。exit code は 0（なし）/ 1（あり）です
- `--backend` のデフォルトは `js`。未知の名前を指定すると利用可能な一覧を含むエラーになります。バックエンドの追加方法は [docs/backends.md](docs/backends.md) を参照
- ファイルごとに `<file>.js` と `<file>.js.map`（source map v3）を出力します。`--out <dir>` 指定時はそのディレクトリ、未指定時はソースと同じ場所です。さらに出力ルートに `runtime.js` を一度だけ出力します（既存ファイルは上書きしません）
- 診断があっても codegen に成功したファイルの出力はそのまま書き出されます（全体の exit code は 1 になります）
- 生成した JS はそのまま Node で実行できます（example を評価して結果を表示）:

```sh
node --enable-source-maps /tmp/out/cart.js
# example 0: ok {"tag":"Err","value":{"tag":"EmptyCart"}}
```

#### 診断の JSON 形式（§9）

実例（`tests/golden/failure.type-mismatch.json`）:

```json
[
  {
    "kind": "type-mismatch",
    "severity": "error",
    "span": { "file": "test.lts", "start": 20, "end": 28, "line": 1, "col": 21 },
    "message": "型が一致しません: 期待 number、実際 (Result string ?2)",
    "data": { "expected": "number", "actual": "(Result string ?2)", "path": "(defn f).body" },
    "fix": { "op": "replace", "path": "(defn f).body", "template": "number" }
  }
]
```

- `kind`（kebab-case）: `parse-error`, `type-mismatch`, `unbound-symbol`, `non-exhaustive`, `redundant-arm`, `effect-violation`, `unknown-effect`, `arity-mismatch`, `name-convention`, `duplicate-definition`, `type-syntax-error`, `import-not-found`, `not-exported`, `module-cycle`, `internal-error`, `unsupported-in-p0`
- `span`: `start`/`end` はバイト位置、`line`/`col` は 1 始まり（col は Unicode スカラ値で換算）
- `data`: kind ごとの詳細（`expected`/`actual` は §6.2 の正規表現、網羅性検査なら `missing` パターン、AST パスは `path`）
- `fix`: 自動修正パッチ。`op` は `replace` / `insert` / `delete`、`path` は §10.2 の AST パス形式（例: `(defn f).body.arms`）

#### 生成される JS（§12）

- ESM 形式。出力はフラットなのでランタイムは常に `import { ok, err, structuralEqual } from "./runtime.js"` で読み込みます
- stdlib 関数は必要なものだけ `$stdlib_<name>` として生成されます（ユーザー定義による stdlib 名のシャドウに耐えるよう、生成側で import 名を `$stdlib_<name>` に逃がします）
- source map v3 には元の関数名が names として入るため、`node --enable-source-maps` での実行時エラーのスタックトレースは `.lts` の行・列を指します（T8）

### fmt — 一貫フォーマット（in-place、§10.3）

```sh
cargo run -- fmt tests/fixtures/money.lts
```

- インデントは 2 スペース。`defn` / `defn!` / `match` / `do` / `cond` / `example` は各要素を改行し、それ以外のフォームは 1 行にします
- コメントは保持されます（ノードの直前に配置。行末コメントは同一行を保持できる場合のみ保持）
- 冪等です（`fmt(fmt(x)) === fmt(x)`）。パースエラーのあるファイルは変更せず、診断を JSON で stdout に出力して exit 1 になります

### repl — JSON Lines REPL（§10.1）

```sh
cargo run -- repl
```

stdin から 1 行 1 JSON のリクエストを受け、stdout に 1 行 1 JSON で応答します（`quit` のみ応答なしで終了）。

リクエスト例:

```json
{"id": 1, "op": "eval", "src": "(defn add (a : number) (b : number) : number (+ a b))"}
{"id": 2, "op": "test"}
{"id": 3, "op": "prop", "name": "add-money commutative"}
{"id": 4, "op": "reload", "file": "./tests/fixtures/cart.lts"}
{"id": 5, "op": "type-of", "src": "add"}
{"id": 6, "op": "quit"}
```

応答例:

```json
{"id": 1, "ok": true, "defined": "add", "type": "(fn (number number) : number)"}
{"id": 2, "ok": true, "results": [{"example": "example 0", "pass": true}]}
{"id": 3, "ok": false, "errors": [{"kind": "unsupported-in-p0", "message": "prop評価はP1で実装"}]}
{"id": 5, "ok": true, "type": "(fn (number number) : number)"}
```

op の意味:

- `eval`: `src` をパース・検査して定義に追加し、定義名と型（§6.2 正規表現）を返します。診断がある場合は `ok: false` + `errors`
- `test`: ロード済みの全 `example` を評価し、期待値と構造的等価（§8.3）で比較します。失敗時は `failures` に `actual` / `expected` を返します
- `prop`: P0 では評価せず `unsupported-in-p0` エラーを返します（P1）
- `reload`: ファイルを再読み込みして再検査し、定義を追加します（相対パスはカレントディレクトリ基準）
- `type-of`: 式の型を §6.2 の正規表現で返します
- `quit`: 終了します
- 未知の op や不正な JSON 行も `ok: false` + `errors` で応答します

## λ LisTS >書き方（抜粋）

```lisp
(type Money (obj (amount : number) (currency : string)))

(export add-money zero)

(defn zero () : Money
  (obj (amount 0) (currency "JPY")))

(defn add-money (a : Money) (b : Money) : (Result Money string)
  (if (= a.currency b.currency)
    (ok (obj (amount (+ a.amount b.amount)) (currency a.currency)))
    (err (str-concat "currency mismatch: " a.currency " vs " b.currency))))

(example (add-money (obj (amount 1) (currency "JPY"))
                    (obj (amount 2) (currency "JPY")))
  => (ok (obj (amount 3) (currency "JPY"))))
```

構文 > full な定義は SPEC.md §2–§3、標準ライブラリは §11 を参照してください。サンプルは `tests/fixtures/`（付録A: money.lts / cart.lts、付録B: broken/effects/stock/cycle、末尾再帰: tail-self.lts / tail-mutual.lts）にあります。

標準ライブラリ（P0）の一覧（各関数の型は SPEC §11 を参照）:

| 分類 | 関数 |
|---|---|
| 数値・比較・論理 | `+ - * /`（2引数固定）、`< > <= >=`、`=`、`mod` `min` `max` `abs`、`not` |
| 文字列 | `str` `str-concat` `str-slice` `len` `contains?` `split` |
| 配列 | `array` `nth` `count` `slice` `push` `concat` `map` `filter` `fold-left` `find` `any?` `all?` |
| Result | `ok` `err` `bind` `map-result` `traverse` |

## P0 >制限

- `defn!`（エフェクト関数）は P1 で完全実装済みです。host を第1引数に取り、JS では `async function` + `await host.<effect>(args)` に生成され（§12.4/§12.6）、インタープリタでも host 経由で実行されます。純粋 `defn` からの effect 呼び出し・`defn!` 呼び出しは `effect-violation`（§7.2/§7.4）
- `prop` >評価（反例生成・縮小）は P1。構文と型検査のみ行います
- 末尾呼び出し（末尾位置の `App`）は **proper tail call** で、インタプリタ・JS の両方で定数スタックで実行されます（M12、§8.5 MUST。JS では 10^8 回の末尾再帰も成功）。非末尾再帰の深さ上限は実装依存のままです（実測値は docs/README.md に記載。インタプリタは `MAX_CALL_DEPTH = 10000`、`src/eval.rs` で超過時にパニックせず `internal-error` を返します）

##構成

```
src/
  main.rs         CLI エントリ（clap）。compile --out/--backend, fmt, repl
  lib.rs          pub mod 宣言
  backends.rs     バックエンド抽象（Backend trait・registry・フロントエンド共通処理）
  span.rs         Span
  diagnostic.rs   Diagnostic / Patch / AST パス（§9）
  token.rs        トークン種
  lexer.rs        §2 字句
  ast.rs          サーフェス AST（トリビア含む）
  parser.rs       §3 ベストエフォート回復
  ir.rs           Core IR（§5）
  desugar.rs      §4 意味的脱糖 → Core IR
  types.rs        Type / Scheme / 正規化・正規表現（§6.1–6.2）
  unify.rs        §6.4 単一化
  check.rs        §6.3 双方向型付け
  stdlib.rs       stdlib 登録（§11）
  exhaustive.rs   §6.5 網羅性検査
  effects.rs      §7 エフェクト検査
  resolver.rs     §3.8 モジュール解決
  value.rs        Value / structuralEqual（§8.2–8.3）
  eval.rs         §8 インタプリタ（末尾呼び出しはトランポリンで定数スタック）
  emit.rs         §12 JS lowering（末尾再帰はループ変換 + 相互再帰トランポリン）
  runtime.rs      backend/runtime.js を include_str! で埋め込む
  sourcemap_out.rs  source map v3 生成
  fmt.rs          §10.3 一貫フォーマッタ
  repl.rs         §10.1 JSON Lines REPL
  backend/runtime.js  生成JSが import するランタイム（唯一の非Rustソース）
tests/          cargo の統合テスト
  fixtures/     付録A/B + 末尾再帰の .lts
  golden/       スナップショット（診断JSON / JS出力 / fmt 結果）
  acceptance/   T1–T11 差分テスト（§13）
docs/           README.md（詳細・既知の相違）、backends.md（バックエンド実装ガイド）
audit/          backend-portability.md（バックエンド可搬性監査）
analysis/       v0.3-pre-audit.md（v0.3 事前監査）
```

コンパイルの流れ: `lexer → parser → desugar → check（types / unify / exhaustive / effects / resolver）→ バックエンド（js なら eval/emit相当）`。CLI は `main.rs`、REPL は `repl.rs`、フォーマッタは `fmt.rs` が入口です。

## 受け入れテスト（SPEC.md §13）

`cargo test` に T1〜T11 が含まれます（全183件）:

| テスト | 内容 | 状態 |
|---|---|---|
| T1 | 純粋関数 + Result（add-money > JS 実行値） | ✅ |
| T2 | 網羅性検査（stock.lts > non-exhaustive + fix） | ✅ |
| T3 | do >早期リターン（Err なら total を呼ばない） | ✅ |
| T4 | モジュール（import 実行 / cycle エラー） | ✅ |
| T5 | パーサ回復（broken.lts） | ✅ |
| T6 | エフェクト違反検査（effects.lts、検査のみ） | ✅ |
| T7 | インタプリタとバックエンドの差分一致 | ✅ |
| T8 | source map（スタックが .lts 行・元関数名を指す） | ✅ |
| T9 | REPL > JSON 往復（eval/test/type-of、prop は unsupported） | ✅ |
| T10 | 深い末尾再帰の差分一致（インタプリタ vs JS、定数スタック） | ✅ |
| T11 | mock バックエンド経由の差分（バックエンド抽象の証明） | ✅ |

差分テスト（T7/T10/T11）は付録の各 example を (1) Rust インタプリタ (2) 選択中バックエンド（`LISTS_BACKEND`、既定 `js` = 生成JSを `node` で実行）の両方で評価し、構造的等価（§8.3）で一致を検査します。ゴールデン（`tests/golden/`）は `UPDATE_SNAPSHOTS=1 cargo test` で再生成できます。詳細な説明・既知の相違は [docs/README.md](docs/README.md) を参照してください。

## 開発者向け

- コーディング規約は [GUIDE.md](GUIDE.md) §5–§6 を参照してください。要点: すべての AST/Core ノードが `span` を持つこと、出力は決定的であること（`BTreeMap` またはソート済みキーで。`HashMap` をそのまま出力に使わない）、`unsafe` は全面禁止（`#![forbid(unsafe_code)]`）、ライブラリコードでは `unwrap` / `expect` / パニック禁止（回復不能な内部状態は `internal-error` 診断に変換）、診断メッセージは日本語
- 依存の追加は最小限に（GUIDE.md §3）。現在の依存: `serde` / `serde_json`（診断JSON・REPL）、`thiserror`、`clap` 4（CLI）、`sourcemap` 9（source map v3 生成）
- 新しいバックエンド（例: WASM）の追加は `Backend` trait（`src/backends.rs`）の3メソッドを実装して `registry()` に登録するだけで、差分テストに自動で参加します。手順は [docs/backends.md](docs/backends.md) を参照
- ドキュメント一覧: [SPEC.md](SPEC.md)（言語仕様 v0.2 の全文）、[GUIDE.md](GUIDE.md)（実装開発ガイド）、[docs/README.md](docs/README.md)（詳細・実測値・既知の相違）、[docs/backends.md](docs/backends.md)（バックエンド実装ガイド）、[audit/backend-portability.md](audit/backend-portability.md)（可搬性監査レポート）、[analysis/v0.3-pre-audit.md](analysis/v0.3-pre-audit.md)（v0.3 事前監査）
- 差分仕様（設計ドラフト・SPEC 未反映）: [design/interp-capability.md](design/interp-capability.md)（interp-capability SPEC v0.1）

## 実装

このプロジェクトは **Rust（cargo）のみ** で実装されています。旧 TypeScript/npm ベースの実装（`package.json`、`tsconfig.json`、`src/**/*.ts`、vitest の `test/`）は撤去済みです。

- ビルド: `cargo build`、テスト: `cargo test`、CLI: `cargo run -- <command> ...`
- 生成した JS の実行には Node.js ≥ 20 が必要です（コンパイラ自体は Node に依存しません）
- `src/backend/runtime.js` は生成JSが `import` するランタイムで、`src/runtime.rs` から `include_str!` で埋め込まれるため削除しないでください
- Rust インタプリタの非末尾再帰の深さ上限は `src/eval.rs` > `MAX_CALL_DEPTH = 10000` です（SPEC §8.5）。上限超過時はパニックせず `internal-error` を返します。末尾呼び出しは proper tail call として定数スタックで実行されます
