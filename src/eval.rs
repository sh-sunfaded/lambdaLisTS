use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::desugar::{desugar, CoreDef};
use crate::ir::{Core, Pat, PatKind};
use crate::lexer::tokenize;
use crate::parser::parse_program;
use crate::resolver::resolve_import_path;
use crate::pmap::{MapKey, PersistentMap};
use crate::value::{print_value, structural_equal, Builtin, BuiltinKind, Closure, Env, Value};

fn map_v(v: &Value) -> Result<Rc<PersistentMap>, EvalError> {
    match v {
        Value::Map(m) => Ok(m.clone()),
        _ => Err(EvalError::Runtime("map が必要です".to_string())),
    }
}

fn set_v(v: &Value) -> Result<Rc<PersistentMap>, EvalError> {
    match v {
        Value::Set(s) => Ok(s.clone()),
        _ => Err(EvalError::Runtime("set が必要です".to_string())),
    }
}

fn map_key(v: &Value) -> Result<MapKey, EvalError> {
    MapKey::from_value(v).ok_or_else(|| {
        EvalError::Runtime("map/set のキーは string か number です（design/interp-capability.md §1.1）".to_string())
    })
}

fn not_found_value() -> Value {
    err_tag_value("not-found")
}

fn err_tag_value(tag: &str) -> Value {
    Value::Obj(BTreeMap::from([(
        Rc::from("tag"),
        Value::Str(Rc::from(tag)),
    )]))
}

/// 位置ベース API の開始位置クランプ（slice_bounds の start と同じ規則: NaN→0、負は len+x、len 超過は len）
fn clamp_start(len: usize, i: f64) -> usize {
    let len_f = len as f64;
    if i.is_nan() {
        0
    } else if i < 0.0 {
        (len_f + i).max(0.0) as usize
    } else {
        (i.min(len_f)) as usize
    }
}

fn char_v(v: &Value) -> Result<char, EvalError> {
    match v {
        Value::Char(c) => Ok(*c),
        _ => Err(EvalError::Runtime("char が必要です".to_string())),
    }
}

pub const MAX_CALL_DEPTH: usize = 10000;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EvalError {
    #[error("internal-error: {0}")]
    Internal(String),
    #[error("{0}")]
    Runtime(String),
}

#[derive(Default)]
pub struct Interp {
    pub depth: usize,
    pub calls: BTreeMap<String, usize>,
    /// design §4: モック host の決定的クロック（now!/elapsed!）。固定列 1700000000000 + 呼び出し回数*1000。
    pub clock_calls: u64,
    pub clock_base: Option<f64>,
    /// trace! の呼び出し記録（repl --host mock ではモック host のログと共有する）
    pub host_log: Rc<RefCell<Vec<crate::host::HostCall>>>,
    /// design §4.1: read-file! が読むモックの仮想ファイル（パス → バイト列）
    pub virtual_files: BTreeMap<String, Vec<u8>>,
}

/// モック now の固定列の基点（ミリ秒）
pub const MOCK_NOW_BASE: f64 = 1_700_000_000_000.0;

impl Interp {
    pub fn new() -> Self {
        Interp::default()
    }

    pub fn set_host_log(&mut self, log: Rc<RefCell<Vec<crate::host::HostCall>>>) {
        self.host_log = log;
    }

    /// 決定的な now 値（呼び出しごとに +1000ms）。初回値を base として記録する。
    fn mock_now(&mut self) -> f64 {
        let v = MOCK_NOW_BASE + self.clock_calls as f64 * 1000.0;
        self.clock_calls += 1;
        if self.clock_base.is_none() {
            self.clock_base = Some(v);
        }
        v
    }

    pub fn calls(&self) -> &BTreeMap<String, usize> {
        &self.calls
    }

    pub fn apply(&mut self, f: Value, args: Vec<Value>) -> Result<Value, EvalError> {
        match f {
            Value::Builtin(b) => self.apply_builtin(&b, args),
            Value::Closure(c) => {
                if c.params.len() != args.len() {
                    return Err(EvalError::Runtime(format!(
                        "引数の数が一致しません: '{}' は {} 引数",
                        c.name.as_deref().unwrap_or("anonymous"),
                        c.params.len()
                    )));
                }
                if let Some(name) = &c.name {
                    *self.calls.entry(name.to_string()).or_insert(0) += 1;
                }
                let vars: BTreeMap<Rc<str>, Value> = c
                    .params
                    .iter()
                    .map(|p| p.name.clone())
                    .zip(args)
                    .collect();
                let env = Env::child_of(&c.env, vars);
                eval_core(self, &c.body, &env)
            }
            _ => Err(EvalError::Runtime(
                "関数ではない値を呼び出しました".to_string(),
            )),
        }
    }

    fn apply_builtin(&mut self, b: &Builtin, args: Vec<Value>) -> Result<Value, EvalError> {
        let num = |v: &Value| -> Result<f64, EvalError> {
            match v {
                Value::Num(n) => Ok(*n),
                _ => Err(EvalError::Runtime("number が必要です".to_string())),
            }
        };
        let str_v = |v: &Value| -> Result<Rc<str>, EvalError> {
            match v {
                Value::Str(s) => Ok(s.clone()),
                _ => Err(EvalError::Runtime("string が必要です".to_string())),
            }
        };
        let arr_v = |v: &Value| -> Result<Vec<Value>, EvalError> {
            match v {
                Value::Array(a) => Ok(a.clone()),
                _ => Err(EvalError::Runtime("Array が必要です".to_string())),
            }
        };
        let num_pair = |args: &[Value]| -> Result<(f64, f64), EvalError> {
            match (&args[0], &args[1]) {
                (Value::Num(a), Value::Num(b)) => Ok((*a, *b)),
                (Value::Dec(_), Value::Dec(_)) => Err(EvalError::Runtime(
                    "decimal には number 用の演算を適用できません（to-num で変換してください）".to_string(),
                )),
                _ => Err(EvalError::Runtime(
                    "演算の引数は number または decimal の同一型である必要があります".to_string(),
                )),
            }
        };
        let bool_v = |v: &Value| -> Result<bool, EvalError> {
            match v {
                Value::Bool(b) => Ok(*b),
                _ => Err(EvalError::Runtime("bool が必要です".to_string())),
            }
        };
        let ok_v = |v: Value| Value::Ok(Box::new(v));
        let err_v = |v: Value| Value::Err(Box::new(v));
        match &b.kind {
            BuiltinKind::Add => {
                if let (Value::Dec(a), Value::Dec(b)) = (&args[0], &args[1]) {
                    return Ok(Value::Dec(
                        crate::decimal::dec_add(*a, *b).map_err(|e| EvalError::Runtime(e.to_string()))?,
                    ));
                }
                let (a, b) = num_pair(&args)?;
                Ok(Value::Num(a + b))
            }
            BuiltinKind::Sub => {
                if let (Value::Dec(a), Value::Dec(b)) = (&args[0], &args[1]) {
                    return Ok(Value::Dec(
                        crate::decimal::dec_sub(*a, *b).map_err(|e| EvalError::Runtime(e.to_string()))?,
                    ));
                }
                let (a, b) = num_pair(&args)?;
                Ok(Value::Num(a - b))
            }
            BuiltinKind::Mul => {
                if let (Value::Dec(a), Value::Dec(b)) = (&args[0], &args[1]) {
                    return Ok(Value::Dec(
                        crate::decimal::dec_mul(*a, *b).map_err(|e| EvalError::Runtime(e.to_string()))?,
                    ));
                }
                let (a, b) = num_pair(&args)?;
                Ok(Value::Num(a * b))
            }
            BuiltinKind::Div => {
                if let (Value::Dec(a), Value::Dec(b)) = (&args[0], &args[1]) {
                    return Ok(Value::Dec(
                        crate::decimal::dec_div(*a, *b).map_err(|e| EvalError::Runtime(e.to_string()))?,
                    ));
                }
                let (a, b) = num_pair(&args)?;
                Ok(Value::Num(a / b))
            }
            BuiltinKind::Lt => {
                if let (Value::Dec(a), Value::Dec(b)) = (&args[0], &args[1]) {
                    return Ok(Value::Bool(a < b));
                }
                let (a, b) = num_pair(&args)?;
                Ok(Value::Bool(a < b))
            }
            BuiltinKind::Gt => {
                if let (Value::Dec(a), Value::Dec(b)) = (&args[0], &args[1]) {
                    return Ok(Value::Bool(a > b));
                }
                let (a, b) = num_pair(&args)?;
                Ok(Value::Bool(a > b))
            }
            BuiltinKind::Le => {
                if let (Value::Dec(a), Value::Dec(b)) = (&args[0], &args[1]) {
                    return Ok(Value::Bool(a <= b));
                }
                let (a, b) = num_pair(&args)?;
                Ok(Value::Bool(a <= b))
            }
            BuiltinKind::Ge => {
                if let (Value::Dec(a), Value::Dec(b)) = (&args[0], &args[1]) {
                    return Ok(Value::Bool(a >= b));
                }
                let (a, b) = num_pair(&args)?;
                Ok(Value::Bool(a >= b))
            }
            BuiltinKind::ToDec => match &args[0] {
                Value::Num(n) => Ok(Value::Dec(
                    crate::decimal::dec_from_num(*n).map_err(|e| EvalError::Runtime(e.to_string()))?,
                )),
                _ => Err(EvalError::Runtime("to-dec は number が必要です".to_string())),
            },
            BuiltinKind::ToNum => match &args[0] {
                Value::Dec(s) => Ok(Value::Num(crate::decimal::dec_to_num(*s))),
                _ => Err(EvalError::Runtime("to-num は decimal が必要です".to_string())),
            },
            BuiltinKind::Eq => Ok(Value::Bool(
                structural_equal(&args[0], &args[1]).map_err(|e| EvalError::Runtime(e.to_string()))?,
            )),
            BuiltinKind::Mod => Ok(Value::Num(num(&args[0])? % num(&args[1])?)),
            BuiltinKind::Abs => Ok(Value::Num(num(&args[0])?.abs())),
            BuiltinKind::Min => Ok(Value::Num(num(&args[0])?.min(num(&args[1])?))),
            BuiltinKind::Max => Ok(Value::Num(num(&args[0])?.max(num(&args[1])?))),
            BuiltinKind::Not => Ok(Value::Bool(!bool_v(&args[0])?)),
            BuiltinKind::Str => Ok(Value::Str(Rc::from(print_value(&args[0]).as_str()))),
            BuiltinKind::StrConcat => {
                let mut s = String::new();
                for a in &args {
                    s.push_str(&str_v(a)?);
                }
                Ok(Value::Str(Rc::from(s.as_str())))
            }
            BuiltinKind::StrSlice => {
                let s: Vec<char> = str_v(&args[0])?.chars().collect();
                let (a, b) = slice_bounds(s.len(), num(&args[1])?, num(&args[2])?);
                Ok(Value::Str(Rc::from(s[a..b].iter().collect::<String>().as_str())))
            }
            BuiltinKind::Len => Ok(Value::Num(str_v(&args[0])?.chars().count() as f64)),
            BuiltinKind::Contains => Ok(Value::Bool(str_v(&args[0])?.contains(&*str_v(&args[1])?))),
            BuiltinKind::Split => {
                let s = str_v(&args[0])?;
                let t = str_v(&args[1])?;
                // §11.6: 区切りが空文字列のときは各 Unicode スカラ値文字を1要素とし、
                // 空文字列要素を生じない（Rust の str::split("") とは異なるので使わない）。
                let parts: Vec<Value> = if t.is_empty() {
                    s.chars().map(|c| Value::Str(Rc::from(c.to_string()))).collect()
                } else {
                    s.split(t.as_ref()).map(|p| Value::Str(Rc::from(p))).collect()
                };
                Ok(Value::Array(parts))
            }
            BuiltinKind::Array => Ok(Value::Array(args)),
            BuiltinKind::Nth => {
                let arr = arr_v(&args[0])?;
                let i = num(&args[1])?;
                // §11.6: (union T nil) は実行時に生値。成功は arr[trunc(i)]、失敗は nil。
                if i >= 0.0 && (i as usize) < arr.len() {
                    Ok(arr[i as usize].clone())
                } else {
                    Ok(Value::Nil)
                }
            }
            BuiltinKind::Count => Ok(Value::Num(arr_v(&args[0])?.len() as f64)),
            BuiltinKind::Slice => {
                let arr = arr_v(&args[0])?;
                let (a, b) = slice_bounds(arr.len(), num(&args[1])?, num(&args[2])?);
                Ok(Value::Array(arr[a..b].to_vec()))
            }
            BuiltinKind::Push => {
                let mut arr = arr_v(&args[0])?;
                arr.push(args[1].clone());
                Ok(Value::Array(arr))
            }
            BuiltinKind::Concat => {
                let mut out = arr_v(&args[0])?;
                out.extend(arr_v(&args[1])?);
                Ok(Value::Array(out))
            }
            BuiltinKind::Map => {
                let arr = arr_v(&args[0])?;
                let f = args[1].clone();
                let mut out = Vec::with_capacity(arr.len());
                for x in arr {
                    out.push(self.apply(f.clone(), vec![x])?);
                }
                Ok(Value::Array(out))
            }
            BuiltinKind::Filter => {
                let arr = arr_v(&args[0])?;
                let f = args[1].clone();
                let mut out = Vec::new();
                for x in arr {
                    if bool_v(&self.apply(f.clone(), vec![x.clone()])?)? {
                        out.push(x);
                    }
                }
                Ok(Value::Array(out))
            }
            BuiltinKind::FoldLeft => {
                let arr = arr_v(&args[0])?;
                let mut acc = args[1].clone();
                let f = args[2].clone();
                for x in arr {
                    acc = self.apply(f.clone(), vec![acc, x])?;
                }
                Ok(acc)
            }
            BuiltinKind::Find => {
                let arr = arr_v(&args[0])?;
                let f = args[1].clone();
                for x in arr {
                    if bool_v(&self.apply(f.clone(), vec![x.clone()])?)? {
                        // §11.6: 成功は生値 x、失敗は nil（Ok で包まない）。
                        return Ok(x);
                    }
                }
                Ok(Value::Nil)
            }
            BuiltinKind::Any => {
                let arr = arr_v(&args[0])?;
                let f = args[1].clone();
                for x in arr {
                    if bool_v(&self.apply(f.clone(), vec![x])?)? {
                        return Ok(Value::Bool(true));
                    }
                }
                Ok(Value::Bool(false))
            }
            BuiltinKind::All => {
                let arr = arr_v(&args[0])?;
                let f = args[1].clone();
                for x in arr {
                    if !bool_v(&self.apply(f.clone(), vec![x])?)? {
                        return Ok(Value::Bool(false));
                    }
                }
                Ok(Value::Bool(true))
            }
            BuiltinKind::Ok => Ok(ok_v(args[0].clone())),
            BuiltinKind::Err => Ok(err_v(args[0].clone())),
            BuiltinKind::Bind => {
                let r = &args[0];
                match r {
                    Value::Ok(v) => {
                        let inner = (**v).clone();
                        self.apply(args[1].clone(), vec![inner])
                    }
                    other => Ok(other.clone()),
                }
            }
            BuiltinKind::MapResult => {
                let r = &args[0];
                match r {
                    Value::Ok(v) => {
                        let inner = (**v).clone();
                        Ok(ok_v(self.apply(args[1].clone(), vec![inner])?))
                    }
                    other => Ok(other.clone()),
                }
            }
            BuiltinKind::Traverse => {
                let arr = arr_v(&args[0])?;
                let f = args[1].clone();
                let mut out = Vec::with_capacity(arr.len());
                for x in arr {
                    let r = self.apply(f.clone(), vec![x])?;
                    match r {
                        Value::Err(_) => return Ok(r),
                        Value::Ok(v) => out.push(*v),
                        other => out.push(other),
                    }
                }
                Ok(ok_v(Value::Array(out)))
            }
            BuiltinKind::MapEmpty => Ok(Value::Map(Rc::new(PersistentMap::new()))),
            BuiltinKind::SetEmpty => Ok(Value::Set(Rc::new(PersistentMap::new()))),
            BuiltinKind::MapGet => {
                let m = map_v(&args[0])?;
                let key = map_key(&args[1])?;
                match m.get(&key) {
                    Some(v) => Ok(ok_v(v.clone())),
                    None => Ok(err_v(not_found_value())),
                }
            }
            BuiltinKind::MapSet => {
                let m = map_v(&args[0])?;
                let key = map_key(&args[1])?;
                let (next, _) = m.insert(key, args[2].clone());
                Ok(Value::Map(Rc::new(next)))
            }
            BuiltinKind::MapDel => {
                let m = map_v(&args[0])?;
                let key = map_key(&args[1])?;
                let (next, _) = m.remove(&key);
                Ok(Value::Map(Rc::new(next)))
            }
            BuiltinKind::MapKeys => {
                let m = map_v(&args[0])?;
                Ok(Value::Array(m.keys().iter().map(|k| k.to_value()).collect()))
            }
            BuiltinKind::MapHas => {
                let m = map_v(&args[0])?;
                let key = map_key(&args[1])?;
                Ok(Value::Bool(m.has(&key)))
            }
            BuiltinKind::MapSize => Ok(Value::Num(map_v(&args[0])?.len() as f64)),
            BuiltinKind::SetAdd => {
                let s = set_v(&args[0])?;
                let key = map_key(&args[1])?;
                let (next, _) = s.insert(key, Value::Nil);
                Ok(Value::Set(Rc::new(next)))
            }
            BuiltinKind::SetHas => {
                let s = set_v(&args[0])?;
                let key = map_key(&args[1])?;
                Ok(Value::Bool(s.has(&key)))
            }
            BuiltinKind::SetToArray => {
                let s = set_v(&args[0])?;
                Ok(Value::Array(s.keys().iter().map(|k| k.to_value()).collect()))
            }
            BuiltinKind::ObjToMap => match &args[0] {
                Value::Obj(fields) => {
                    // BTreeMap のキー順（UTF-8 バイト順）= 決定的な挿入順
                    let mut m = PersistentMap::new();
                    for (k, v) in fields {
                        let (next, _) = m.insert(MapKey::Str(k.clone()), v.clone());
                        m = next;
                    }
                    Ok(Value::Map(Rc::new(m)))
                }
                _ => Err(EvalError::Runtime("obj->map は obj が必要です".to_string())),
            },
            BuiltinKind::CharAt => {
                let chars: Vec<char> = str_v(&args[0])?.chars().collect();
                let i = num(&args[1])?;
                // 小数添字は nth と同じく切り捨て（§11.6）。NaN・範囲外は out-of-range。
                let k = if i.is_nan() { usize::MAX } else { i.trunc() as usize };
                if i.is_nan() || i < 0.0 || k >= chars.len() {
                    Ok(err_v(err_tag_value("out-of-range")))
                } else {
                    Ok(ok_v(Value::Char(chars[k])))
                }
            }
            BuiltinKind::CharCode => Ok(Value::Num(char_v(&args[0])? as u32 as f64)),
            BuiltinKind::CodeToChar => {
                let n = num(&args[0])?;
                let valid = n.is_finite()
                    && n == n.trunc()
                    && n >= 0.0
                    && n <= 0x10FFFF as f64
                    && !(0xD800..=0xDFFF).contains(&(n as u32));
                match valid.then(|| char::from_u32(n as u32)).flatten() {
                    Some(c) => Ok(ok_v(Value::Char(c))),
                    None => Ok(err_v(err_tag_value("invalid-code"))),
                }
            }
            BuiltinKind::StrTakeWhile => {
                let chars: Vec<char> = str_v(&args[0])?.chars().collect();
                let start = clamp_start(chars.len(), num(&args[1])?);
                let f = args[2].clone();
                let mut acc = String::new();
                let mut pos = start;
                while pos < chars.len() && bool_v(&self.apply(f.clone(), vec![Value::Char(chars[pos])])?)? {
                    acc.push(chars[pos]);
                    pos += 1;
                }
                Ok(Value::Obj(BTreeMap::from([
                    (Rc::from("acc"), Value::Str(Rc::from(acc.as_str()))),
                    (Rc::from("pos"), Value::Num(pos as f64)),
                ])))
            }
            BuiltinKind::StrSkipWhile => {
                let chars: Vec<char> = str_v(&args[0])?.chars().collect();
                let start = clamp_start(chars.len(), num(&args[1])?);
                let f = args[2].clone();
                let mut pos = start;
                while pos < chars.len() && bool_v(&self.apply(f.clone(), vec![Value::Char(chars[pos])])?)? {
                    pos += 1;
                }
                Ok(Value::Num(pos as f64))
            }
            BuiltinKind::StrSplitAt => {
                let chars: Vec<char> = str_v(&args[0])?.chars().collect();
                let i = num(&args[1])?;
                let k = if i.is_nan() { None } else { Some(i.trunc() as usize) };
                match k {
                    Some(k) if !i.is_nan() && i >= 0.0 && k <= chars.len() => {
                        let left: String = chars[..k].iter().collect();
                        let right: String = chars[k..].iter().collect();
                        Ok(ok_v(Value::Obj(BTreeMap::from([
                            (Rc::from("left"), Value::Str(Rc::from(left.as_str()))),
                            (Rc::from("right"), Value::Str(Rc::from(right.as_str()))),
                        ]))))
                    }
                    _ => Ok(err_v(err_tag_value("out-of-range"))),
                }
            }
            BuiltinKind::StrContains => Ok(Value::Bool(str_v(&args[0])?.contains(&*str_v(&args[1])?))),
            BuiltinKind::CharLe => Ok(Value::Bool(char_v(&args[0])? <= char_v(&args[1])?)),
            BuiltinKind::CharEq => Ok(Value::Bool(char_v(&args[0])? == char_v(&args[1])?)),
            BuiltinKind::CharAlphabetic => Ok(Value::Bool(char_v(&args[0])?.is_alphabetic())),
            BuiltinKind::CharNumeric => Ok(Value::Bool(char_v(&args[0])?.is_numeric())),
            BuiltinKind::CharWhitespace => Ok(Value::Bool(char_v(&args[0])?.is_whitespace())),
            BuiltinKind::BuilderEmpty => Ok(Value::Builder(Rc::new(Vec::new()))),
            BuiltinKind::BuilderPush => {
                let b = match &args[0] {
                    Value::Builder(b) => Rc::clone(b),
                    _ => return Err(EvalError::Runtime("builder が必要です".to_string())),
                };
                let s = str_v(&args[1])?;
                // 共有されていなければ in-place（線形利用で償却 O(1)）。共有時のみ複製（永続性維持）。
                let mut nb = b;
                Rc::make_mut(&mut nb).push(s.to_string());
                Ok(Value::Builder(nb))
            }
            BuiltinKind::BuilderToStr => {
                let parts = match &args[0] {
                    Value::Builder(b) => b.clone(),
                    _ => return Err(EvalError::Runtime("builder が必要です".to_string())),
                };
                Ok(Value::Str(Rc::from(parts.join("").as_str())))
            }
            BuiltinKind::BuilderJoin => {
                let arr = arr_v(&args[0])?;
                let sep = str_v(&args[1])?;
                let mut out = String::new();
                for (i, x) in arr.iter().enumerate() {
                    if i > 0 {
                        out.push_str(&sep);
                    }
                    out.push_str(&str_v(x)?);
                }
                Ok(Value::Str(Rc::from(out.as_str())))
            }
            BuiltinKind::BytesGet => {
                let b = match &args[0] {
                    Value::Bytes(b) => b.clone(),
                    _ => return Err(EvalError::Runtime("bytes が必要です".to_string())),
                };
                let i = num(&args[1])?;
                // nth と同じく小数添字は切り捨て、範囲外は nil（Option）
                let k = if i.is_nan() { usize::MAX } else { i.trunc() as usize };
                if !i.is_nan() && i >= 0.0 && k < b.len() {
                    Ok(Value::Num(b[k] as f64))
                } else {
                    Ok(Value::Nil)
                }
            }
            BuiltinKind::BytesLen => match &args[0] {
                Value::Bytes(b) => Ok(Value::Num(b.len() as f64)),
                _ => Err(EvalError::Runtime("bytes が必要です".to_string())),
            },
            BuiltinKind::BytesSlice => {
                let b = match &args[0] {
                    Value::Bytes(b) => b.clone(),
                    _ => return Err(EvalError::Runtime("bytes が必要です".to_string())),
                };
                let (a, z) = slice_bounds(b.len(), num(&args[1])?, num(&args[2])?);
                Ok(Value::Bytes(Rc::new(b[a..z].to_vec())))
            }
            BuiltinKind::StrToBytes => {
                let s = str_v(&args[0])?;
                Ok(Value::Bytes(Rc::new(s.as_bytes().to_vec())))
            }
            BuiltinKind::BytesToStr => match &args[0] {
                Value::Bytes(b) => match std::str::from_utf8(b) {
                    Ok(s) => Ok(ok_v(Value::Str(Rc::from(s)))),
                    Err(_) => Ok(err_v(err_tag_value("invalid-utf8"))),
                },
                _ => Err(EvalError::Runtime("bytes が必要です".to_string())),
            },
            BuiltinKind::StrToCodepoints => {
                let s = str_v(&args[0])?;
                Ok(Value::Array(s.chars().map(|c| Value::Num(c as u32 as f64)).collect()))
            }
            BuiltinKind::CodepointsToStr => {
                let arr = arr_v(&args[0])?;
                let mut s = String::new();
                for x in &arr {
                    let n = num(x)?;
                    let valid = n.is_finite()
                        && n == n.trunc()
                        && n >= 0.0
                        && n <= 0x10FFFF as f64
                        && !(0xD800..=0xDFFF).contains(&(n as u32));
                    match valid.then(|| char::from_u32(n as u32)).flatten() {
                        Some(c) => s.push(c),
                        None => return Ok(err_v(err_tag_value("invalid-code"))),
                    }
                }
                Ok(ok_v(Value::Str(Rc::from(s.as_str()))))
            }
            BuiltinKind::HostMock {
                effect,
                default,
                log,
            } => {
                log.borrow_mut().push(crate::host::HostCall {
                    effect: effect.clone(),
                    args: args.clone(),
                });
                Ok(default.clone())
            }
        }
    }
}

fn slice_bounds(len: usize, start: f64, end: f64) -> (usize, usize) {
    let len_f = len as f64;
    let s = if start.is_nan() {
        0.0
    } else if start < 0.0 {
        (len_f + start).max(0.0)
    } else {
        start.min(len_f)
    };
    let e = if end.is_nan() {
        len_f
    } else if end < 0.0 {
        (len_f + end).max(0.0)
    } else {
        end.min(len_f)
    };
    (s as usize, (e.max(s)) as usize)
}

pub fn eval_core(interp: &mut Interp, e: &Core, env: &Rc<Env>) -> Result<Value, EvalError> {
    interp.depth += 1;
    if interp.depth > MAX_CALL_DEPTH {
        interp.depth -= 1;
        return Err(EvalError::Internal(format!(
            "再帰深さの上限 {MAX_CALL_DEPTH} を超えました（§8.5）"
        )));
    }
    let r = eval_loop(interp, e, env);
    interp.depth -= 1;
    r
}

// 末尾位置の式はループで評価する。proper tail call（末尾の App）はフレームを消費しない（SPEC §8.5 MUST）。
fn eval_loop(interp: &mut Interp, e: &Core, env: &Rc<Env>) -> Result<Value, EvalError> {
    let mut env = env.clone();
    let mut cur = e.clone();
    loop {
        let next = match cur {
            Core::Lit { value, .. } => return Ok(value),
            Core::Var { name, .. } => {
                return env
                    .lookup(&name)
                    .ok_or_else(|| EvalError::Runtime(format!("未束縛のシンボルです: '{name}'")))
            }
            Core::Lam {
                params, body, ..
            } => {
                return Ok(Value::Closure(Rc::new(Closure {
                    params,
                    body: Rc::new(*body),
                    env,
                    name: None,
                })))
            }
            Core::App { f, args, .. } => {
                let fv = eval_core(interp, &f, &env)?;
                let mut argv = Vec::with_capacity(args.len());
                for a in &args {
                    argv.push(eval_core(interp, a, &env)?);
                }
                match fv {
                    Value::Builtin(b) => return interp.apply_builtin(&b, argv),
                    Value::Closure(c) => {
                        if c.params.len() != argv.len() {
                            return Err(EvalError::Runtime(format!(
                                "引数の数が一致しません: '{}' は {} 引数",
                                c.name.as_deref().unwrap_or("anonymous"),
                                c.params.len()
                            )));
                        }
                        if let Some(name) = &c.name {
                            *interp.calls.entry(name.to_string()).or_insert(0) += 1;
                        }
                        let vars: BTreeMap<Rc<str>, Value> = c
                            .params
                            .iter()
                            .map(|p| p.name.clone())
                            .zip(argv)
                            .collect();
                        env = Env::child_of(&c.env, vars);
                        (*c.body).clone()
                    }
                    _ => {
                        return Err(EvalError::Runtime(
                            "関数ではない値を呼び出しました".to_string(),
                        ))
                    }
                }
            }
            Core::Let {
                name, rhs, body, ..
            } => {
                let v = eval_core(interp, &rhs, &env)?;
                env = Env::child_of(&env, BTreeMap::from([(name, v)]));
                *body
            }
            Core::Letrec {
                name,
                lam,
                body,
                span,
            } => {
                // design §2.1: 束縛対象は Lam のみ（check で検査済み）。自己参照環境を作る。
                let Core::Lam {
                    params,
                    body: lam_body,
                    ..
                } = lam.as_ref()
                else {
                    return Err(EvalError::Runtime(format!(
                        "letrec の束縛対象は fn のみです（span {:?}）",
                        span
                    )));
                };
                let child = Env::child_of(&env, BTreeMap::new());
                let clo = Value::Closure(Rc::new(Closure {
                    params: params.clone(),
                    body: Rc::new((**lam_body).clone()),
                    env: child.clone(),
                    name: Some(name.clone()),
                }));
                child.define(name.clone(), clo);
                env = child;
                return eval_core(interp, &body, &env);
            }
            Core::If {
                cond,
                then,
                else_,
                ..
            } => {
                let c = eval_core(interp, &cond, &env)?;
                match c {
                    Value::Bool(true) => *then,
                    Value::Bool(false) => *else_,
                    _ => {
                        return Err(EvalError::Runtime(
                            "if >条件式は bool でなければなりません".to_string(),
                        ))
                    }
                }
            }
            Core::Match {
                scrutinee, arms, ..
            } => {
                let scrut = eval_core(interp, &scrutinee, &env)?;
                let mut matched: Option<(Core, BTreeMap<Rc<str>, Value>)> = None;
                for arm in arms {
                    let mut binds = BTreeMap::new();
                    if match_value(&arm.pattern, &scrut, &mut binds) {
                        matched = Some((arm.body, binds));
                        break;
                    }
                }
                let (body, binds) = matched.ok_or_else(|| {
                    EvalError::Runtime("match がどのアームにも合致しませんでした".to_string())
                })?;
                env = Env::child_of(&env, binds);
                body
            }
            Core::Obj { fields, .. } => {
                let mut map: BTreeMap<Rc<str>, Value> = BTreeMap::new();
                for (name, expr) in fields {
                    map.insert(name, eval_core(interp, &expr, &env)?);
                }
                return Ok(Value::Obj(map));
            }
            Core::Field { obj, name, .. } => {
                let v = eval_core(interp, &obj, &env)?;
                return match &v {
                    Value::Obj(fields) => fields.get(&name).cloned().ok_or_else(|| {
                        EvalError::Runtime(format!("フィールド '{name}' が存在しません"))
                    }),
                    _ => Err(EvalError::Runtime(format!(
                        "フィールド '{name}' を obj 以外から取り出そうとしました"
                    ))),
                };
            }
            Core::Do { binds, body, .. } => {
                let mut vars: BTreeMap<Rc<str>, Value> = BTreeMap::new();
                for b in &binds {
                    let do_env = Env::child_of(&env, vars.clone());
                    let v = eval_core(interp, &b.expr, &do_env)?;
                    if matches!(v, Value::Err(_)) {
                        return Ok(v);
                    }
                    if b.name.as_ref() != "_" {
                        let bound = match v {
                            Value::Ok(inner) => *inner,
                            other => other,
                        };
                        vars.insert(b.name.clone(), bound);
                    }
                }
                env = Env::child_of(&env, vars);
                *body
            }
            Core::EffectCall {
                host,
                effect,
                args,
                ..
            } => {
                // design §1.5/§4: 組み込みエフェクトは host 不要で直接評価する
                if crate::effects::is_builtin_effect(effect.as_ref()) {
                    let mut argv = Vec::with_capacity(args.len());
                    for a in &args {
                        argv.push(eval_core(interp, a, &env)?);
                    }
                    return match effect.as_ref() {
                        // §4: now!/elapsed! はモックでは固定列で決定的、trace! は呼び出し記録へ蓄積
                        "now" => Ok(Value::Num(interp.mock_now())),
                        "elapsed" => {
                            let n = interp.mock_now();
                            let base = interp.clock_base.unwrap_or(n);
                            Ok(Value::Num(n - base))
                        }
                        "trace" => {
                            interp.host_log.borrow_mut().push(crate::host::HostCall {
                                effect: effect.clone(),
                                args: argv,
                            });
                            Ok(Value::Nil)
                        }
                        // §4.1: read-file! はモックの仮想ファイルから読む。無ければ io-error。
                        "read-file" => {
                            let path = match &argv[0] {
                                Value::Str(s) => s.to_string(),
                                _ => return Err(EvalError::Runtime("read-file! は string が必要です".to_string())),
                            };
                            match interp.virtual_files.get(&path) {
                                Some(data) => Ok(Value::Ok(Box::new(Value::Bytes(Rc::new(data.clone()))))),
                                None => Ok(Value::Err(Box::new(err_tag_value("io-error")))),
                            }
                        }
                        "ref" => Ok(Value::Ref(Rc::new(RefCell::new(argv[0].clone())))),
                        "ref-get" => match &argv[0] {
                            Value::Ref(r) => Ok(r.borrow().clone()),
                            _ => Err(EvalError::Runtime("ref-get! は ref が必要です".to_string())),
                        },
                        "ref-set" => match &argv[0] {
                            Value::Ref(r) => {
                                *r.borrow_mut() = argv[1].clone();
                                Ok(Value::Nil)
                            }
                            _ => Err(EvalError::Runtime("ref-set! は ref が必要です".to_string())),
                        },
                        _ => unreachable!(),
                    };
                }
                // P1 (§12.4/§12.6): EffectCall は host オブジェクトのハンドラを呼ぶ。
                // host は囲んでいる defn! の第1引数として束縛されている。
                let hv = env.lookup(host.as_ref()).ok_or_else(|| {
                    EvalError::Runtime(format!(
                        "host '{host}' が束縛されていません（defn! の第1引数として host を渡してください §12.6）"
                    ))
                })?;
                let handler = match &hv {
                    Value::Obj(fields) => fields.get(effect.as_ref()).cloned().ok_or_else(|| {
                        EvalError::Runtime(format!(
                            "host にエフェクト '{effect}' のハンドラがありません"
                        ))
                    })?,
                    _ => {
                        return Err(EvalError::Runtime(format!(
                            "host は obj である必要があります: '{host}'"
                        )))
                    }
                };
                let mut argv = Vec::with_capacity(args.len());
                for a in &args {
                    argv.push(eval_core(interp, a, &env)?);
                }
                return interp.apply(handler, argv);
            }
            Core::Error { message, .. } => return Err(EvalError::Runtime(message)),
        };
        cur = next;
    }
}

fn match_value(p: &Pat, v: &Value, binds: &mut BTreeMap<Rc<str>, Value>) -> bool {
    match &p.kind {
        PatKind::Wildcard => true,
        PatKind::Var(name) => {
            binds.insert(name.clone(), v.clone());
            true
        }
        PatKind::Lit(lit) => matches!(structural_equal(lit, v), Ok(true)),
        PatKind::Or(ps) => {
            for sub in ps {
                let mut sub_binds = BTreeMap::new();
                if match_value(sub, v, &mut sub_binds) {
                    binds.extend(sub_binds);
                    return true;
                }
            }
            false
        }
        PatKind::Obj(fields) => {
            let Value::Obj(obj) = v else {
                return false;
            };
            for (name, sub) in fields {
                let Some(fv) = obj.get(name) else {
                    return false;
                };
                if !match_value(sub, fv, binds) {
                    return false;
                }
            }
            true
        }
        PatKind::Array(elements) => {
            let Value::Array(arr) = v else {
                return false;
            };
            if arr.len() != elements.len() {
                return false;
            }
            elements
                .iter()
                .zip(arr.iter())
                .all(|(el, av)| match_value(el, av, binds))
        }
    }
}

pub fn stdlib_env() -> Rc<Env> {
    let env = Env::new();
    let def = |name: &str, kind: BuiltinKind| {
        env.define(
            Rc::from(name),
            Value::Builtin(Rc::new(Builtin {
                name: Rc::from(name),
                kind,
            })),
        );
    };
    def("+", BuiltinKind::Add);
    def("-", BuiltinKind::Sub);
    def("*", BuiltinKind::Mul);
    def("/", BuiltinKind::Div);
    def("<", BuiltinKind::Lt);
    def(">", BuiltinKind::Gt);
    def("<=", BuiltinKind::Le);
    def(">=", BuiltinKind::Ge);
    def("=", BuiltinKind::Eq);
    def("mod", BuiltinKind::Mod);
    def("abs", BuiltinKind::Abs);
    def("min", BuiltinKind::Min);
    def("max", BuiltinKind::Max);
    def("not", BuiltinKind::Not);
    def("str", BuiltinKind::Str);
    def("str-concat", BuiltinKind::StrConcat);
    def("str-slice", BuiltinKind::StrSlice);
    def("len", BuiltinKind::Len);
    def("contains?", BuiltinKind::Contains);
    def("split", BuiltinKind::Split);
    def("array", BuiltinKind::Array);
    def("nth", BuiltinKind::Nth);
    def("count", BuiltinKind::Count);
    def("slice", BuiltinKind::Slice);
    def("push", BuiltinKind::Push);
    def("concat", BuiltinKind::Concat);
    def("map", BuiltinKind::Map);
    def("filter", BuiltinKind::Filter);
    def("fold-left", BuiltinKind::FoldLeft);
    def("find", BuiltinKind::Find);
    def("any?", BuiltinKind::Any);
    def("all?", BuiltinKind::All);
    def("ok", BuiltinKind::Ok);
    def("err", BuiltinKind::Err);
    def("bind", BuiltinKind::Bind);
    def("map-result", BuiltinKind::MapResult);
    def("traverse", BuiltinKind::Traverse);
    def("to-dec", BuiltinKind::ToDec);
    def("to-num", BuiltinKind::ToNum);
    def("map-empty", BuiltinKind::MapEmpty);
    def("map-get", BuiltinKind::MapGet);
    def("map-set", BuiltinKind::MapSet);
    def("map-del", BuiltinKind::MapDel);
    def("map-keys", BuiltinKind::MapKeys);
    def("map-has?", BuiltinKind::MapHas);
    def("map-size", BuiltinKind::MapSize);
    def("set-empty", BuiltinKind::SetEmpty);
    def("set-add", BuiltinKind::SetAdd);
    def("set-has?", BuiltinKind::SetHas);
    def("set->array", BuiltinKind::SetToArray);
    def("obj->map", BuiltinKind::ObjToMap);
    def("char-at", BuiltinKind::CharAt);
    def("char-code", BuiltinKind::CharCode);
    def("code->char", BuiltinKind::CodeToChar);
    def("str-take-while", BuiltinKind::StrTakeWhile);
    def("str-skip-while", BuiltinKind::StrSkipWhile);
    def("str-split-at", BuiltinKind::StrSplitAt);
    def("str-contains?", BuiltinKind::StrContains);
    def("char<=?", BuiltinKind::CharLe);
    def("char=?", BuiltinKind::CharEq);
    def("char-alphabetic?", BuiltinKind::CharAlphabetic);
    def("char-numeric?", BuiltinKind::CharNumeric);
    def("char-whitespace?", BuiltinKind::CharWhitespace);
    def("builder-empty", BuiltinKind::BuilderEmpty);
    def("builder-push", BuiltinKind::BuilderPush);
    def("builder->str", BuiltinKind::BuilderToStr);
    def("builder-join", BuiltinKind::BuilderJoin);
    def("bytes-get", BuiltinKind::BytesGet);
    def("bytes-len", BuiltinKind::BytesLen);
    def("bytes-slice", BuiltinKind::BytesSlice);
    def("str->bytes", BuiltinKind::StrToBytes);
    def("bytes->str", BuiltinKind::BytesToStr);
    def("str->codepoints", BuiltinKind::StrToCodepoints);
    def("codepoints->str", BuiltinKind::CodepointsToStr);
    env
}

pub fn global_env() -> Rc<Env> {
    stdlib_env()
}

pub fn load_defs(defs: &[CoreDef], env: &Rc<Env>) {
    for d in defs {
        if let CoreDef::Defn {
            name,
            params,
            body,
            ..
        } = d
        {
            env.define(
                name.clone(),
                Value::Closure(Rc::new(Closure {
                    params: params.clone(),
                    body: Rc::new(body.clone()),
                    env: env.clone(),
                    name: Some(name.clone()),
                })),
            );
        }
    }
}

pub struct ExampleResult {
    pub index: usize,
    pub pass: bool,
    pub actual: Value,
    pub expected: Value,
}

pub fn run_examples(
    interp: &mut Interp,
    defs: &[CoreDef],
    env: &Rc<Env>,
) -> Result<Vec<ExampleResult>, EvalError> {
    let mut results = Vec::new();
    let mut index = 0;
    for d in defs {
        if let CoreDef::Example {
            actual, expected, ..
        } = d
        {
            let a = eval_core(interp, actual, env)?;
            let e = eval_core(interp, expected, env)?;
            let pass =
                structural_equal(&a, &e).map_err(|e| EvalError::Runtime(e.to_string()))?;
            results.push(ExampleResult {
                index,
                pass,
                actual: a,
                expected: e,
            });
            index += 1;
        }
    }
    Ok(results)
}

pub fn load_file(
    path: &str,
    env: &Rc<Env>,
    seen: &mut BTreeSet<String>,
) -> Result<Vec<CoreDef>, EvalError> {
    if !seen.insert(path.to_string()) {
        return Ok(Vec::new());
    }
    let src = std::fs::read_to_string(path)
        .map_err(|e| EvalError::Runtime(format!("ファイルを読めません: '{path}': {e}")))?;
    let lex = tokenize(&src, path);
    let (program, _) = parse_program(&lex);
    let (defs, _) = desugar(&program);
    for d in &defs {
        if let CoreDef::Import { path: import_path, .. } = d {
            if let Some(target) = resolve_import_path(path, import_path) {
                load_file(&target, env, seen)?;
            }
        }
    }
    load_defs(&defs, env);
    Ok(defs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{check_file, ModuleCache};

    fn fixture_path(name: &str) -> String {
        format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
    }

    fn defs_of(src: &str) -> Vec<CoreDef> {
        let lex = tokenize(src, "test.lts");
        let (program, _) = parse_program(&lex);
        desugar(&program).0
    }

    fn call_src(src: &str, fn_name: &str, args: Vec<Value>) -> Result<Value, EvalError> {
        let env = global_env();
        load_defs(&defs_of(src), &env);
        let f = env.lookup(fn_name).expect("関数が未定義です");
        let mut interp = Interp::new();
        interp.apply(f, args)
    }

    fn load_fixture(name: &str) -> (Rc<Env>, Vec<CoreDef>) {
        let env = global_env();
        let mut seen = BTreeSet::new();
        let defs = load_file(&fixture_path(name), &env, &mut seen).expect("load");
        (env, defs)
    }

    fn obj(pairs: Vec<(&str, Value)>) -> Value {
        Value::Obj(pairs.into_iter().map(|(k, v)| (Rc::from(k), v)).collect())
    }

    fn money(amount: f64, currency: &str) -> Value {
        obj(vec![
            ("amount", Value::Num(amount)),
            ("currency", Value::Str(Rc::from(currency))),
        ])
    }

    #[test]
    fn money_lts_example_passes() {
        let (env, defs) = load_fixture("money.lts");
        let mut interp = Interp::new();
        let results = run_examples(&mut interp, &defs, &env).expect("examples");
        assert_eq!(results.len(), 1);
        assert!(results[0].pass, "{:?} vs {:?}", results[0].actual, results[0].expected);
        assert!(structural_equal(
            &results[0].actual,
            &Value::Ok(Box::new(money(3.0, "JPY")))
        )
        .expect("eq"));
    }

    #[test]
    fn cart_lts_example_place_order_empty_cart() {
        let (env, defs) = load_fixture("cart.lts");
        let mut interp = Interp::new();
        let results = run_examples(&mut interp, &defs, &env).expect("examples");
        assert_eq!(results.len(), 1);
        assert!(results[0].pass);
        assert!(structural_equal(
            &results[0].actual,
            &Value::Err(Box::new(obj(vec![("tag", Value::Str(Rc::from("EmptyCart")))])))
        )
        .expect("eq"));
    }

    #[test]
    fn add_money_ok_and_err() {
        let (env, _) = load_fixture("money.lts");
        let add_money = env.lookup("add-money").expect("add-money");
        let mut interp = Interp::new();
        let r = interp
            .apply(add_money.clone(), vec![money(1.0, "JPY"), money(2.0, "JPY")])
            .expect("ok");
        assert_eq!(r, Value::Ok(Box::new(money(3.0, "JPY"))));
        let r = interp
            .apply(add_money, vec![money(1.0, "JPY"), money(2.0, "USD")])
            .expect("err");
        assert_eq!(
            r,
            Value::Err(Box::new(Value::Str(Rc::from("currency mismatch: JPY vs USD"))))
        );
    }

    #[test]
    fn t3_do_early_return_total_not_called() {
        let (env, _) = load_fixture("cart.lts");
        let place_order = env.lookup("place-order").expect("place-order");
        let mut interp = Interp::new();
        let r = interp
            .apply(place_order, vec![Value::Array(vec![])])
            .expect("eval");
        assert!(matches!(r, Value::Err(_)));
        let calls = interp.calls();
        assert!(!calls.contains_key("total"), "total は呼ばれてはいけない: {calls:?}");
        assert!(calls.contains_key("check-items"));
        assert!(calls.contains_key("place-order"));
    }

    #[test]
    fn match_evaluates_scrutinee_once_and_binds() {
        let f = |args: Vec<Value>| {
            call_src(
                "(defn f (x : number) : string (match x [0 \"zero\"] [n (str-concat \"n=\" (str n))]))",
                "f",
                args,
            )
        };
        assert_eq!(f(vec![Value::Num(0.0)]).expect("ok"), Value::Str(Rc::from("zero")));
        assert_eq!(f(vec![Value::Num(3.0)]).expect("ok"), Value::Str(Rc::from("n=3")));
        let g = |v: Value| {
            call_src(
                "(defn g (e : (obj (tag : string))) : string (match e [(obj (tag \"EmptyCart\")) \"empty\"] [_ \"other\"]))",
                "g",
                vec![v],
            )
        };
        assert_eq!(
            g(obj(vec![("tag", Value::Str(Rc::from("EmptyCart")))]))
                .expect("ok"),
            Value::Str(Rc::from("empty"))
        );
        assert_eq!(
            g(obj(vec![("tag", Value::Str(Rc::from("X")))]))
                .expect("ok"),
            Value::Str(Rc::from("other"))
        );
    }

    #[test]
    fn and_or_short_circuit() {
        assert_eq!(
            call_src("(defn f () : bool (and false boom))", "f", vec![]).expect("ok"),
            Value::Bool(false)
        );
        assert_eq!(
            call_src("(defn f () : bool (or true boom))", "f", vec![]).expect("ok"),
            Value::Bool(true)
        );
        assert_eq!(
            call_src("(defn f () : bool (and true false))", "f", vec![]).expect("ok"),
            Value::Bool(false)
        );
        assert_eq!(
            call_src("(defn f () : bool (or false true))", "f", vec![]).expect("ok"),
            Value::Bool(true)
        );
    }

    #[test]
    fn let_if_field_pipe_obj() {
        assert_eq!(
            call_src("(defn f () : number (let [a 1 b 2] (+ a b)))", "f", vec![]).expect("ok"),
            Value::Num(3.0)
        );
        assert_eq!(
            call_src("(defn f () : string (if true \"yes\" \"no\"))", "f", vec![]).expect("ok"),
            Value::Str(Rc::from("yes"))
        );
        assert_eq!(
            call_src(
                "(defn f (o : (obj (a : number) (b : number))) : number o.a)",
                "f",
                vec![obj(vec![("a", Value::Num(10.0)), ("b", Value::Num(20.0))])]
            )
            .expect("ok"),
            Value::Num(10.0)
        );
        assert_eq!(
            call_src(
                "(defn g () : number (-> (obj (a 1)) (fn (o : (obj (a : number))) : number o.a)))",
                "g",
                vec![]
            )
            .expect("ok"),
            Value::Num(1.0)
        );
    }

    #[test]
    fn division_by_zero_is_infinity_or_nan() {
        let inf = call_src("(defn f () : number (/ 1 0))", "f", vec![]).expect("ok");
        assert_eq!(inf, Value::Num(f64::INFINITY));
        let nan = call_src("(defn f () : number (/ 0 0))", "f", vec![]).expect("ok");
        assert!(matches!(nan, Value::Num(n) if n.is_nan()));
    }

    #[test]
    fn fact_lts_recursion() {
        let mut cache = ModuleCache::default();
        let r = check_file(&fixture_path("fact.lts"), &mut cache);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let (env, _) = load_fixture("fact.lts");
        let fact = env.lookup("fact").expect("fact");
        let mut interp = Interp::new();
        assert_eq!(
            interp.apply(fact.clone(), vec![Value::Num(5.0)]).expect("ok"),
            Value::Num(120.0)
        );
        assert_eq!(
            interp.apply(fact, vec![Value::Num(0.0)]).expect("ok"),
            Value::Num(1.0)
        );
    }

    #[test]
    fn non_tail_recursion_depth_limit_is_internal_error() {
        let handle = std::thread::Builder::new()
            .stack_size(256 * 1024 * 1024)
            .spawn(|| {
                let (env, _) = load_fixture("fact.lts");
                let fact = env.lookup("fact").expect("fact");
                let mut interp = Interp::new();
                let r = interp.apply(fact, vec![Value::Num(20000.0)]);
                matches!(r, Err(EvalError::Internal(_)))
            })
            .expect("spawn");
        assert!(
            handle.join().expect("join"),
            "非末尾再帰の深さ上限超過は internal-error であるべき"
        );
    }

    #[test]
    fn tail_self_recursion_runs_in_constant_stack() {
        let r = call_src(
            "(defn loop (n : number) (acc : number) : number (if (= n 0) acc (loop (- n 1) (+ acc 1))))",
            "loop",
            vec![Value::Num(1_000_000.0), Value::Num(0.0)],
        )
        .expect("ok");
        assert_eq!(r, Value::Num(1_000_000.0));
    }

    #[test]
    fn tail_mutual_recursion_runs_in_constant_stack() {
        let src = "(defn ev (n : number) : bool (if (= n 0) true (od (- n 1)))) (defn od (n : number) : bool (if (= n 0) false (ev (- n 1))))";
        let r = call_src(src, "ev", vec![Value::Num(1_000_000.0)]).expect("ok");
        assert_eq!(r, Value::Bool(true));
        let r = call_src(src, "od", vec![Value::Num(1_000_001.0)]).expect("ok");
        assert_eq!(r, Value::Bool(true));
    }

    fn builtin_call(name: &str, args: Vec<Value>) -> Result<Value, EvalError> {
        let env = global_env();
        let f = env.lookup(name).expect("builtin");
        let mut interp = Interp::new();
        interp.apply(f, args)
    }

    #[test]
    fn nth_returns_raw_value_not_ok_wrapped() {
        let arr = Value::Array(vec![Value::Num(10.0), Value::Num(20.0), Value::Num(30.0)]);
        let hit = builtin_call("nth", vec![arr.clone(), Value::Num(1.0)]).expect("ok");
        assert_eq!(hit, Value::Num(20.0), "生値であること（§11.6: Ok で包まない）");
        let miss = builtin_call("nth", vec![arr, Value::Num(9.0)]).expect("ok");
        assert_eq!(miss, Value::Nil);
    }

    #[test]
    fn find_returns_raw_value_or_nil() {
        let src = "(defn f (arr : (Array number)) : (Option number) (find arr (fn (x : number) : bool (> x 1))))";
        let hit = call_src(src, "f", vec![Value::Array(vec![Value::Num(1.0), Value::Num(2.0), Value::Num(3.0)])]).expect("ok");
        assert_eq!(hit, Value::Num(2.0), "生値であること（§11.6）");
        let miss = call_src(src, "f", vec![Value::Array(vec![Value::Num(1.0)])]).expect("ok");
        assert_eq!(miss, Value::Nil);
    }

    #[test]
    fn split_empty_separator_splits_into_scalar_values_without_empty_parts() {
        let r = builtin_call("split", vec![Value::Str(Rc::from("a\u{1f600}b")), Value::Str(Rc::from(""))]).expect("ok");
        assert_eq!(
            r,
            Value::Array(vec![
                Value::Str(Rc::from("a")),
                Value::Str(Rc::from("\u{1f600}")),
                Value::Str(Rc::from("b"))
            ]),
            "絵文字は1要素・空要素なし（§11.6）"
        );
        let r = builtin_call("split", vec![Value::Str(Rc::from("abc")), Value::Str(Rc::from(""))]).expect("ok");
        assert_eq!(r, Value::Array(vec![Value::Str(Rc::from("a")), Value::Str(Rc::from("b")), Value::Str(Rc::from("c"))]));
        let r = builtin_call("split", vec![Value::Str(Rc::from("")), Value::Str(Rc::from(""))]).expect("ok");
        assert_eq!(r, Value::Array(vec![]));
        let r = builtin_call("split", vec![Value::Str(Rc::from("a,b,c")), Value::Str(Rc::from(","))]).expect("ok");
        assert_eq!(r, Value::Array(vec![Value::Str(Rc::from("a")), Value::Str(Rc::from("b")), Value::Str(Rc::from("c"))]));
    }

    #[test]
    fn decimal_arithmetic_rounding_and_errors() {
        let r = call_src("(defn f () : decimal (+ 1.50d 2.25d))", "f", vec![]).expect("ok");
        assert_eq!(r, Value::Dec(3_750_000_000_000_000_000));
        let r = call_src("(defn f () : decimal (/ 2.00d 3.00d))", "f", vec![]).expect("ok");
        assert_eq!(r, Value::Dec(666_666_666_666_666_667), "half-away で切り上がる");
        let r = call_src("(defn f () : bool (= 1.50d 1.50d))", "f", vec![]).expect("ok");
        assert_eq!(r, Value::Bool(true));
        let r = call_src("(defn f () : number (to-num 2.50d))", "f", vec![]).expect("ok");
        assert_eq!(r, Value::Num(2.5));
        let r = call_src("(defn f () : decimal (to-dec 0.1))", "f", vec![]).expect("ok");
        assert_eq!(r, Value::Dec(100_000_000_000_000_000));
        // 0 除算・オーバーフローはバックエンド中立の実行時エラー
        let e = call_src("(defn f () : decimal (/ 1.00d 0.00d))", "f", vec![]).expect_err("err");
        assert!(e.to_string().contains("decimal-division-by-zero"), "{e}");
        let e = call_src(
            "(defn f () : decimal (* 100000000000d 100000000000d))",
            "f",
            vec![],
        )
        .expect_err("err");
        assert!(e.to_string().contains("decimal-overflow"), "{e}");
        // 混在は実行時エラー（検査で弾かれるはずだが eval でも事故らない）
        let e = call_src("(defn f () : number (+ 1 1.5d))", "f", vec![]);
        assert!(e.is_err());
    }
}
