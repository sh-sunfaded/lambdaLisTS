use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use serde_json::json;

use crate::desugar::CoreDef;
use crate::diagnostic::{DiagKind, Diagnostic, Severity};
use crate::effects::{find_defn_calls, find_effect_calls};
use crate::ir::Core;
use crate::sourcemap_out::MapBuilder;
use crate::span::Span;
use crate::stdlib::stdlib;
use crate::types::{AliasEnv, Type};
use crate::value::Value;

const JS_RESERVED: &[&str] = &[
    "await", "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete",
    "do", "else", "enum", "export", "extends", "false", "finally", "for", "function", "if",
    "implements", "import", "in", "instanceof", "interface", "let", "new", "null", "package",
    "private", "protected", "public", "return", "static", "super", "switch", "this", "throw",
    "true", "try", "typeof", "undefined", "var", "void", "while", "with", "yield",
];

const RUNTIME_EXPORTS: &[&str] = &["ok", "err", "structuralEqual"];

// SPEC §11.6/付録C #11: slice/str-slice の境界規則（slice_bounds）の生成コード側実装。
const SLICE_BOUNDS_JS: &str = "function $slice_bounds(len, start, end) {\n  const s = Number.isNaN(start) ? 0 : start < 0 ? Math.max(len + start, 0) : Math.min(start, len);\n  const e = Number.isNaN(end) ? len : end < 0 ? Math.max(len + end, 0) : Math.min(end, len);\n  return [Math.trunc(s), Math.trunc(Math.max(e, s))];\n}\n";

// SPEC §11.6/付録C #10: str の印字規約（print_value 互換）。f64 は指数表記を展開する。
const PRINT_JS: &str = "function $print(v) {\n  if (v === null) return \"nil\";\n  if (typeof v === \"string\") return v;\n  if (typeof v === \"number\") return $print_num(v);\n  if (typeof v === \"boolean\") return v ? \"true\" : \"false\";\n  if (Array.isArray(v)) return \"[\" + v.map($print).join(\" \") + \"]\";\n  if (v instanceof Uint8Array) return \"#u8[\" + [...v].join(\" \") + \"]\";\n  if (typeof v === \"object\") {\n    if (v.tag === \"Ok\" && Object.keys(v).length === 2 && \"value\" in v) return \"(ok \" + $print(v.value) + \")\";\n    if (v.tag === \"Err\" && Object.keys(v).length === 2 && \"value\" in v) return \"(err \" + $print(v.value) + \")\";\n    const ks = Object.keys(v).sort($cmp_str);\n    return \"{\" + ks.map((k) => k + \": \" + $print(v[k])).join(\", \") + \"}\";\n  }\n  return \"<fn>\";\n}\nfunction $print_num(n) {\n  if (Number.isNaN(n)) return \"NaN\";\n  if (n === Infinity) return \"Infinity\";\n  if (n === -Infinity) return \"-Infinity\";\n  if (Object.is(n, -0)) return \"-0\";\n  const s = String(n);\n  if (s.indexOf(\"e\") < 0 && s.indexOf(\"E\") < 0) return s;\n  const m = /^(-?)(\\d+)(?:\\.(\\d+))?[eE]([+-]?\\d+)$/.exec(s);\n  if (!m) return s;\n  const sign = m[1];\n  const digits = m[2] + (m[3] || \"\");\n  const point = m[2].length + parseInt(m[4], 10);\n  if (point <= 0) return sign + \"0.\" + \"0\".repeat(-point) + digits;\n  if (point >= digits.length) return sign + digits + \"0\".repeat(point - digits.length);\n  return sign + digits.slice(0, point) + \".\" + digits.slice(point);\n}\nfunction $cmp_str(a, b) {\n  const pa = [...a];\n  const pb = [...b];\n  const n = Math.min(pa.length, pb.length);\n  for (let i = 0; i < n; i++) {\n    const d = pa[i].codePointAt(0) - pb[i].codePointAt(0);\n    if (d !== 0) return d;\n  }\n  return pa.length - pb.length;\n}\n";

// decimal（M23）用のローカルヘルパ。BigInt のスケール済み整数で interp(i128) とビット一致する。
// エラーメッセージは interp 側（src/decimal.rs の ERR_*）と同一文字列。
// design/interp-capability.md §1.1/§7: map は内部 Map、set は内部 Set（map との等価性の区別のため。
// design §7 の「set も Map」からの逸脱は REPORT.md に記録）。挿入順反復に一致させる。
// map/set 系 stdlib を使うときだけ出力し、既存の出力バイトには影響しない。
const MAP_JS: &str = "function $mjson(v) {\n  if (typeof v === \"bigint\") { const neg = v < 0n; const a = neg ? -v : v; const i = a / 1000000000000000000n; const f = a % 1000000000000000000n; return JSON.stringify({ tag: \"Dec\", value: (neg ? \"-\" : \"\") + i.toString() + \".\" + f.toString().padStart(18, \"0\") }); }\n  if (v instanceof Map) return \"{\\\"tag\\\":\\\"Map\\\",\\\"entries\\\":[\" + [...v].map(([k, x]) => \"[\" + $mjson(k) + \",\" + $mjson(x) + \"]\").join(\",\") + \"}]}\";\n  if (v instanceof Uint8Array) return \"[\" + [...v].join(\",\") + \"]\";\n  if (v instanceof Set) return \"{\\\"tag\\\":\\\"Set\\\",\\\"items\\\":[\" + [...v].map($mjson).join(\",\") + \"}]}\";\n  if (Array.isArray(v)) return \"[\" + v.map($mjson).join(\",\") + \"]\";\n  if (v !== null && typeof v === \"object\") { const ks = Object.keys(v).sort(); return \"{\" + ks.map((k) => JSON.stringify(k) + \":\" + $mjson(v[k])).join(\",\") + \"}\"; }\n  return JSON.stringify(v);\n}\nfunction $print_m(v) {\n  if (v instanceof Map) return v.size === 0 ? \"(map)\" : \"(map \" + [...v].map(([k, x]) => \"(\" + $print_m(k) + \" \" + $print_m(x) + \")\").join(\" \") + \")\";\n  if (v instanceof Set) return v.size === 0 ? \"(set)\" : \"(set \" + [...v].map($print_m).join(\" \") + \")\";\n  if (v === null) return \"nil\";\n  if (typeof v === \"string\") return v;\n  if (typeof v === \"number\") return $print_num(v);\n  if (typeof v === \"boolean\") return v ? \"true\" : \"false\";\n  if (typeof v === \"bigint\") return $print_dec(v);\n  if (Array.isArray(v)) return \"[\" + v.map($print_m).join(\" \") + \"]\";\n  if (typeof v === \"object\") {\n    if (v.tag === \"Ok\" && Object.keys(v).length === 2 && \"value\" in v) return \"(ok \" + $print_m(v.value) + \")\";\n    if (v.tag === \"Err\" && Object.keys(v).length === 2 && \"value\" in v) return \"(err \" + $print_m(v.value) + \")\";\n    const ks = Object.keys(v).sort($cmp_str);\n    return \"{\" + ks.map((k) => k + \": \" + $print_m(v[k])).join(\", \") + \"}\";\n  }\n  return \"<fn>\";\n}\nfunction $obj_to_map(o) {\n  const ks = Object.keys(o);\n  ks.sort((a, b) => { const pa = [...a]; const pb = [...b]; const n = Math.min(pa.length, pb.length); for (let i = 0; i < n; i++) { const d = pa[i].codePointAt(0) - pb[i].codePointAt(0); if (d !== 0) return d; } return pa.length - pb.length; });\n  const m = new Map();\n  for (const k of ks) m.set(k, o[k]);\n  return m;\n}\n";

// design/interp-capability.md §1.5/§7: ref の JS 写像は { value } オブジェクト。
// 参照等価のため非列挙のマーカを付ける（Object.keys や $mjson には出ない）。
// runtime.js の structuralEqual がこのマーカで a === b による参照等価にする。
const REF_JS: &str = "function $ref_new(v) { const r = { value: v }; Object.defineProperty(r, \"__lists_ref__\", { value: true }); return r; }\n";

fn core_uses_builtin_ref(e: &Core) -> bool {
    let mut sites = Vec::new();
    find_effect_calls(e, &mut sites);
    sites.iter().any(|s| crate::effects::is_builtin_ref_effect(&s.effect))
}

fn core_uses_clock(e: &Core) -> bool {
    let mut sites = Vec::new();
    find_effect_calls(e, &mut sites);
    sites
        .iter()
        .any(|s| matches!(s.effect.as_ref(), "now" | "elapsed"))
}

fn defs_use_clock(defs: &[CoreDef]) -> bool {
    defs.iter().any(|d| match d {
        CoreDef::Defn { body, .. } => core_uses_clock(body),
        CoreDef::Example {
            actual, expected, ..
        } => core_uses_clock(actual) || core_uses_clock(expected),
        CoreDef::Prop { body, .. } => core_uses_clock(body),
        _ => false,
    })
}

/// §4: elapsed! の基準時刻（モジュール先頭で1度だけ評価）
const NOW_JS: &str = "const $BOOT_NOW = Date.now();\n";

/// §4.1: read-file! の JS 写像（Node fs。戻り値は Promise<Result<Uint8Array, io-error>>>）
const READFILE_JS: &str = "async function $read_file(p) { try { const b = await $fs.readFile(p); return { tag: \"Ok\", value: new Uint8Array(b) }; } catch (e) { return { tag: \"Err\", value: { tag: \"io-error\" } }; } }\n";

fn core_uses_read_file(e: &Core) -> bool {
    let mut sites = Vec::new();
    find_effect_calls(e, &mut sites);
    sites.iter().any(|s| s.effect.as_ref() == "read-file")
}

fn defs_use_read_file(defs: &[CoreDef]) -> bool {
    defs.iter().any(|d| match d {
        CoreDef::Defn { body, .. } => core_uses_read_file(body),
        CoreDef::Example {
            actual, expected, ..
        } => core_uses_read_file(actual) || core_uses_read_file(expected),
        CoreDef::Prop { body, .. } => core_uses_read_file(body),
        _ => false,
    })
}

fn defs_use_builtin_ref(defs: &[CoreDef]) -> bool {
    defs.iter().any(|d| match d {
        CoreDef::Defn { body, .. } => core_uses_builtin_ref(body),
        CoreDef::Example {
            actual, expected, ..
        } => core_uses_builtin_ref(actual) || core_uses_builtin_ref(expected),
        CoreDef::Prop { body, .. } => core_uses_builtin_ref(body),
        _ => false,
    })
}

const DEC_JS: &str = "const $DEC_S = 10n ** 18n;\nconst $DEC_MAX = 170141183460469231731687303715884105727n;\nfunction $dec_check(r) { if (r > $DEC_MAX || r < -$DEC_MAX) throw new Error(\"decimal-overflow\"); return r; }\nconst $dec_add = (a, b) => $dec_check(a + b);\nconst $dec_sub = (a, b) => $dec_check(a - b);\nconst $dec_mul = (a, b) => { const p = a * b; const neg = p < 0n; const ap = neg ? -p : p; let q = ap / $DEC_S; const r = ap % $DEC_S; if (2n * r >= $DEC_S) q += 1n; return $dec_check(neg ? -q : q); };\nconst $dec_div = (a, b) => { if (b === 0n) throw new Error(\"decimal-division-by-zero\"); const neg = (a < 0n) !== (b < 0n); const num = (a < 0n ? -a : a) * $DEC_S; const den = b < 0n ? -b : b; let q = num / den; const r = num % den; if (2n * r >= den) q += 1n; return $dec_check(neg ? -q : q); };\nfunction $print_dec(v) { const neg = v < 0n; const a = neg ? -v : v; const i = a / $DEC_S; const f = a % $DEC_S; return (neg ? \"-\" : \"\") + i.toString() + \".\" + f.toString().padStart(18, \"0\"); }\nconst $to_dec = (a) => { if (typeof a !== \"number\" || Number.isNaN(a) || !Number.isFinite(a)) throw new Error(\"decimal-overflow\"); const m = /^([+-]?)(\\d+)(?:\\.(\\d+))?e([+-]?\\d+)$/.exec(a.toExponential()); if (!m) throw new Error(\"decimal-overflow\"); const sign = m[1] === \"-\" ? -1n : 1n; const digits = m[2] + (m[3] || \"\"); const exp = parseInt(m[4], 10) - (m[3] || \"\").length + 18; const mant = BigInt(digits); let r; if (exp >= 0) { r = mant * 10n ** BigInt(exp); } else { const den = 10n ** BigInt(-exp); let q = mant / den; const rem = mant % den; if (2n * rem >= den) q += 1n; r = q; } return $dec_check(sign * r); };\nfunction $json(v) { if (typeof v === \"bigint\") return JSON.stringify({ tag: \"Dec\", value: $print_dec(v) }); if (Array.isArray(v)) return \"[\" + v.map($json).join(\",\") + \"]\"; if (v !== null && typeof v === \"object\") { const ks = Object.keys(v).sort(); return \"{\" + ks.map((k) => JSON.stringify(k) + \":\" + $json(v[k])).join(\",\") + \"}\"; } return JSON.stringify(v); }\n";

pub fn js_name(name: &str) -> String {
    // 名前中の演算子由来の文字を無害化（§12.3 の kebab-case 変換の拡張。
    // design §3.1 の char<=? / char=?、§1.1 の obj->map 等）
    let mut n = name
        .replace("->", "_to_")
        .replace("<=", "_le_")
        .replace(">=", "_ge_")
        .replace('<', "_lt_")
        .replace('>', "_gt_")
        .replace('=', "_eq_")
        .replace('-', "_");
    if n.ends_with('?') {
        n = format!("{}_q", &n[..n.len() - 1]);
    }
    if JS_RESERVED.contains(&n.as_str()) {
        n = format!("${n}");
    }
    n
}

fn is_map_builtin(name: &str) -> bool {
    matches!(
        name,
        "map-empty"
            | "map-get"
            | "map-set"
            | "map-del"
            | "map-keys"
            | "map-has?"
            | "map-size"
            | "set-empty"
            | "set-add"
            | "set-has?"
            | "set->array"
            | "obj->map"
    )
}

fn stdlib_alias(name: &str) -> Option<&'static str> {
    Some(match name {
        "+" => "add",
        "-" => "sub",
        "*" => "mul",
        "/" => "div",
        "<" => "lt",
        ">" => "gt",
        "<=" => "le",
        ">=" => "ge",
        "=" => "eq",
        _ => return None,
    })
}

pub fn stdlib_js_name(name: &str) -> String {
    match stdlib_alias(name) {
        Some(a) => format!("$stdlib_{a}"),
        None => format!("$stdlib_{}", js_name(name)),
    }
}

pub fn lit_js(value: &Value) -> String {
    match value {
        Value::Nil => "null".to_string(),
        Value::Str(s) => serde_json::to_string(s.as_ref()).unwrap_or_else(|_| format!("\"{s}\"")),
        Value::Num(n) => {
            if n.is_nan() {
                "NaN".to_string()
            } else if n.is_infinite() {
                if *n > 0.0 {
                    "Infinity".to_string()
                } else {
                    "-Infinity".to_string()
                }
            } else {
                format!("{n}")
            }
        }
        Value::Bool(b) => format!("{b}"),
        // char は1要素文字列（コードポイント単位。design §7）。JSON 文字列リテラル=有効な JS 文字列。
        Value::Char(c) => serde_json::to_string(&c.to_string()).unwrap_or_else(|_| "\"?\"".to_string()),
        Value::Dec(s) => format!("{s}n"),
        _ => "null".to_string(),
    }
}

pub struct EmitInput {
    pub defs: Vec<CoreDef>,
    pub file: String,
    pub source_file: String,
    pub type_names: BTreeSet<String>,
    /// import 先も含む「defn! の名前」集合（async 生成の要否判定用。ローカルの defn! は pure フラグから自動で足す）。
    pub impure_names: BTreeSet<String>,
    /// prop 評価のシード（codegen 出力に $SEED として埋め込む。既定: rng::DEFAULT_SEED）
    pub seed: u64,
    /// 型エイリアス環境（prop 型ディスクリプタ生成用）
    pub aliases: crate::types::AliasEnv,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmitOutput {
    pub js: String,
    pub map: String,
    /// prop が含まれる場合 true（バックエンドは prop-runtime.js を配置する）
    pub uses_prop: bool,
}

/// 型を prop-runtime.js の生成規則用ディスクリプタへ（alias は展開済み）
fn type_json(t: &Type, aliases: &AliasEnv) -> serde_json::Value {
    use serde_json::json;
    match crate::types::normalize_type(t, aliases) {
        Type::Number => json!({"t":"number"}),
        Type::Str => json!({"t":"string"}),
        Type::Bool => json!({"t":"bool"}),
        Type::Nil => json!({"t":"nil"}),
        Type::Char => json!({"t":"char"}),
        Type::Dec => json!({"t":"decimal"}),
        Type::Builder => json!({"t":"builder"}),
        Type::Bytes => json!({"t":"bytes"}),
        Type::Literal(l) => json!({"t":"lit","v":l}),
        Type::Array(el) => json!({"t":"array","el":type_json(el.as_ref(), aliases)}),
        Type::Union(ms) => json!({"t":"union","members":ms.iter().map(|m| type_json(m, aliases)).collect::<Vec<_>>()}),
        Type::Obj(fs) => json!({"t":"obj","fields":fs.iter().map(|(k, v)| (k.to_string(), type_json(v, aliases))).collect::<serde_json::Map<String, serde_json::Value>>()}),
        Type::Map(k, v) => json!({"t":"map","k":type_json(k.as_ref(), aliases),"v":type_json(v.as_ref(), aliases)}),
        Type::Set(k) => json!({"t":"set","k":type_json(k.as_ref(), aliases)}),
        Type::Ref(inner) => json!({"t":"ref","inner":type_json(inner.as_ref(), aliases)}),
        Type::ResultT(ok, err) => json!({"t":"result","ok":type_json(ok.as_ref(), aliases),"err":type_json(err.as_ref(), aliases)}),
        _ => json!({"t":"other"}),
    }
}

fn contains_error_node(e: &Core) -> bool {
    match e {
        Core::Error { .. } => true,
        Core::Lam { body, .. } => contains_error_node(body),
        Core::App { f, args, .. } => {
            contains_error_node(f) || args.iter().any(contains_error_node)
        }
        Core::Let { rhs, body, .. } => contains_error_node(rhs) || contains_error_node(body),
        Core::Letrec { lam, body, .. } => contains_error_node(lam) || contains_error_node(body),
        Core::If {
            cond,
            then,
            else_,
            ..
        } => {
            contains_error_node(cond) || contains_error_node(then) || contains_error_node(else_)
        }
        Core::Match {
            scrutinee, arms, ..
        } => contains_error_node(scrutinee) || arms.iter().any(|a| contains_error_node(&a.body)),
        Core::Obj { fields, .. } => fields.iter().any(|(_, c)| contains_error_node(c)),
        Core::Field { obj, .. } => contains_error_node(obj),
        Core::Do { binds, body, .. } => {
            binds.iter().any(|b| contains_error_node(&b.expr)) || contains_error_node(body)
        }
        _ => false,
    }
}

/// エフェクト名を host オブジェクトのプロパティアクセスにする（§12.3/§12.4）。
/// host のフィールドは obj リテラルと同じく js_name で出力するので、ここも js_name に揃える。
fn effect_prop(name: &str) -> String {
    format!(".{}", js_name(name))
}

fn core_has_effect_call(e: &Core) -> bool {
    let mut sites = Vec::new();
    find_effect_calls(e, &mut sites);
    !sites.is_empty()
}

fn core_calls_impure(e: &Core, impure: &BTreeSet<String>) -> bool {
    let mut calls = Vec::new();
    find_defn_calls(e, &mut calls);
    calls.iter().any(|c| impure.contains(c.callee.as_ref()))
}

/// P1: defn!（またはエフェクト呼び出しを含む式）があるモジュールは async 生成を行う。
fn defs_use_async(defs: &[CoreDef], impure: &BTreeSet<String>) -> bool {
    for d in defs {
        match d {
            CoreDef::Defn { pure, body, .. } => {
                if !pure || core_has_effect_call(body) || core_calls_impure(body, impure) {
                    return true;
                }
            }
            CoreDef::Example {
                actual, expected, ..
            } => {
                if core_has_effect_call(actual) || core_has_effect_call(expected) {
                    return true;
                }
            }
            CoreDef::Prop { body, .. } => {
                if core_has_effect_call(body) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn core_uses_decimal(e: &Core) -> bool {
    match e {
        Core::Lit { value: Value::Dec(_), .. } => true,
        Core::Var { name, .. } => matches!(name.as_ref(), "to-dec" | "to-num"),
        Core::App { f, args, .. } => {
            core_uses_decimal(f)
                || args.iter().any(core_uses_decimal)
        }
        Core::Lam { body, .. } => core_uses_decimal(body),
        Core::Let { rhs, body, .. } => core_uses_decimal(rhs) || core_uses_decimal(body),
        Core::Letrec { lam, body, .. } => core_uses_decimal(lam) || core_uses_decimal(body),
        Core::If { cond, then, else_, .. } => {
            core_uses_decimal(cond) || core_uses_decimal(then) || core_uses_decimal(else_)
        }
        Core::Match { scrutinee, arms, .. } => {
            core_uses_decimal(scrutinee) || arms.iter().any(|a| core_uses_decimal(&a.body))
        }
        Core::Obj { fields, .. } => fields.iter().any(|(_, c)| core_uses_decimal(c)),
        Core::Field { obj, .. } => core_uses_decimal(obj),
        Core::EffectCall { args, .. } => args.iter().any(core_uses_decimal),
        Core::Do { binds, body, .. } => {
            binds.iter().any(|b| core_uses_decimal(&b.expr)) || core_uses_decimal(body)
        }
        _ => false,
    }
}

fn type_is_dec(t: &Type) -> bool {
    matches!(t, Type::Dec)
}

fn defs_use_decimal(defs: &[CoreDef]) -> bool {
    defs.iter().any(|d| match d {
        CoreDef::Defn {
            body,
            params,
            ret,
            ..
        } => {
            core_uses_decimal(body)
                || params.iter().any(|p| type_is_dec(&p.ty))
                || type_is_dec(ret)
        }
        CoreDef::Example {
            actual, expected, ..
        } => core_uses_decimal(actual) || core_uses_decimal(expected),
        CoreDef::Prop { body, params, .. } => {
            core_uses_decimal(body) || params.iter().any(|p| type_is_dec(&p.ty))
        }
        _ => false,
    })
}

pub fn emit(input: &EmitInput) -> Result<EmitOutput, Vec<Diagnostic>> {
    let mut errors: Vec<Diagnostic> = Vec::new();
    for d in &input.defs {
        match d {
            CoreDef::Error {
                message,
                span,
                path,
            } => errors.push(Diagnostic {
                kind: DiagKind::ParseError,
                severity: Severity::Error,
                span: span.clone(),
                message: message.clone(),
                data: Some(json!({ "path": path })),
                fix: None,
            }),
            CoreDef::Defn {
                name,
                body,
                span,
                path,
                ..
            } => {
                if contains_error_node(body) {
                    errors.push(Diagnostic {
                        kind: DiagKind::ParseError,
                        severity: Severity::Error,
                        span: span.clone(),
                        message: format!("フォーム '{name}' にパースエラーの回復ノードが含まれます"),
                        data: Some(json!({ "path": path })),
                        fix: None,
                    });
                }
            }
            _ => {}
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut forms: Vec<&CoreDef> = input.defs.iter().collect();
    forms.sort_by_key(|d| d.span().start);

    let mut probe = Gen::new(input);
    {
        let mut sink = JsWriter::sink();
        for d in &forms {
            probe.form_js(d, &mut sink);
        }
    }
    let header = probe.header();

    let mut w = JsWriter::new(&input.file, &input.source_file);
    if !header.is_empty() {
        w.write(&header);
    }
    let mut gen = Gen::new(input);
    if gen.uses_trampoline() {
        w.line("const $TAIL = Symbol(\"lists.tail\");");
        w.line("const $thunk = (f, args) => ({ [$TAIL]: true, f, args });");
        if gen.uses_async {
            w.line("async function $run(v) { while (v !== null && typeof v === \"object\" && v[$TAIL] === true) { v = await v.f(...v.args); } return v; }");
        } else {
            w.line("function $run(v) { while (v !== null && typeof v === \"object\" && v[$TAIL] === true) { v = v.f(...v.args); } return v; }");
        }
        w.line("");
    }
    for d in &forms {
        gen.form_js(d, &mut w);
    }
    let uses_prop = gen.has_props;
    if uses_prop {
        // --seed を埋め込み（REPL の --seed と同じ既定値 rng::DEFAULT_SEED）
        w.line(&format!("const $SEED = {}n;", input.seed));
        let specs: Vec<String> = gen
            .prop_specs
            .iter()
            .map(|(name, params, body, js)| {
                let params_json: Vec<String> = params
                    .iter()
                    .map(|p| {
                        format!(
                            "{{ name: {}, ty: {} }}",
                            serde_json::to_string(p.name.as_ref()).unwrap_or_else(|_| "\"?\"".to_string()),
                            type_json(&p.ty, &gen.aliases)
                        )
                    })
                    .collect();
                let core_json = serde_json::to_string(body.as_ref()).unwrap_or_else(|_| "null".to_string());
                format!(
                    "{{ name: {}, params: [{}], fn: {}, core: {} }}",
                    serde_json::to_string(name).unwrap_or_else(|_| "\"?\"".to_string()),
                    params_json.join(", "),
                    js,
                    core_json
                )
            })
            .collect();
        w.line(&format!("const $PROPS = [{}];", specs.join(", ")));
        w.line("await runProps($PROPS, $SEED, 100);");
    }
    let (body, map_builder) = w.into_parts();
    let map = map_builder.to_json();
    let js = format!("{body}\n//# sourceMappingURL={}.map\n", input.file);
    Ok(EmitOutput { js, map, uses_prop })
}

struct JsWriter {
    buf: String,
    line: u32,
    col: u32,
    map: Option<MapBuilder>,
}

impl JsWriter {
    fn new(out_file: &str, source_file: &str) -> Self {
        JsWriter {
            buf: String::new(),
            line: 0,
            col: 0,
            map: Some(MapBuilder::new(out_file, source_file)),
        }
    }

    fn sink() -> Self {
        JsWriter {
            buf: String::new(),
            line: 0,
            col: 0,
            map: None,
        }
    }

    fn write(&mut self, s: &str) {
        for ch in s.chars() {
            if ch == '\n' {
                self.line += 1;
                self.col = 0;
            } else {
                self.col += 1;
            }
        }
        self.buf.push_str(s);
    }

    fn line(&mut self, s: &str) {
        self.write(s);
        self.write("\n");
    }

    fn map_at(&mut self, span: &Span, name: Option<&str>) {
        if let Some(m) = &mut self.map {
            m.add(self.line, self.col, span, name);
        }
    }

    fn into_parts(self) -> (String, MapBuilder) {
        let map = self.map.unwrap_or_else(|| MapBuilder::new("", ""));
        (self.buf, map)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TailKind {
    Normal,
    SelfLoop,
    Trampoline,
}

#[derive(Clone)]
enum TailCtx {
    SelfLoop { fn_js: String, params: Vec<String> },
    Trampoline { members: BTreeSet<String> },
}

enum TailCallee {
    Defn(String),
    Indirect,
}

// Core IR 上の末尾位置（SPEC §8.5）にある App の呼び出し先を集める。
fn collect_tail_callees(
    e: &Core,
    in_tail: bool,
    defn_names: &BTreeSet<String>,
    import_names: &BTreeSet<String>,
    out: &mut Vec<TailCallee>,
) {
    match e {
        Core::App { f, args, .. } => {
            if in_tail {
                match f.as_ref() {
                    Core::Var { name, .. } => {
                        if defn_names.contains(name.as_ref()) {
                            out.push(TailCallee::Defn(name.to_string()));
                        } else if !RUNTIME_EXPORTS.contains(&name.as_ref())
                            && !stdlib().contains_key(name.as_ref())
                            && !import_names.contains(name.as_ref())
                        {
                            out.push(TailCallee::Indirect);
                        }
                    }
                    _ => out.push(TailCallee::Indirect),
                }
            }
            collect_tail_callees(f, false, defn_names, import_names, out);
            for a in args {
                collect_tail_callees(a, false, defn_names, import_names, out);
            }
        }
        // ラムダは独立した関数として Normal に出力するので、ここでは回収しない
        Core::Lam { .. } => {}
        Core::Let { rhs, body, .. } => {
            collect_tail_callees(rhs, false, defn_names, import_names, out);
            collect_tail_callees(body, in_tail, defn_names, import_names, out);
        }
        // Letrec.body は末尾位置（§8.5 の Let.body と同じ扱い。SPEC 側の未記載は REPORT.md で報告）
        Core::Letrec { lam, body, .. } => {
            collect_tail_callees(lam, false, defn_names, import_names, out);
            collect_tail_callees(body, in_tail, defn_names, import_names, out);
        }
        Core::If {
            cond,
            then,
            else_,
            ..
        } => {
            collect_tail_callees(cond, false, defn_names, import_names, out);
            collect_tail_callees(then, in_tail, defn_names, import_names, out);
            collect_tail_callees(else_, in_tail, defn_names, import_names, out);
        }
        Core::Match { scrutinee, arms, .. } => {
            collect_tail_callees(scrutinee, false, defn_names, import_names, out);
            for arm in arms {
                collect_tail_callees(&arm.body, in_tail, defn_names, import_names, out);
            }
        }
        Core::Obj { fields, .. } => {
            for (_, c) in fields {
                collect_tail_callees(c, false, defn_names, import_names, out);
            }
        }
        Core::Field { obj, .. } => collect_tail_callees(obj, false, defn_names, import_names, out),
        Core::Do { binds, body, .. } => {
            for b in binds {
                collect_tail_callees(&b.expr, false, defn_names, import_names, out);
            }
            collect_tail_callees(body, in_tail, defn_names, import_names, out);
        }
        _ => {}
    }
}

fn reachable(start: &str, graph: &BTreeMap<String, BTreeSet<String>>) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![start.to_string()];
    while let Some(n) = stack.pop() {
        if !seen.insert(n.clone()) {
            continue;
        }
        if let Some(nexts) = graph.get(&n) {
            for m in nexts {
                stack.push(m.clone());
            }
        }
    }
    seen
}

struct TailAnalysis {
    kinds: BTreeMap<String, TailKind>,
    scc_members: BTreeMap<String, BTreeSet<String>>,
}

fn analyze_tail(defs: &[CoreDef]) -> TailAnalysis {
    let defn_names: BTreeSet<String> = defs
        .iter()
        .filter_map(|d| match d {
            CoreDef::Defn { name, .. } => Some(name.to_string()),
            _ => None,
        })
        .collect();
    let import_names: BTreeSet<String> = defs
        .iter()
        .filter_map(|d| match d {
            CoreDef::Import { names, .. } => Some(names.iter().map(|n| n.to_string()).collect::<Vec<_>>()),
            _ => None,
        })
        .flatten()
        .collect();
    let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut callees: BTreeMap<String, Vec<TailCallee>> = BTreeMap::new();
    for d in defs {
        if let CoreDef::Defn { name, body, .. } = d {
            let mut out = Vec::new();
            collect_tail_callees(body, true, &defn_names, &import_names, &mut out);
            let edges: BTreeSet<String> = out
                .iter()
                .filter_map(|c| match c {
                    TailCallee::Defn(n) => Some(n.clone()),
                    TailCallee::Indirect => None,
                })
                .collect();
            graph.insert(name.to_string(), edges);
            callees.insert(name.to_string(), out);
        }
    }
    let mut kinds = BTreeMap::new();
    let mut scc_members = BTreeMap::new();
    for name in defn_names.iter() {
        let out = &callees[name];
        let edges = &graph[name];
        let has_indirect = out.iter().any(|c| matches!(c, TailCallee::Indirect));
        let self_edge = edges.contains(name);
        let reach_from = reachable(name, &graph);
        let members: BTreeSet<String> = graph
            .keys()
            .filter(|m| reach_from.contains(*m) && reachable(m, &graph).contains(name))
            .cloned()
            .collect();
        let kind = if edges.is_empty() && !has_indirect {
            TailKind::Normal
        } else if members.len() > 1 {
            TailKind::Trampoline
        } else if self_edge && !has_indirect {
            TailKind::SelfLoop
        } else if has_indirect {
            TailKind::Trampoline
        } else {
            TailKind::Normal
        };
        kinds.insert(name.clone(), kind);
        scc_members.insert(name.clone(), members);
    }
    TailAnalysis { kinds, scc_members }
}

struct Gen<'a> {
    user_names: BTreeSet<String>,
    export_names: BTreeSet<String>,
    stdlib_used: BTreeSet<String>,
    fresh_counter: usize,
    example_index: usize,
    function_names: Vec<(String, String)>,
    type_names: &'a BTreeSet<String>,
    tail_kinds: BTreeMap<String, TailKind>,
    scc_members: BTreeMap<String, BTreeSet<String>>,
    cur_tail: Option<TailCtx>,
    impure_defs: BTreeSet<String>,
    host_stack: Vec<String>,
    in_async: bool,
    uses_async: bool,
    decimal_used: bool,
    builtin_ref_used: bool,
    clock_used: bool,
    read_file_used: bool,
    has_props: bool,
    prop_specs: Vec<(String, Vec<crate::ir::Param>, Rc<Core>, String)>,
    aliases: crate::types::AliasEnv,
}

impl<'a> Gen<'a> {
    fn new(input: &'a EmitInput) -> Self {
        let mut user_names = BTreeSet::new();
        let mut export_names = BTreeSet::new();
        for d in &input.defs {
            match d {
                CoreDef::Defn { name, .. } => {
                    user_names.insert(name.to_string());
                }
                CoreDef::Export { names, .. } => {
                    for n in names {
                        export_names.insert(n.to_string());
                    }
                }
                _ => {}
            }
        }
        let analysis = analyze_tail(&input.defs);
        let mut impure_defs: BTreeSet<String> = input.impure_names.clone();
        for d in &input.defs {
            if let CoreDef::Defn {
                name,
                pure: false,
                ..
            } = d
            {
                impure_defs.insert(name.to_string());
            }
        }
        let uses_async = defs_use_async(&input.defs, &impure_defs);
        let builtin_ref_used = defs_use_builtin_ref(&input.defs);
        let clock_used = defs_use_clock(&input.defs);
        let read_file_used = defs_use_read_file(&input.defs);
        let has_props = input.defs.iter().any(|d| matches!(d, CoreDef::Prop { .. }));
        Gen {
            user_names,
            export_names,
            stdlib_used: BTreeSet::new(),
            fresh_counter: 0,
            example_index: 0,
            function_names: Vec::new(),
            type_names: &input.type_names,
            tail_kinds: analysis.kinds,
            scc_members: analysis.scc_members,
            cur_tail: None,
            impure_defs,
            host_stack: Vec::new(),
            in_async: false,
            uses_async,
            builtin_ref_used,
            clock_used,
            read_file_used,
            has_props,
            prop_specs: Vec::new(),
            aliases: input.aliases.clone(),
            decimal_used: defs_use_decimal(&input.defs),
        }
    }

    fn uses_trampoline(&self) -> bool {
        self.tail_kinds
            .values()
            .any(|k| matches!(k, TailKind::Trampoline))
    }

    fn fresh(&mut self) -> usize {
        self.fresh_counter += 1;
        self.fresh_counter
    }

    fn runtime_ref(&self, name: &str) -> String {
        if self.user_names.contains(name) {
            js_name(name)
        } else {
            name.to_string()
        }
    }

    fn map_used(&self) -> bool {
        self.stdlib_used.iter().any(|n| is_map_builtin(n))
    }

    fn var_ref(&mut self, name: &str) -> String {
        if self.user_names.contains(name) {
            return js_name(name);
        }
        if RUNTIME_EXPORTS.contains(&name) {
            return self.runtime_ref(name);
        }
        if stdlib().contains_key(name) {
            self.stdlib_used.insert(name.to_string());
            return stdlib_js_name(name);
        }
        js_name(name)
    }

    fn header(&self) -> String {
        let mut out = String::new();
        let imports: Vec<&str> = RUNTIME_EXPORTS
            .iter()
            .filter(|n| !self.user_names.contains(**n))
            .copied()
            .collect();
        if !imports.is_empty() {
            out += &format!("import {{ {} }} from \"./runtime.js\";\n", imports.join(", "));
        }
        if self.stdlib_used.contains("slice")
            || self.stdlib_used.contains("str-slice")
            || self.stdlib_used.contains("bytes-slice")
        {
            out += SLICE_BOUNDS_JS;
        }
        if self.read_file_used {
            out += "import * as $fs from \"node:fs/promises\";\n";
            out += READFILE_JS;
        }
        if self.has_props {
            out += "import { runProps } from \"./prop-runtime.js\";\n";
        }
        if self.stdlib_used.contains("str") {
            out += PRINT_JS;
        }
        if self.map_used() {
            out += MAP_JS;
        }
        if self.builtin_ref_used {
            out += REF_JS;
        }
        if self.clock_used {
            out += NOW_JS;
        }
        for name in &self.stdlib_used {
            out += &format!("const {} = {};\n", stdlib_js_name(name), self.stdlib_impl(name));
        }
        if self.decimal_used {
            out += DEC_JS;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out
    }

    fn stdlib_impl(&self, name: &str) -> String {
        let se = self.runtime_ref("structuralEqual");
        match name {
            "+" => {
                if self.decimal_used {
                    "(a, b) => typeof a === \"bigint\" ? $dec_add(a, b) : a + b".to_string()
                } else {
                    "(a, b) => a + b".to_string()
                }
            }
            "-" => {
                if self.decimal_used {
                    "(a, b) => typeof a === \"bigint\" ? $dec_sub(a, b) : a - b".to_string()
                } else {
                    "(a, b) => a - b".to_string()
                }
            }
            "*" => {
                if self.decimal_used {
                    "(a, b) => typeof a === \"bigint\" ? $dec_mul(a, b) : a * b".to_string()
                } else {
                    "(a, b) => a * b".to_string()
                }
            }
            "/" => {
                if self.decimal_used {
                    "(a, b) => typeof a === \"bigint\" ? $dec_div(a, b) : a / b".to_string()
                } else {
                    "(a, b) => a / b".to_string()
                }
            }
            "<" => "(a, b) => a < b".to_string(),
            ">" => "(a, b) => a > b".to_string(),
            "<=" => "(a, b) => a <= b".to_string(),
            ">=" => "(a, b) => a >= b".to_string(),
            "=" => format!("(a, b) => {se}(a, b)"),
            "mod" => "(a, b) => a % b".to_string(),
            "abs" => "(a) => Math.abs(a)".to_string(),
            "min" => "(a, b) => { if (Number.isNaN(a)) return b; if (Number.isNaN(b)) return a; return a < b ? a : b; }".to_string(),
            "max" => "(a, b) => { if (Number.isNaN(a)) return b; if (Number.isNaN(b)) return a; return a > b ? a : b; }".to_string(),
            "not" => "(a) => !a".to_string(),
            "str" => {
                let f = if self.map_used() { "$print_m" } else { "$print" };
                if self.decimal_used {
                    format!("(a) => typeof a === \"bigint\" ? $print_dec(a) : {f}(a)")
                } else {
                    format!("(a) => {f}(a)")
                }
            }
            "to-dec" => "(a) => $to_dec(a)".to_string(),
            "to-num" => "(a) => Number(a) / 1e18".to_string(),
            "str-concat" => "(...a) => a.join(\"\")".to_string(),
            "str-slice" => "(s, i, j) => { const c = [...s]; const [a, b] = $slice_bounds(c.length, i, j); return c.slice(a, b).join(\"\"); }".to_string(),
            "len" => "(s) => [...s].length".to_string(),
            "contains?" => "(s, t) => s.includes(t)".to_string(),
            // §11.6: 空区切りは各 Unicode スカラ値文字を1要素とし空要素を生まない（[...s] はコードポイント単位）
            "split" => "(s, t) => t === \"\" ? [...s] : s.split(t)".to_string(),
            "array" => "(...a) => a".to_string(),
            // §11.6: (union T nil) は実行時に生値。成功は arr[trunc(i)]、失敗は null(=nil)。
            "nth" => format!(
                "(arr, i) => {{ const k = Math.trunc(i); return (i >= 0 && k < arr.length) ? arr[k] : null; }}"
            ),
            "count" => "(arr) => arr.length".to_string(),
            "slice" => "(arr, i, j) => { const [a, b] = $slice_bounds(arr.length, i, j); return arr.slice(a, b); }".to_string(),
            "push" => "(arr, x) => [...arr, x]".to_string(),
            "concat" => "(arr, b) => [...arr, ...b]".to_string(),
            "map" => "(arr, f) => arr.map((x) => f(x))".to_string(),
            "filter" => "(arr, f) => arr.filter((x) => f(x))".to_string(),
            "fold-left" => "(arr, init, f) => arr.reduce((acc, x) => f(acc, x), init)".to_string(),
            // §11.6: 成功は生値 x、失敗は null(=nil)。
            "find" => format!(
                "(arr, f) => {{ for (const x of arr) if (f(x)) return x; return null; }}"
            ),
            "any?" => "(arr, f) => arr.some((x) => f(x))".to_string(),
            "all?" => "(arr, f) => arr.every((x) => f(x))".to_string(),
            // design/interp-capability.md §1.1/§7: map は内部 Map（挿入順反復）、set は内部 Set。
            // map-set の既存キー上書きは JS Map と同じく元の順序を維持する。
            "map-empty" => "() => new Map()".to_string(),
            "map-get" => {
                let ok = self.runtime_ref("ok");
                let err = self.runtime_ref("err");
                format!("(m, k) => m.has(k) ? {ok}(m.get(k)) : {err}({{ tag: \"not-found\" }})")
            }
            "map-set" => "(m, k, v) => { const n = new Map(m); n.set(k, v); return n; }".to_string(),
            "map-del" => "(m, k) => { const n = new Map(m); n.delete(k); return n; }".to_string(),
            "map-keys" => "(m) => [...m.keys()]".to_string(),
            "map-has?" => "(m, k) => m.has(k)".to_string(),
            "map-size" => "(m) => m.size".to_string(),
            "set-empty" => "() => new Set()".to_string(),
            "set-add" => "(s, k) => { const n = new Set(s); n.add(k); return n; }".to_string(),
            "set-has?" => "(s, k) => s.has(k)".to_string(),
            "set->array" => "(s) => [...s]".to_string(),
            "obj->map" => "(o) => $obj_to_map(o)".to_string(),
            // design/interp-capability.md §1.2/§3.1: char は1要素文字列。位置はコードポイント（スカラー値）単位。
            // 添字の切り捨て・NaN は nth と同じ規則（§11.6）。
            "char-at" => {
                let ok = self.runtime_ref("ok");
                let err = self.runtime_ref("err");
                format!("(s, i) => {{ const c = [...s]; const k = Math.trunc(i); return (Number.isNaN(i) || i < 0 || k >= c.length) ? {err}({{ tag: \"out-of-range\" }}) : {ok}(c[k]); }}")
            }
            "char-code" => "(c) => c.codePointAt(0)".to_string(),
            "code->char" => {
                let ok = self.runtime_ref("ok");
                let err = self.runtime_ref("err");
                format!("(n) => {{ if (!Number.isInteger(n) || n < 0 || n > 0x10FFFF || (n >= 0xD800 && n <= 0xDFFF)) return {err}({{ tag: \"invalid-code\" }}); return {ok}(String.fromCodePoint(n)); }}")
            }
            "str-take-while" => "(s, i, f) => { const c = [...s]; const len = c.length; const st = Number.isNaN(i) ? 0 : i < 0 ? Math.max(len + i, 0) : Math.min(i, len); let p = Math.trunc(st); let acc = \"\"; while (p < len && f(c[p])) { acc += c[p]; p++; } return { acc, pos: p }; }".to_string(),
            "str-skip-while" => "(s, i, f) => { const c = [...s]; const len = c.length; const st = Number.isNaN(i) ? 0 : i < 0 ? Math.max(len + i, 0) : Math.min(i, len); let p = Math.trunc(st); while (p < len && f(c[p])) p++; return p; }".to_string(),
            "str-split-at" => {
                let ok = self.runtime_ref("ok");
                let err = self.runtime_ref("err");
                format!("(s, i) => {{ const c = [...s]; if (Number.isNaN(i)) return {err}({{ tag: \"out-of-range\" }}); const k = Math.trunc(i); if (i < 0 || k > c.length) return {err}({{ tag: \"out-of-range\" }}); return {ok}({{ left: c.slice(0, k).join(\"\"), right: c.slice(k).join(\"\") }}); }}")
            }
            "str-contains?" => "(s, t) => s.includes(t)".to_string(),
            // 文字列の <= は UTF-16 コード単位順なので、スカラー値（コードポイント）比較にする
            "char<=?" => "(a, b) => a.codePointAt(0) <= b.codePointAt(0)".to_string(),
            "char=?" => "(a, b) => a === b".to_string(),
            "char-alphabetic?" => "(c) => /\\p{Alphabetic}/u.test(c)".to_string(),
            "char-numeric?" => "(c) => /\\p{N}/u.test(c)".to_string(),
            "char-whitespace?" => "(c) => /\\p{White_Space}/u.test(c)".to_string(),
            // design §3.2: builder は JS では配列 + join。push は新しい配列を返す（不変値）。
            "builder-empty" => "() => []".to_string(),
            "builder-push" => "(b, s) => [...b, s]".to_string(),
            "builder->str" => "(b) => b.join(\"\")".to_string(),
            "builder-join" => "(arr, sep) => arr.join(sep)".to_string(),
            // design/interp-capability.md §1.4: bytes は JS では Uint8Array（不変。変更操作はstdlibに存在しない）
            // bytes-get は nth と同じく Option（範囲外は null=nil）、小数添字は切り捨て
            "bytes-get" => format!(
                "(b, i) => {{ const k = Math.trunc(i); return (Number.isNaN(i) || i < 0 || k >= b.length) ? null : b[k]; }}"
            ),
            "bytes-len" => "(b) => b.length".to_string(),
            "bytes-slice" => "(b, i, j) => { const [a, z] = $slice_bounds(b.length, i, j); return b.slice(a, z); }".to_string(),
            "str->bytes" => "(s) => new TextEncoder().encode(s)".to_string(),
            "bytes->str" => {
                let ok = self.runtime_ref("ok");
                let err = self.runtime_ref("err");
                format!("(b) => {{ try {{ return {ok}(new TextDecoder(\"utf-8\", {{ fatal: true }}).decode(b)); }} catch (e) {{ return {err}({{ tag: \"invalid-utf8\" }}); }} }}")
            }
            "str->codepoints" => "(s) => [...s].map((c) => c.codePointAt(0))".to_string(),
            "codepoints->str" => {
                let ok = self.runtime_ref("ok");
                let err = self.runtime_ref("err");
                format!("(arr) => {{ for (const n of arr) {{ if (!Number.isInteger(n) || n < 0 || n > 0x10FFFF || (n >= 0xD800 && n <= 0xDFFF)) return {err}({{ tag: \"invalid-code\" }}); }} return {ok}(arr.map((n) => String.fromCodePoint(n)).join(\"\")); }}")
            }
            other => format!(
                "(() => {{ throw new Error('unsupported stdlib: ' + {other:?}); }})"
            ),
        }
    }

    fn form_js(&mut self, d: &CoreDef, w: &mut JsWriter) {
        match d {
            CoreDef::Defn {
                name,
                params,
                body,
                span,
                pure,
                ..
            } => {
                let exported = if self.export_names.contains(name.as_ref()) {
                    "export "
                } else {
                    ""
                };
                let js = js_name(name);
                self.function_names.push((js.clone(), name.to_string()));
                let kind = self
                    .tail_kinds
                    .get(name.as_ref())
                    .copied()
                    .unwrap_or(TailKind::Normal);
                let params_js = params.iter().map(|p| js_name(&p.name)).collect::<Vec<_>>().join(", ");
                let param_names: Vec<String> = params.iter().map(|p| js_name(&p.name)).collect();
                // P1 (§12.4/§12.6): defn! は async 関数。host は第1引数（§12.6）。
                let async_kw = if *pure { "" } else { "async " };
                let saved_host_len = self.host_stack.len();
                if !*pure {
                    let h = params
                        .first()
                        .map(|p| js_name(&p.name))
                        .unwrap_or_else(|| "host".to_string());
                    self.host_stack.push(h);
                }
                let saved_async = self.in_async;
                self.in_async = !*pure;
                match kind {
                    TailKind::Normal => {
                        w.map_at(span, Some(name));
                        w.line(&format!("{exported}{async_kw}function {js}({params_js}) {{"));
                        self.stmt(body, "  ", w);
                        w.line("}");
                    }
                    TailKind::SelfLoop => {
                        w.map_at(span, Some(name));
                        w.line(&format!("{exported}{async_kw}function {js}({params_js}) {{"));
                        w.map_at(span, None);
                        w.line("  while (true) {");
                        let saved = self.cur_tail.replace(TailCtx::SelfLoop {
                            fn_js: js.clone(),
                            params: param_names,
                        });
                        self.stmt(body, "    ", w);
                        self.cur_tail = saved;
                        w.line("  }");
                        w.line("}");
                    }
                    TailKind::Trampoline => {
                        let inner = format!("{js}$inner");
                        let members = self
                            .scc_members
                            .get(name.as_ref())
                            .cloned()
                            .unwrap_or_else(|| BTreeSet::from([name.to_string()]));
                        w.map_at(span, Some(name));
                        w.line(&format!("{async_kw}function {inner}({params_js}) {{"));
                        let saved = self.cur_tail.replace(TailCtx::Trampoline { members });
                        self.stmt(body, "  ", w);
                        self.cur_tail = saved;
                        w.line("}");
                        w.map_at(span, Some(name));
                        w.line(&format!(
                            "{exported}{async_kw}function {js}({params_js}) {{ return $run({inner}({params_js})); }}"
                        ));
                    }
                }
                self.in_async = saved_async;
                self.host_stack.truncate(saved_host_len);
            }
            CoreDef::Import {
                path,
                names,
                span,
            } => {
                let value_names: Vec<String> = names
                    .iter()
                    .filter(|n| !self.type_names.contains(n.as_ref()))
                    .map(|n| js_name(n))
                    .collect();
                if value_names.is_empty() {
                    return;
                }
                let js_path = path.trim_end_matches(".lts").to_string() + ".js";
                w.map_at(span, None);
                w.line(&format!(
                    "import {{ {} }} from {};",
                    value_names.join(", "),
                    serde_json::to_string(&js_path).unwrap_or_else(|_| format!("\"{js_path}\""))
                ));
            }
            CoreDef::Prop {
                name,
                params,
                body,
                ..
            } => {
                // M22 prop の JS 評価: 本体を関数として出力し、$PROPS に登録する
                let js = format!("$prop_{}", self.prop_specs.len());
                self.prop_specs.push((
                    name.to_string(),
                    params.clone(),
                    Rc::new(body.clone()),
                    js.clone(),
                ));
                let params_js = params
                    .iter()
                    .map(|p| js_name(&p.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                let p_async = core_has_effect_call(body) || core_calls_impure(body, &self.impure_defs);
                let async_kw = if p_async { "async " } else { "" };
                w.line(&format!("{async_kw}function {js}({params_js}) {{"));
                let saved_tail = self.cur_tail.take();
                let saved_async = self.in_async;
                self.in_async = p_async;
                self.stmt(body, "  ", w);
                self.in_async = saved_async;
                self.cur_tail = saved_tail;
                w.line("}");
            }
            CoreDef::Example {
                actual,
                expected,
                span,
                ..
            } => {
                let i = self.example_index;
                self.example_index += 1;
                let se = self.runtime_ref("structuralEqual");
                // P1: モジュールが async を含む場合、example は defn! を呼びうるので await で包む。
                let saved_async = self.in_async;
                self.in_async = self.uses_async;
                w.map_at(span, None);
                w.line("{");
                w.write("  const actual = ");
                self.expr(actual, "  ", w);
                w.write(";\n");
                w.write("  const expected = ");
                self.expr(expected, "  ", w);
                w.write(";\n");
                self.in_async = saved_async;
                let ser = if self.map_used() {
                    "$mjson(actual)"
                } else if self.decimal_used {
                    "$json(actual)"
                } else {
                    "JSON.stringify(actual)"
                };
                w.line(&format!(
                    "  if (!{se}(actual, expected)) throw new Error(\"example {i} failed: \" + {ser});"
                ));
                w.line(&format!(
                    "  console.log(\"example {i}: ok \" + {ser});"
                ));
                w.line("}");
            }
            _ => {}
        }
    }

    fn stmt(&mut self, e: &Core, ind: &str, w: &mut JsWriter) {
        w.map_at(e.span(), None);
        if let Some(ctx) = self.cur_tail.clone() {
            if let Core::App { f, args, .. } = e {
                let is_self = match &ctx {
                    TailCtx::SelfLoop { fn_js, .. } => {
                        matches!(f.as_ref(), Core::Var { name, .. } if js_name(name) == *fn_js)
                    }
                    TailCtx::Trampoline { .. } => false,
                };
                if is_self || matches!(ctx, TailCtx::Trampoline { .. }) {
                    match &ctx {
                        TailCtx::SelfLoop { params, .. } => {
                            let temps: Vec<String> =
                                (0..args.len()).map(|_| format!("$a{}", self.fresh())).collect();
                            for (t, a) in temps.iter().zip(args.iter()) {
                                w.write(&format!("{ind}const {t} = "));
                                self.expr(a, ind, w);
                                w.write(";\n");
                            }
                            for (p, t) in params.iter().zip(temps.iter()) {
                                w.line(&format!("{ind}{p} = {t};"));
                            }
                            w.line(&format!("{ind}continue;"));
                        }
                        TailCtx::Trampoline { members } => {
                            let member = match f.as_ref() {
                                Core::Var { name, .. } if members.contains(name.as_ref()) => {
                                    Some(format!("{}$inner", js_name(name)))
                                }
                                _ => None,
                            };
                            w.write(&format!("{ind}return $thunk("));
                            match member {
                                Some(m) => w.write(&m),
                                None => self.expr(f, ind, w),
                            }
                            w.write(", [");
                            for (i, a) in args.iter().enumerate() {
                                if i > 0 {
                                    w.write(", ");
                                }
                                self.expr(a, ind, w);
                            }
                            w.write("]);\n");
                        }
                    }
                    return;
                }
            }
        }
        match e {
            Core::Do { .. } => self.do_stmt(e, ind, w),
            Core::If {
                cond,
                then,
                else_,
                ..
            } => {
                w.write(&format!("{ind}if ("));
                self.expr(cond, ind, w);
                w.write(") {\n");
                self.stmt(then, &format!("{ind}  "), w);
                w.write(&format!("\n{ind}}} else {{\n"));
                self.stmt(else_, &format!("{ind}  "), w);
                w.write(&format!("\n{ind}}}\n"));
            }
            Core::Let {
                name, rhs, body, ..
            } => {
                w.write(&format!("{ind}const {} = ", js_name(name)));
                self.expr(rhs, ind, w);
                w.write(";\n");
                self.stmt(body, ind, w);
            }
            Core::Match { .. } => self.match_tail(e, ind, w, None),
            _ => {
                w.write(&format!("{ind}return "));
                self.expr(e, ind, w);
                w.write(";\n");
            }
        }
    }

    fn do_stmt(&mut self, e: &Core, ind: &str, w: &mut JsWriter) {
        let Core::Do { binds, body, .. } = e else {
            return;
        };
        for b in binds {
            let r = format!("$r{}", self.fresh());
            let is_effect = matches!(b.expr, Core::EffectCall { .. });
            w.write(&format!("{ind}const {r} = "));
            self.expr(&b.expr, ind, w);
            w.write(";\n");
            if is_effect {
                // §7.4 rule 5: defn! 内のエフェクト呼び出しは Result とは限らない。interp と同じく
                // Err なら早期 return、束縛値は Ok なら中身、それ以外はそのまま。
                w.line(&format!(
                    "{ind}if ({r} !== null && {r}.tag === \"Err\") return {r};"
                ));
                if b.name.as_ref() != "_" {
                    w.line(&format!(
                        "{ind}const {} = ({r} !== null && {r}.tag === \"Ok\") ? {r}.value : {r};",
                        js_name(&b.name)
                    ));
                }
            } else {
                w.line(&format!("{ind}if ({r}.tag === \"Err\") return {r};"));
                if b.name.as_ref() != "_" {
                    w.line(&format!("{ind}const {} = {r}.value;", js_name(&b.name)));
                }
            }
        }
        self.stmt(body, ind, w);
    }

    fn match_tail(&mut self, e: &Core, ind: &str, w: &mut JsWriter, bound: Option<&str>) {
        let Core::Match {
            scrutinee, arms, ..
        } = e
        else {
            return;
        };
        let s = match bound {
            Some(b) => b.to_string(),
            None => {
                let s = format!("$s{}", self.fresh());
                w.write(&format!("{ind}const {s} = "));
                self.expr(scrutinee, ind, w);
                w.write(";\n");
                s
            }
        };
        let mut first = true;
        let mut has_catch_all = false;
        for arm in arms {
            let cond = self.arm_cond(&arm.pattern, &s);
            w.write(ind);
            if cond == "true" {
                has_catch_all = true;
                w.write("else {\n");
            } else {
                w.write(if first { "if" } else { "else if" });
                w.write(&format!(" ({cond}) {{\n"));
                first = false;
            }
            let mut binds_out: Vec<String> = Vec::new();
            self.arm_binds(&arm.pattern, &s, &format!("{ind}  "), &mut binds_out);
            for b in binds_out {
                w.line(&b);
            }
            self.stmt(&arm.body, &format!("{ind}  "), w);
            w.write(&format!("\n{ind}}}\n"));
        }
        if !has_catch_all {
            w.write(&format!("{ind}else {{\n{ind}  throw new Error(\"non-exhaustive match\");\n{ind}}}\n"));
        }
    }

    fn arm_cond(&mut self, p: &crate::ir::Pat, s: &str) -> String {
        use crate::ir::PatKind;
        match &p.kind {
            PatKind::Wildcard | PatKind::Var(_) => "true".to_string(),
            PatKind::Lit(v) => format!("{s} === {}", lit_js(v)),
            PatKind::Or(ps) => ps
                .iter()
                .map(|sub| format!("({})", self.arm_cond(sub, s)))
                .collect::<Vec<_>>()
                .join(" || "),
            PatKind::Obj(fields) => {
                let mut conds = vec![format!(
                    "typeof {s} === \"object\" && {s} !== null && !Array.isArray({s})"
                )];
                for (n, sub) in fields {
                    conds.push(format!("({})", self.arm_cond(sub, &format!("{s}.{}", js_name(n)))));
                }
                conds.join(" && ")
            }
            PatKind::Array(elements) => {
                let mut conds = vec![format!("Array.isArray({s})"), format!("{s}.length === {}", elements.len())];
                for (i, el) in elements.iter().enumerate() {
                    conds.push(format!("({})", self.arm_cond(el, &format!("{s}[{i}]"))));
                }
                conds.join(" && ")
            }
        }
    }

    fn arm_binds(&mut self, p: &crate::ir::Pat, s: &str, ind: &str, out: &mut Vec<String>) {
        use crate::ir::PatKind;
        match &p.kind {
            PatKind::Var(name) => out.push(format!("{ind}const {} = {s};", js_name(name))),
            PatKind::Obj(fields) => {
                for (n, sub) in fields {
                    self.arm_binds(sub, &format!("{s}.{}", js_name(n)), ind, out);
                }
            }
            PatKind::Array(elements) => {
                for (i, el) in elements.iter().enumerate() {
                    self.arm_binds(el, &format!("{s}[{i}]"), ind, out);
                }
            }
            _ => {}
        }
    }

    fn expr(&mut self, e: &Core, ind: &str, w: &mut JsWriter) {
        // async 文脈（defn! 本体やエフェクトを含む lam）では呼び出し結果が Promise になりうるので
        // await で包む（§12.4 の `await host.name(args)` と同じ扱い）。非同期でない値への await は恒等。
        if self.in_async && matches!(e, Core::App { .. } | Core::EffectCall { .. } | Core::Letrec { .. }) {
            w.write("(await ");
            self.expr_inner(e, ind, w);
            w.write(")");
            return;
        }
        self.expr_inner(e, ind, w);
    }

    fn expr_inner(&mut self, e: &Core, ind: &str, w: &mut JsWriter) {
        w.map_at(e.span(), None);
        match e {
            Core::Lit { value, .. } => w.write(&lit_js(value)),
            Core::Var { name, .. } => {
                let n = name.clone();
                let r = self.var_ref(&n);
                w.write(&r);
            }
            Core::Lam {
                params, body, ..
            } => {
                let params_js = params.iter().map(|p| js_name(&p.name)).collect::<Vec<_>>().join(", ");
                let lam_async = core_has_effect_call(body) || core_calls_impure(body, &self.impure_defs);
                let async_kw = if lam_async { "async " } else { "" };
                w.write(&format!("({async_kw}({params_js}) => {{\n"));
                let saved = self.cur_tail.take();
                let saved_async = self.in_async;
                self.in_async = lam_async;
                self.stmt(body, &format!("{ind}  "), w);
                self.in_async = saved_async;
                self.cur_tail = saved;
                w.write(&format!("{ind}}})"));
            }
            Core::App { f, args, .. } => {
                if let Core::Var { name, .. } = f.as_ref() {
                    let n = name.as_ref();
                    if !self.user_names.contains(n) {
                        if n == "bind" && args.len() == 2 {
                            w.write("(($b) => $b.tag === \"Ok\" ? (");
                            self.expr(&args[1], ind, w);
                            w.write(")($b.value) : $b)(");
                            self.expr(&args[0], ind, w);
                            w.write(")");
                            return;
                        }
                        if n == "map-result" && args.len() == 2 {
                            let ok = self.runtime_ref("ok");
                            w.write(&format!("(($b) => $b.tag === \"Ok\" ? {ok}("));
                            self.expr(&args[1], ind, w);
                            w.write(")($b.value) : $b)(");
                            self.expr(&args[0], ind, w);
                            w.write(")");
                            return;
                        }
                        if n == "traverse" && args.len() == 2 {
                            let ok = self.runtime_ref("ok");
                            w.write("(($arr) => { const $out = []; for (const $x of ");
                            self.expr(&args[0], ind, w);
                            w.write(") { const $r = (");
                            self.expr(&args[1], ind, w);
                            w.write(")($x); if ($r.tag === \"Err\") return $r; $out.push($r.value); } return ");
                            w.write(&ok);
                            w.write("($out); })()");
                            return;
                        }
                    }
                }
                self.expr(f, ind, w);
                w.write("(");
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        w.write(", ");
                    }
                    self.expr(a, ind, w);
                }
                w.write(")");
            }
            Core::Let {
                name, rhs, body, ..
            } => {
                if self.in_async {
                    w.write(&format!("(await (async ({}) => {{\n", js_name(name)));
                    self.stmt(body, &format!("{ind}  "), w);
                    w.write(&format!("{ind}}})("));
                    self.expr(rhs, ind, w);
                    w.write("))");
                } else {
                    w.write(&format!("(({}) => {{\n", js_name(name)));
                    self.stmt(body, &format!("{ind}  "), w);
                    w.write(&format!("{ind}}})("));
                    self.expr(rhs, ind, w);
                    w.write(")");
                }
            }
            Core::Letrec {
                name, lam, body, ..
            } => {
                // design §2.1: JS 生成は自己参照できる名前付き関数式で直接書像する。
                let js = js_name(name);
                if self.in_async {
                    w.write("(async () => {\n");
                } else {
                    w.write("(() => {\n");
                }
                if let Core::Lam {
                    params,
                    body: lam_body,
                    ..
                } = lam.as_ref()
                {
                    let params_js = params
                        .iter()
                        .map(|p| js_name(&p.name))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let lam_async =
                        core_has_effect_call(lam_body) || core_calls_impure(lam_body, &self.impure_defs);
                    let async_kw = if lam_async { "async " } else { "" };
                    w.write(&format!(
                        "{ind}  const {js} = {async_kw}function {js}({params_js}) {{\n"
                    ));
                    let saved_tail = self.cur_tail.take();
                    let saved_async = self.in_async;
                    self.in_async = lam_async;
                    self.stmt(lam_body, &format!("{ind}    "), w);
                    self.in_async = saved_async;
                    self.cur_tail = saved_tail;
                    w.write(&format!("{ind}  }};\n"));
                } else {
                    // check で type-mismatch になるはず。emit 単体では明示エラー。
                    w.write(&format!(
                        "{ind}  throw new Error(\"letrec binding must be fn\");\n"
                    ));
                }
                w.write(&format!("{ind}  return "));
                self.expr(body, &format!("{ind}  "), w);
                w.write(";\n})()");
            }
            Core::If {
                cond,
                then,
                else_,
                ..
            } => {
                w.write("(");
                self.expr(cond, ind, w);
                w.write(" ? ");
                self.expr(then, ind, w);
                w.write(" : ");
                self.expr(else_, ind, w);
                w.write(")");
            }
            Core::Match { .. } => {
                let s = format!("$s{}", self.fresh());
                if self.in_async {
                    w.write(&format!("(await (async ({s}) => {{\n"));
                    self.match_tail(e, &format!("{ind}  "), w, Some(&s));
                    w.write(&format!("{ind}}})("));
                    if let Core::Match { scrutinee, .. } = e {
                        self.expr(scrutinee, ind, w);
                    }
                    w.write("))");
                } else {
                    w.write(&format!("(({s}) => {{\n"));
                    self.match_tail(e, &format!("{ind}  "), w, Some(&s));
                    w.write(&format!("{ind}}})("));
                    if let Core::Match { scrutinee, .. } = e {
                        self.expr(scrutinee, ind, w);
                    }
                    w.write(")");
                }
            }
            Core::Obj { fields, .. } => {
                w.write("{ ");
                for (i, (n, c)) in fields.iter().enumerate() {
                    if i > 0 {
                        w.write(", ");
                    }
                    w.write(&format!("{}: ", js_name(n)));
                    self.expr(c, ind, w);
                }
                w.write(" }");
            }
            Core::Field { obj, name, .. } => {
                w.write("(");
                self.expr(obj, ind, w);
                w.write(&format!(").{}", js_name(name)));
            }
            Core::Do { .. } => {
                if self.in_async {
                    w.write("(await (async () => {\n");
                    self.do_stmt(e, &format!("{ind}  "), w);
                    w.write(&format!("{ind}}})())"));
                } else {
                    w.write("(() => {\n");
                    self.do_stmt(e, &format!("{ind}  "), w);
                    w.write(&format!("{ind}}})()"));
                }
            }
            Core::EffectCall { effect, args, .. } => {
                // design §1.5/§4: 組み込みエフェクトは host なしで直接書像
                if crate::effects::is_builtin_effect(effect) {
                    match effect.as_ref() {
                        "ref" => {
                            w.write("$ref_new(");
                            self.expr(&args[0], ind, w);
                            w.write(")");
                        }
                        "ref-get" => {
                            w.write("(");
                            self.expr(&args[0], ind, w);
                            w.write(").value");
                        }
                        "ref-set" => {
                            w.write("((");
                            self.expr(&args[0], ind, w);
                            w.write(").value = ");
                            self.expr(&args[1], ind, w);
                            w.write(", null)");
                        }
                        // §4: now! → Date.now()、elapsed! → Date.now() - $BOOT_NOW、trace! → console.error
                        "now" => w.write("Date.now()"),
                        "elapsed" => w.write("(Date.now() - $BOOT_NOW)"),
                        "trace" => {
                            // console.error は undefined を返すので nil（null）にする（do の Err 検査対策）
                            w.write("(console.error(");
                            self.expr(&args[0], ind, w);
                            w.write("), null)");
                        }
                        // §4.1: read-file! → Node fs（非同期）。差分テスト対象外（FS は非決定的）
                        "read-file" => {
                            w.write("$read_file(");
                            self.expr(&args[0], ind, w);
                            w.write(")");
                        }
                        _ => unreachable!(),
                    }
                    return;
                }
                if self.in_async {
                    // §12.4: EffectCall -> `await host.name(args)`（host は囲む defn! の第1引数 §12.6）。
                    // 外側の expr ラッパが (await ...) で包む。
                    let h = self
                        .host_stack
                        .last()
                        .cloned()
                        .unwrap_or_else(|| "host".to_string());
                    w.write(&format!("{h}{}(", effect_prop(effect)));
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            w.write(", ");
                        }
                        self.expr(a, ind, w);
                    }
                    w.write(")");
                } else {
                    w.write(&format!(
                        "(() => {{ throw new Error({}); }})()",
                        serde_json::to_string(&format!(
                            "エフェクト {effect}! は defn! 本体の外では実行できません"
                        ))
                        .unwrap_or_else(|_| "\"effect\"".to_string())
                    ));
                }
            }
            Core::Error { message, .. } => {
                w.write(&format!(
                    "(() => {{ throw new Error({}); }})()",
                    serde_json::to_string(message).unwrap_or_else(|_| "\"error\"".to_string())
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desugar::desugar;
    use crate::lexer::tokenize;
    use crate::parser::parse_program;

    fn emit_src(src: &str) -> Result<EmitOutput, Vec<Diagnostic>> {
        let lex = tokenize(src, "test.lts");
        let (program, _) = parse_program(&lex);
        let (defs, _) = desugar(&program);
        emit(&EmitInput {
            defs,
            file: "test.js".to_string(),
            source_file: "test.lts".to_string(),
            type_names: BTreeSet::new(),
            impure_names: BTreeSet::new(),
            seed: crate::rng::DEFAULT_SEED,
            aliases: Default::default(),
        })
    }

    #[test]
    fn js_name_table() {
        assert_eq!(js_name("add-money"), "add_money");
        assert_eq!(js_name("contains?"), "contains_q");
        assert_eq!(js_name("any?"), "any_q");
        assert_eq!(js_name("class"), "$class");
        assert_eq!(js_name("new"), "$new");
        assert_eq!(js_name("zero"), "zero");
        assert_eq!(js_name("fold-left"), "fold_left");
        assert_eq!(js_name("obj->map"), "obj_to_map");
        assert_eq!(js_name("set->array"), "set_to_array");
        assert_eq!(js_name("char<=?"), "char_le__q");
        assert_eq!(js_name("char=?"), "char_eq__q");
    }

    #[test]
    fn do_lowers_to_early_return() {
        let out = emit_src(
            "(defn f (items : (Array number)) : (Result number string) (do [_ (g items)] [t (h items)] (ok t)))",
        )
        .expect("emit");
        assert!(out.js.contains(".tag === \"Err\") return $r"), "{}", out.js);
        assert!(out.js.contains("const $r1 = "), "{}", out.js);
    }

    #[test]
    fn bind_map_result_traverse_are_inlined() {
        let out = emit_src(
            "(defn f (x : (Result number string)) : (Result number string) (bind x (fn (n : number) : (Result number string) (map-result x (fn (m : number) : number m)))))",
        )
        .expect("emit");
        assert!(out.js.contains("$b.tag === \"Ok\""), "{}", out.js);
        assert!(!out.js.contains("$stdlib_bind"), "{}", out.js);
        assert!(!out.js.contains("$stdlib_map_result"), "{}", out.js);
    }

    #[test]
    fn defn_bang_emits_async_function_with_host_call() {
        let out = emit_src(
            "(effect log : (fn (s : string) : nil))\n(defn! f (host : (obj (log : (fn (s : string) : nil)))) (s : string) : nil (log! s))",
        )
        .expect("defn! は P1 で生成できる");
        assert!(out.js.contains("async function f(host, s)"), "{}", out.js);
        assert!(out.js.contains("(await host.log(s))"), "{}", out.js);
    }

    #[test]
    fn defn_bang_example_is_awaited_and_kebab_effect_uses_snake_case_key() {
        let out = emit_src(
            "(effect db-write : (fn (s : string) : nil))\n(defn! f (host : (obj (db-write : (fn (s : string) : nil)))) : nil (db-write! \"x\"))\n(example (f (obj (db-write (fn (s : string) : nil nil)))) => nil)",
        )
        .expect("emit");
        assert!(out.js.contains("(await host.db_write(\"x\"))"), "{}", out.js);
        assert!(out.js.contains("const actual = (await f("), "{}", out.js);
    }

    #[test]
    fn pure_module_has_no_async_anywhere() {
        let out = emit_src(
            "(defn add-money (a : number) : number (+ a 1))\n(example (add-money 1) => 2)",
        )
        .expect("emit");
        assert!(!out.js.contains("async"), "{}", out.js);
        assert!(!out.js.contains("await"), "{}", out.js);
        assert!(out.js.contains("const actual = add_money(1);"), "{}", out.js);
    }

    #[test]
    fn emit_is_deterministic() {
        let src = "(defn add-money (a : number) : number (+ a 1))\n(example (add-money 1) => 2)";
        let a = emit_src(src).expect("a");
        let b = emit_src(src).expect("b");
        assert_eq!(a.js, b.js);
        assert_eq!(a.map, b.map);
        let map_json: serde_json::Value = serde_json::from_str(&a.map).expect("map json");
        assert_eq!(map_json["version"], 3);
        assert_eq!(map_json["sources"], serde_json::json!(["test.lts"]));
    }

    #[test]
    fn option_and_split_stdlib_lowering_is_raw_and_scalar_based() {
        let out = emit_src(
            "(defn f (arr : (Array number)) : (Option number) (find arr (fn (x : number) : bool (> x 0))))\n(defn g (arr : (Array number)) (i : number) : (Option number) (nth arr i))\n(defn h (s : string) (t : string) : (Array string) (split s t))",
        )
        .expect("emit");
        assert!(out.js.contains("? arr[k] : null"), "{}", out.js);
        assert!(!out.js.contains("{ok}(arr[k])"), "{}", out.js);
        assert!(out.js.contains("if (f(x)) return x; return null;"), "{}", out.js);
        assert!(out.js.contains("t === \"\" ? [...s] : s.split(t)"), "{}", out.js);
    }

    #[test]
    fn decimal_literals_lower_to_bigint_and_helpers_are_emitted() {
        let out = emit_src(
            "(defn f (a : decimal) (b : decimal) : decimal (* a b))\n(example (f 1.50d 2.00d) => 3.00d)",
        )
        .expect("emit");
        assert!(out.js.contains("1500000000000000000n"), "{}", out.js);
        assert!(out.js.contains("$dec_mul"), "{}", out.js);
        assert!(out.js.contains("$json(actual)"), "{}", out.js);
    }

    #[test]
    fn pure_number_module_has_no_decimal_helpers() {
        let out = emit_src("(defn add (a : number) (b : number) : number (+ a b))\n(example (add 1 2) => 3)").expect("emit");
        assert!(!out.js.contains("$dec_mul"), "{}", out.js);
        assert!(!out.js.contains("bigint"), "{}", out.js);
        assert!(out.js.contains("const $stdlib_add = (a, b) => a + b;"), "{}", out.js);
    }
}
