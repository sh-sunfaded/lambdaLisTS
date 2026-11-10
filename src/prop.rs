//! prop 評価（M22, SPEC §3.7/§10.1 の P1 スコープ）: forall 束縛の型から値を生成し、
//! 既定回数だけ prop 本体を実行する。失敗時は反例を縮小して機械可読 JSON で返す。
//! 乱数は自前 PRNG（rng.rs）＋明示シードで、同じシードなら同じ生成列・同じ反例（決定性 MUST）。

use std::rc::Rc;

use serde_json::{json, Value as Json};

use crate::backends::value_to_json;
use crate::ir::{Core, Param};
use crate::rng::Rng;
use crate::types::{normalize_type, AliasEnv, Type};
use crate::value::{print_value, Closure, Env, Value};
use crate::eval::Interp;

/// 既定の試行回数（SPEC への提案値: 100）。
pub const DEFAULT_MAX_TRIALS: usize = 100;

/// 再帰型の生成深さ上限。これを超えると最小値（nil/空/0）を選ぶ。
pub const MAX_GEN_DEPTH: usize = 6;

/// 文字列生成で使う文字種（ASCII + 空白 + ひらがな + 絵文字で Unicode を exercising）。
const STR_CHARSET: &[char] = &['a', 'b', '0', '1', ' ', 'あ', '😀'];

/// number 生成で優先して選ぶ境界値。
const NUM_BOUNDARIES: &[f64] = &[0.0, 1.0, -1.0, 9007199254740992.0, -9007199254740992.0];

/// 型から値を生成できるか（関数型・型変数は不可）。
pub fn is_generatable(ty: &Type, aliases: &AliasEnv) -> bool {
    match normalize_type(ty, aliases) {
        Type::FnT(..) | Type::Var(_) => false,
        Type::Array(t) => is_generatable(&t, aliases),
        Type::Map(k, v) => is_generatable(&k, aliases) && is_generatable(&v, aliases),
        Type::Set(k) => is_generatable(&k, aliases),
        Type::Builder => true,
        Type::Bytes => true,
        Type::Ref(_) => false,
        Type::Union(ms) => ms.iter().all(|m| is_generatable(m, aliases)),
        Type::Obj(fields) => fields.values().all(|t| is_generatable(t, aliases)),
        Type::ResultT(t, e) => is_generatable(&t, aliases) && is_generatable(&e, aliases),
        _ => true,
    }
}

/// 型からランダムな値を生成する。depth が上限なら最小値を返す。
pub fn gen_value(ty: &Type, aliases: &AliasEnv, rng: &mut Rng, depth: usize) -> Option<Value> {
    if depth > MAX_GEN_DEPTH {
        return min_value(ty, aliases);
    }
    match normalize_type(ty, aliases) {
        Type::Nil => Some(Value::Nil),
        Type::Bool => Some(Value::Bool(rng.gen_bool())),
        Type::Dec => {
            // decimal: 境界 {0, ±1単位, ±1.0, ±DEC_MAX} を半分、他は ±1.0 の範囲
            if rng.gen_bool() {
                let b = [
                    0i128,
                    1,
                    -1,
                    crate::decimal::S,
                    -crate::decimal::S,
                    crate::decimal::DEC_MAX,
                    -crate::decimal::DEC_MAX,
                ];
                Some(Value::Dec(b[rng.gen_range(0, 6) as usize]))
            } else {
                Some(Value::Dec(
                    rng.gen_range(0, 2 * crate::decimal::S as u64) as i128
                        - crate::decimal::S,
                ))
            }
        }
        Type::Number => {
            if rng.gen_bool() {
                let b = NUM_BOUNDARIES[rng.gen_range(0, (NUM_BOUNDARIES.len() - 1) as u64) as usize];
                Some(Value::Num(b))
            } else {
                Some(Value::Num(rng.gen_range(0, 200) as i64 as f64 - 100.0))
            }
        }
        Type::Str => {
            let len = rng.gen_range(0, 8) as usize;
            let mut s = String::new();
            for _ in 0..len {
                let c = STR_CHARSET[rng.gen_range(0, (STR_CHARSET.len() - 1) as u64) as usize];
                s.push(c);
            }
            Some(Value::Str(Rc::from(s)))
        }
        Type::Char => {
            let idx = rng.gen_range(0, (STR_CHARSET.len() - 1) as u64) as usize;
            Some(Value::Char(STR_CHARSET[idx]))
        }
        Type::Literal(lit) => Some(Value::Str(lit)),
        Type::Array(t) => {
            let len = if rng.gen_range(0, 2) == 0 { 0 } else { rng.gen_range(0, 6) as usize };
            let mut out = Vec::with_capacity(len);
            for _ in 0..len {
                out.push(gen_value(&t, aliases, rng, depth + 1)?);
            }
            Some(Value::Array(out))
        }
        Type::Obj(fields) => {
            let mut out = std::collections::BTreeMap::new();
            for (k, v) in &fields {
                out.insert(k.clone(), gen_value(v, aliases, rng, depth + 1)?);
            }
            Some(Value::Obj(out))
        }
        Type::Map(k, v) => {
            let n = rng.gen_range(0, 4) as usize;
            let mut m = crate::pmap::PersistentMap::new();
            for _ in 0..n {
                let key = gen_value(&k, aliases, rng, depth + 1)?;
                let val = gen_value(&v, aliases, rng, depth + 1)?;
                let Some(mk) = crate::pmap::MapKey::from_value(&key) else {
                    return None;
                };
                let (next, _) = m.insert(mk, val);
                m = next;
            }
            Some(Value::Map(Rc::new(m)))
        }
        Type::Set(k) => {
            let n = rng.gen_range(0, 4) as usize;
            let mut m = crate::pmap::PersistentMap::new();
            for _ in 0..n {
                let key = gen_value(&k, aliases, rng, depth + 1)?;
                let Some(mk) = crate::pmap::MapKey::from_value(&key) else {
                    return None;
                };
                let (next, _) = m.insert(mk, Value::Nil);
                m = next;
            }
            Some(Value::Set(Rc::new(m)))
        }
        Type::Builder => {
            let n = rng.gen_range(0, 4) as usize;
            let mut parts = Vec::with_capacity(n);
            for _ in 0..n {
                if let Some(Value::Str(st)) = gen_value(&Type::Str, aliases, rng, depth + 1) {
                    parts.push(st.to_string());
                }
            }
            Some(Value::Builder(Rc::new(parts)))
        }
        Type::Bytes => {
            let n = rng.gen_range(0, 5) as usize;
            let mut b = Vec::with_capacity(n);
            for _ in 0..n {
                b.push(rng.gen_range(0, 256) as u8);
            }
            Some(Value::Bytes(Rc::new(b)))
        }
        Type::Ref(_) => None,
        Type::Union(members) => {
            let idx = rng.gen_range(0, (members.len().max(1) - 1) as u64) as usize;
            gen_value(&members[idx], aliases, rng, depth + 1)
        }
        Type::ResultT(t, e) => {
            if rng.gen_bool() {
                Some(Value::Ok(Box::new(gen_value(&t, aliases, rng, depth + 1)?)))
            } else {
                Some(Value::Err(Box::new(gen_value(&e, aliases, rng, depth + 1)?)))
            }
        }
        Type::FnT(..) | Type::Var(_) | Type::Alias(_) => None,
    }
}

/// 深さ上限で使う最小値（縮小の到達点とも一致させる）。
pub fn min_value(ty: &Type, aliases: &AliasEnv) -> Option<Value> {
    match normalize_type(ty, aliases) {
        Type::Nil => Some(Value::Nil),
        Type::Bool => Some(Value::Bool(false)),
        Type::Number => Some(Value::Num(0.0)),
        Type::Dec => Some(Value::Dec(0)),
        Type::Str => Some(Value::Str(Rc::from(""))),
        Type::Char => Some(Value::Char('a')),
        Type::Literal(lit) => Some(Value::Str(lit)),
        Type::Array(_) => Some(Value::Array(Vec::new())),
        Type::Map(..) => Some(Value::Map(Rc::new(crate::pmap::PersistentMap::new()))),
        Type::Set(_) => Some(Value::Set(Rc::new(crate::pmap::PersistentMap::new()))),
        Type::Builder => Some(Value::Builder(Rc::new(Vec::new()))),
        Type::Bytes => Some(Value::Bytes(Rc::new(Vec::new()))),
        Type::Ref(_) => None,
        Type::Obj(fields) => {
            let mut out = std::collections::BTreeMap::new();
            for (k, v) in &fields {
                out.insert(k.clone(), min_value(v, aliases)?);
            }
            Some(Value::Obj(out))
        }
        Type::Union(members) => {
            if members.iter().any(|m| matches!(normalize_type(m, aliases), Type::Nil)) {
                return Some(Value::Nil);
            }
            members.first().and_then(|m| min_value(m, aliases))
        }
        Type::ResultT(_, e) => Some(Value::Err(Box::new(min_value(&e, aliases)?))),
        Type::FnT(..) | Type::Var(_) | Type::Alias(_) => None,
    }
}

/// 値の「大きさ」（縮小の優劣判定用）。
pub fn value_size(v: &Value) -> usize {
    match v {
        Value::Nil | Value::Bool(_) => 0,
        Value::Dec(s) => {
            if *s == 0 {
                0
            } else {
                1 + format!("{}", s.unsigned_abs()).len()
            }
        }
        Value::Num(n) => {
            if *n == 0.0 {
                0
            } else {
                1 + format!("{}", n.abs()).len()
            }
        }
        Value::Str(s) => s.chars().count(),
        Value::Char(_) => 1,
        Value::Builder(parts) => 1 + parts.iter().map(|p| p.chars().count()).sum::<usize>(),
        Value::Bytes(b) => 1 + b.len(),
        Value::Ref(_) => usize::MAX / 2,
        Value::Array(xs) => 1 + xs.iter().map(value_size).sum::<usize>(),
        Value::Obj(m) => 1 + m.values().map(value_size).sum::<usize>(),
        Value::Map(m) => 1 + m.entries().iter().map(|(k, v)| value_size(&k.to_value()) + value_size(v)).sum::<usize>(),
        Value::Set(s) => 1 + s.keys().iter().map(|k| value_size(&k.to_value())).sum::<usize>(),
        Value::Ok(x) | Value::Err(x) => 1 + value_size(x),
        Value::Closure(_) | Value::Builtin(_) => usize::MAX / 2,
    }
}

/// 縮小候補（小さい順序ではなく候補列。呼び側で size を見て絞る）。
/// number は 0 方向へ、Array は要素削減、string は短縮。
pub fn shrink_candidates(v: &Value, ty: &Type, aliases: &AliasEnv) -> Vec<Value> {
    let mut out = Vec::new();
    match (v, normalize_type(ty, aliases)) {
        (Value::Dec(s), Type::Dec) => {
            if *s != 0 {
                out.push(Value::Dec(0));
                let half = s / 2;
                if half != *s {
                    out.push(Value::Dec(half));
                }
            }
        }
        (Value::Num(n), Type::Number) => {
            if *n != 0.0 {
                out.push(Value::Num(0.0));
                let half = n / 2.0;
                if half != *n {
                    out.push(Value::Num(half));
                }
            }
        }
        (Value::Str(s), Type::Str) => {
            let chars: Vec<char> = s.chars().collect();
            if !chars.is_empty() {
                out.push(Value::Str(Rc::from("")));
                let half: String = chars[..chars.len() / 2].iter().collect();
                if half.len() < s.len() {
                    out.push(Value::Str(Rc::from(half.as_str())));
                }
                let drop_last: String = chars[..chars.len() - 1].iter().collect();
                out.push(Value::Str(Rc::from(drop_last.as_str())));
            }
        }
        (Value::Array(xs), Type::Array(t)) => {
            if !xs.is_empty() {
                out.push(Value::Array(Vec::new()));
                out.push(Value::Array(xs[..xs.len() - 1].to_vec()));
                out.push(Value::Array(xs[1..].to_vec()));
                let half = &xs[..xs.len() / 2];
                if half.len() < xs.len() {
                    out.push(Value::Array(half.to_vec()));
                }
                // 要素ごとの縮小も試す（各要素の第一候補）
                for (i, x) in xs.iter().enumerate() {
                    if let Some(c) = shrink_candidates(x, &t, aliases).into_iter().next() {
                        if value_size(&c) < value_size(x) {
                            let mut xs2 = xs.clone();
                            xs2[i] = c;
                            out.push(Value::Array(xs2));
                        }
                    }
                }
            }
        }
        (Value::Bool(b), Type::Bool) => {
            if *b {
                out.push(Value::Bool(false));
            }
        }
        (Value::Obj(m), Type::Obj(fields)) => {
            for (k, x) in m {
                if let Some(ft) = fields.get(k) {
                    for c in shrink_candidates(x, ft, aliases) {
                        if value_size(&c) < value_size(x) {
                            let mut m2 = m.clone();
                            m2.insert(k.clone(), c);
                            out.push(Value::Obj(m2));
                        }
                    }
                }
            }
        }
        (Value::Ok(x), Type::ResultT(t, _)) => {
            for c in shrink_candidates(x, &t, aliases) {
                if value_size(&c) < value_size(x) {
                    out.push(Value::Ok(Box::new(c)));
                }
            }
        }
        (Value::Err(x), Type::ResultT(_, e)) => {
            for c in shrink_candidates(x, &e, aliases) {
                if value_size(&c) < value_size(x) {
                    out.push(Value::Err(Box::new(c)));
                }
            }
        }
        (v, Type::Union(members)) => {
            for m in &members {
                for c in shrink_candidates(v, m, aliases) {
                    if value_size(&c) < value_size(v) {
                        out.push(c);
                    }
                }
            }
        }
        _ => {}
    }
    out
}

#[derive(Debug)]
pub enum PropOutcome {
    Pass { trials: usize },
    Fail {
        trials: usize,
        counterexample: Vec<(Rc<str>, Value)>,
        shrunk: Vec<(Rc<str>, Value)>,
        error: Option<String>,
    },
    NotGeneratable { param: Rc<str> },
}

/// prop を評価する。seed が同じなら生成列も反例も同じ（決定性）。
pub fn run_prop(
    name: &str,
    params: &[Param],
    body: &Core,
    env: &Rc<Env>,
    aliases: &AliasEnv,
    seed: u64,
    max_trials: usize,
) -> PropOutcome {
    for p in params {
        if !is_generatable(&p.ty, aliases) {
            return PropOutcome::NotGeneratable { param: p.name.clone() };
        }
    }
    let f = Value::Closure(Rc::new(Closure {
        params: params.to_vec(),
        body: Rc::new(body.clone()),
        env: env.clone(),
        name: Some(Rc::from(name)),
    }));
    let mut interp = Interp::new();
    let mut rng = Rng::new(seed);
    let mut trials = 0;
    while trials < max_trials {
        trials += 1;
        let args: Vec<Value> = params
            .iter()
            .map(|p| gen_value(&p.ty, aliases, &mut rng, 0).unwrap_or(Value::Nil))
            .collect();
        match interp.apply(f.clone(), args.clone()) {
            Ok(Value::Bool(true)) => continue,
            Ok(Value::Bool(false)) => {
                let (cex, shrunk) = shrink_prop(&mut interp, &f, params, aliases, args);
                return PropOutcome::Fail {
                    trials,
                    counterexample: params.iter().map(|p| p.name.clone()).zip(cex).collect(),
                    shrunk: params.iter().map(|p| p.name.clone()).zip(shrunk).collect(),
                    error: None,
                };
            }
            Ok(other) => {
                let (cex, shrunk) = shrink_prop(&mut interp, &f, params, aliases, args);
                return PropOutcome::Fail {
                    trials,
                    counterexample: params.iter().map(|p| p.name.clone()).zip(cex).collect(),
                    shrunk: params.iter().map(|p| p.name.clone()).zip(shrunk).collect(),
                    error: Some(format!("prop 本体は bool を返すべきですが {} が返りました", print_value(&other))),
                };
            }
            Err(e) => {
                let (cex, shrunk) = shrink_prop(&mut interp, &f, params, aliases, args);
                return PropOutcome::Fail {
                    trials,
                    counterexample: params.iter().map(|p| p.name.clone()).zip(cex).collect(),
                    shrunk: params.iter().map(|p| p.name.clone()).zip(shrunk).collect(),
                    error: Some(e.to_string()),
                };
            }
        }
    }
    PropOutcome::Pass { trials }
}

/// 反例を貪欲に縮小する（各パラメータを順に、失敗し続けるより小さい候補へ置き換える）。
fn shrink_prop(
    interp: &mut Interp,
    f: &Value,
    params: &[Param],
    aliases: &AliasEnv,
    mut args: Vec<Value>,
) -> (Vec<Value>, Vec<Value>) {
    let fails = |interp: &mut Interp, args: &Vec<Value>| -> bool {
        !matches!(interp.apply(f.clone(), args.clone()), Ok(Value::Bool(true)))
    };
    loop {
        let mut improved = false;
        'outer: for i in 0..args.len() {
            for cand in shrink_candidates(&args[i], &params[i].ty, aliases) {
                if value_size(&cand) >= value_size(&args[i]) {
                    continue;
                }
                let mut trial = args.clone();
                trial[i] = cand;
                if fails(interp, &trial) {
                    args = trial;
                    improved = true;
                    break 'outer;
                }
            }
        }
        if !improved {
            break;
        }
    }
    let shrunk = args.clone();
    (args, shrunk)
}

/// 値を λ リテラルとして印字（example に貼れる形。文字列は引用符つき）。
pub fn value_to_lit(v: &Value) -> String {
    match v {
        Value::Str(s) => serde_json::to_string(s.as_ref()).unwrap_or_else(|_| format!("\"{s}\"")),
        Value::Num(_) => print_value(v),
        Value::Bool(b) => b.to_string(),
        Value::Nil => "nil".to_string(),
        Value::Array(xs) => format!("(array {})", xs.iter().map(value_to_lit).collect::<Vec<_>>().join(" ")),
        Value::Obj(m) => format!(
            "(obj {})",
            m.iter()
                .map(|(k, val)| format!("({k} {})", value_to_lit(val)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Value::Ok(x) => format!("(ok {})", value_to_lit(x)),
        Value::Err(x) => format!("(err {})", value_to_lit(x)),
        // map/set にはリテラル構文が無いため print_value 形式で印字（REPORT.md に記録）
        Value::Map(_) | Value::Set(_) => print_value(v),
        Value::Ref(_) => "<ref>".to_string(),
        // bytes にはリテラル構文が無いので印字形式で（REPORT.md 記録）
        Value::Bytes(_) => print_value(v),
        Value::Builder(parts) => format!(
            "(array {})",
            parts.iter().map(|p| serde_json::to_string(p).unwrap_or_else(|_| format!("\"{p}\""))).collect::<Vec<_>>().join(" ")
        ),
        // char はリテラル構文 `\<文字>` があるのでそれで印字
        Value::Char(c) => crate::fmt::print_char_lit(*c),
        Value::Dec(s) => crate::decimal::fmt_dec(*s),
        Value::Closure(_) | Value::Builtin(_) => "<fn>".to_string(),
    }
}

/// Core 式の最小限の印字（fix テンプレート用。未対応フォームは <expr>）。
pub fn print_core(e: &Core) -> String {
    match e {
        Core::Lit { value, .. } => value_to_lit(value),
        Core::Var { name, .. } => name.to_string(),
        Core::App { f, args, .. } => format!(
            "({} {})",
            print_core(f),
            args.iter().map(print_core).collect::<Vec<_>>().join(" ")
        ),
        Core::If { cond, then, else_, .. } => {
            format!("(if {} {} {})", print_core(cond), print_core(then), print_core(else_))
        }
        Core::Let { name, rhs, body, .. } => {
            format!("(let [{} {}] {})", name, print_core(rhs), print_core(body))
        }
        Core::Field { obj, name, .. } => format!("{}.{}", print_core(obj), name),
        Core::Obj { fields, .. } => format!(
            "(obj {})",
            fields
                .iter()
                .map(|(k, c)| format!("({k} {})", print_core(c)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        _ => "<expr>".to_string(),
    }
}

/// forall パラメータを値で置き換えた本体（fix テンプレート用）。
pub fn subst_params(body: &Core, params: &[Param], values: &[Value]) -> Core {
    fn go(e: &Core, map: &std::collections::BTreeMap<Rc<str>, Value>) -> Core {
        match e {
            Core::Var { name, span } => {
                if let Some(v) = map.get(name) {
                    Core::Lit { value: v.clone(), span: span.clone() }
                } else {
                    e.clone()
                }
            }
            Core::App { f, args, span } => Core::App {
                f: Box::new(go(f, map)),
                args: args.iter().map(|a| go(a, map)).collect(),
                span: span.clone(),
            },
            Core::Let { name, rhs, body, span } => Core::Let {
                name: name.clone(),
                rhs: Box::new(go(rhs, map)),
                body: Box::new(go(body, map)),
                span: span.clone(),
            },
            Core::If { cond, then, else_, span } => Core::If {
                cond: Box::new(go(cond, map)),
                then: Box::new(go(then, map)),
                else_: Box::new(go(else_, map)),
                span: span.clone(),
            },
            Core::Obj { fields, span } => Core::Obj {
                fields: fields.iter().map(|(k, c)| (k.clone(), go(c, map))).collect(),
                span: span.clone(),
            },
            Core::Field { obj, name, span } => Core::Field {
                obj: Box::new(go(obj, map)),
                name: name.clone(),
                span: span.clone(),
            },
            other => other.clone(),
        }
    }
    let map: std::collections::BTreeMap<Rc<str>, Value> = params
        .iter()
        .map(|p| p.name.clone())
        .zip(values.iter().cloned())
        .collect();
    go(body, &map)
}

/// REPL prop op の応答 JSON。失敗時は反例・縮小後の値・施行回数・シードを機械可読で返す。
pub fn prop_result_json(id: Json, name: &str, seed: u64, params: &[Param], body: &Core, outcome: PropOutcome) -> Json {
    match outcome {
        PropOutcome::Pass { trials } => json!({
            "id": id, "ok": true, "prop": name, "seed": seed, "trials": trials
        }),
        PropOutcome::NotGeneratable { param } => json!({
            "id": id, "ok": false,
            "errors": [{ "kind": "type-mismatch",
                "message": format!("prop '{name}' の forall パラメータ '{param}' は生成できない型です（関数型など）") }]
        }),
        PropOutcome::Fail { trials, counterexample, shrunk, error } => {
            let cex: serde_json::Map<String, Json> = counterexample
                .iter()
                .map(|(n, v)| (n.to_string(), value_to_json(v)))
                .collect();
            let shr: serde_json::Map<String, Json> = shrunk
                .iter()
                .map(|(n, v)| (n.to_string(), value_to_json(v)))
                .collect();
            let shrunk_vals: Vec<Value> = shrunk.iter().map(|(_, v)| v.clone()).collect();
            let body_lit = print_core(&subst_params(body, params, &shrunk_vals));
            let mut out = json!({
                "id": id, "ok": false, "prop": name, "seed": seed, "trials": trials,
                "counterexample": Json::Object(cex),
                "shrunk": Json::Object(shr),
                "fix": {
                    "op": "insert",
                    "path": format!("(prop {name})"),
                    "template": format!("(example {body_lit} => false)")
                }
            });
            if let Some(e) = error {
                out["error"] = json!(e);
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{check_file, ModuleCache};
    use crate::desugar::CoreDef;
    use crate::eval::{global_env, load_file};
    use crate::lexer::tokenize;
    use crate::parser::parse_program;
    use crate::desugar::desugar;
    use std::collections::BTreeSet;

    fn defs_of(src: &str) -> Vec<CoreDef> {
        let lex = tokenize(src, "test.lts");
        let (program, _) = parse_program(&lex);
        desugar(&program).0
    }

    fn check_source(src: &str, cache: &mut ModuleCache) -> crate::check::ModuleResult {
        crate::check::check_source(src, "test.lts", cache)
    }

    fn run_src_prop(src: &str, prop_name: &str, seed: u64) -> PropOutcome {
        let defs = defs_of(src);
        let mut cache = ModuleCache::default();
        let res = check_source(src, &mut cache);
        assert!(res.diagnostics.is_empty(), "{:?}", res.diagnostics);
        let env = global_env();
        crate::eval::load_defs(&defs, &env);
        let prop = defs
            .iter()
            .find_map(|d| match d {
                CoreDef::Prop { name, params, body, .. } if name.as_ref() == prop_name => {
                    Some((params.clone(), body.clone()))
                }
                _ => None,
            })
            .expect("prop");
        run_prop(prop_name, &prop.0, &prop.1, &env, &res.type_aliases, seed, DEFAULT_MAX_TRIALS)
    }

    #[test]
    fn gen_number_uses_boundaries_and_range() {
        let aliases = AliasEnv::new();
        let mut rng = Rng::new(1);
        let mut seen_boundary = false;
        for _ in 0..200 {
            let Value::Num(n) = gen_value(&Type::Number, &aliases, &mut rng, 0).expect("num") else {
                panic!()
            };
            if NUM_BOUNDARIES.contains(&n) {
                seen_boundary = true;
            } else {
                assert!((-100.0..=100.0).contains(&n));
            }
        }
        assert!(seen_boundary);
    }

    #[test]
    fn gen_string_array_union_rules() {
        let aliases = AliasEnv::new();
        let mut rng = Rng::new(2);
        for _ in 0..100 {
            let Value::Str(s) = gen_value(&Type::Str, &aliases, &mut rng, 0).expect("str") else {
                panic!()
            };
            assert!(s.chars().count() <= 8);
            assert!(s.chars().all(|c| STR_CHARSET.contains(&c)));
            let Value::Array(xs) =
                gen_value(&Type::Array(Box::new(Type::Number)), &aliases, &mut rng, 0).expect("arr")
            else {
                panic!()
            };
            assert!(xs.len() <= 6);
        }
        // union は nil を含むなら深さ上限で nil
        let opt = Type::Union(vec![Type::Str, Type::Nil]);
        assert_eq!(min_value(&opt, &aliases), Some(Value::Nil));
        assert_eq!(min_value(&Type::Array(Box::new(Type::Number)), &aliases), Some(Value::Array(vec![])));
        assert!(!is_generatable(&Type::FnT(vec![], Box::new(Type::Nil)), &aliases));
        assert!(is_generatable(&Type::Array(Box::new(Type::Number)), &aliases));
    }

    #[test]
    fn shrink_moves_toward_zero_empty() {
        let aliases = AliasEnv::new();
        let c = shrink_candidates(&Value::Num(37.0), &Type::Number, &aliases);
        assert!(c.contains(&Value::Num(0.0)));
        let c = shrink_candidates(&Value::Str(Rc::from("abcd")), &Type::Str, &aliases);
        assert!(c.contains(&Value::Str(Rc::from(""))));
        let arr = Value::Array(vec![Value::Num(1.0), Value::Num(2.0), Value::Num(3.0)]);
        let c = shrink_candidates(&arr, &Type::Array(Box::new(Type::Number)), &aliases);
        assert!(c.contains(&Value::Array(vec![])));
        assert_eq!(value_size(&Value::Num(0.0)), 0);
        assert!(value_size(&arr) > 0);
    }

    #[test]
    fn prop_pass_and_fail_with_shrink_and_determinism() {
        // 真の prop: 通る
        let ok_src = "(defn f (x : number) : number (+ x 1)) (prop \"inc\" (forall [(x : number)] (= (f x) (+ x 1))))";
        assert!(matches!(
            run_src_prop(ok_src, "inc", 42),
            PropOutcome::Pass { trials: 100 }
        ));
        // 偽 prop: 反例があり、number は 0 へ縮小される
        let bad_src = "(prop \"neg\" (forall [(x : number)] (< x 0)))";
        let PropOutcome::Fail { trials, counterexample, shrunk, error } = run_src_prop(bad_src, "neg", 7) else {
            panic!("fail expected")
        };
        assert!(trials >= 1 && trials <= 100);
        assert!(error.is_none());
        assert_eq!(counterexample.len(), 1);
        assert_eq!(shrunk[0].1, Value::Num(0.0), "number は 0 へ縮小: {shrunk:?}");
        // 決定性: 同じシードで同じ反例・同じ縮小結果
        let again = run_src_prop(bad_src, "neg", 7);
        let PropOutcome::Fail { counterexample: c2, shrunk: s2, trials: t2, .. } = again else {
            panic!()
        };
        assert_eq!(counterexample, c2);
        assert_eq!(shrunk, s2);
        assert_eq!(trials, t2);
        // Array は空へ縮小
        let arr_src = "(prop \"nonempty\" (forall [(xs : (Array number))] (> (count xs) 0)))";
        let PropOutcome::Fail { shrunk, .. } = run_src_prop(arr_src, "nonempty", 3) else {
            panic!("fail expected")
        };
        assert_eq!(shrunk[0].1, Value::Array(vec![]), "Array は空へ縮小");
    }

    #[test]
    fn money_lts_prop_passes() {
        let path = format!("{}/tests/fixtures/money.lts", env!("CARGO_MANIFEST_DIR"));
        let mut cache = ModuleCache::default();
        let res = check_file(&path, &mut cache);
        assert!(res.diagnostics.is_empty(), "{:?}", res.diagnostics);
        let env = global_env();
        let mut seen = BTreeSet::new();
        let defs = load_file(&path, &env, &mut seen).expect("load");
        crate::eval::load_defs(&defs, &env);
        let (params, body) = defs
            .iter()
            .find_map(|d| match d {
                CoreDef::Prop { name, params, body, .. } if name.as_ref() == "add-money commutative" => {
                    Some((params.clone(), body.clone()))
                }
                _ => None,
            })
            .expect("prop");
        let outcome = run_prop("add-money commutative", &params, &body, &env, &res.type_aliases, crate::rng::DEFAULT_SEED, DEFAULT_MAX_TRIALS);
        assert!(matches!(outcome, PropOutcome::Pass { trials: 100 }), "{outcome:?}");
    }

    #[test]
    fn prop_result_json_is_machine_readable() {
        let src = "(prop \"neg\" (forall [(x : number)] (< x 0)))";
        let outcome = run_src_prop(src, "neg", 7);
        let defs = defs_of(src);
        let (params, body) = defs.iter().find_map(|d| match d {
            CoreDef::Prop { params, body, .. } => Some((params.clone(), body.clone())),
            _ => None,
        }).expect("prop");
        let j = prop_result_json(json!(1), "neg", 7, &params, &body, outcome);
        assert_eq!(j["ok"], false);
        assert_eq!(j["seed"], 7);
        assert!(j["trials"].is_number());
        assert_eq!(j["shrunk"]["x"], json!(0.0));
        assert!(j["counterexample"]["x"].is_number());
        assert!(j["fix"]["template"].as_str().expect("t").starts_with("(example (< 0 0)"));
        // value_to_lit は文字列を引用符つきで
        assert_eq!(value_to_lit(&Value::Str(Rc::from("a\"b"))), "\"a\\\"b\"");
    }
}
