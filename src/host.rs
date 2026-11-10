//! host DI（M21 の基盤）: host は「能力インターフェース」であり、effect 宣言
//! `(effect name : sig)` の name → sig の対応が、実装（interp では λ の関数値、
//! 将来の WASM ではホスト関数、JS では host オブジェクトのメソッド）へ 1対1 に写像する。
//! 処理系本体は特定の host 実装に依存せず、実装は外部から注入する（§12.6 と同じ方針）。
//! 言語機能（エフェクト実行そのもの）は M21。ここでは注入機構とモック host のみ。

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use serde::{Deserialize, Serialize};

use crate::types::{normalize_type, AliasEnv, Type};
use crate::value::{Env, Value};

/// 注入する host の種別（CLI `--host` に対応）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostMode {
    None,
    Mock,
}

impl HostMode {
    pub fn parse(s: &str) -> Option<HostMode> {
        match s {
            "none" => Some(HostMode::None),
            "mock" => Some(HostMode::Mock),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            HostMode::None => "none",
            HostMode::Mock => "mock",
        }
    }
}

/// モック host が記録したエフェクト呼び出し（決定性のテスト・差分テストの基礎）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostCall {
    pub effect: Rc<str>,
    pub args: Vec<Value>,
}

/// effect 宣言の集まりから host の obj 型を導出する。
/// `(effect log : (fn (s : string) : nil))` → `(obj (log : (fn (s : string) : nil)))`。
/// これが defn! の第1引数（host）の型にもなる（§12.6）。
pub fn host_type(effects: &BTreeMap<Rc<str>, Type>) -> Type {
    Type::Obj(
        effects
            .iter()
            .map(|(n, t)| (n.clone(), t.clone()))
            .collect(),
    )
}

/// モック host の決定的な固定応答: 宣言された戻り型の「形」だけから決まる値。
/// - nil → Nil、number → 0、string → ""、bool → false、Array → []、obj → 各フィールドの既定値
/// - union → nil を含むなら Nil、 otherwise 先頭要素の既定値
/// - Result T E → Err(E の既定値)（fail-safe）
/// - fn 型・型変数 → 構築できないので None（その effect はモックに含めない）
pub fn default_value(ty: &Type, aliases: &AliasEnv) -> Option<Value> {
    match normalize_type(ty, aliases) {
        Type::Nil => Some(Value::Nil),
        Type::Number => Some(Value::Num(0.0)),
        Type::Dec => Some(Value::Dec(0)),
        Type::Str => Some(Value::Str(Rc::from(""))),
        Type::Bool => Some(Value::Bool(false)),
        Type::Char => Some(Value::Char('a')),
        Type::Literal(_) => None,
        Type::Array(_) => Some(Value::Array(Vec::new())),
        Type::Map(..) => Some(Value::Map(Rc::new(crate::pmap::PersistentMap::new()))),
        Type::Set(_) => Some(Value::Set(Rc::new(crate::pmap::PersistentMap::new()))),
        Type::Builder => Some(Value::Builder(Rc::new(Vec::new()))),
        Type::Bytes => Some(Value::Bytes(Rc::new(Vec::new()))),
        // ref は構築不可（エフェクト経由のみ）
        Type::Ref(_) => None,
        Type::Union(members) => {
            if members.iter().any(|m| matches!(normalize_type(m, aliases), Type::Nil)) {
                Some(Value::Nil)
            } else {
                members.first().and_then(|m| default_value(m, aliases))
            }
        }
        Type::Obj(fields) => {
            let mut out = BTreeMap::new();
            for (k, v) in &fields {
                out.insert(k.clone(), default_value(v, aliases)?);
            }
            Some(Value::Obj(out))
        }
        Type::ResultT(_, e) => Some(Value::Err(Box::new(default_value(&e, aliases)?))),
        Type::FnT(..) | Type::Var(_) | Type::Alias(_) => None,
    }
}

/// design/interp-capability.md §4: M20 host 能力インターフェースに追加される組み込みエフェクト。
/// 言語側で宣言済みとして扱い（effect 宣言不要）、host ハンドラ検査もスキップする。
/// - now! / elapsed! : モックでは固定列（MOCK_NOW_BASE + 呼び出し回数*1000）で決定的（eval 側クロック）
/// - trace! : 呼び出し記録（host-log）に蓄積。host-log op は trace! の糖衣として残る
/// - read-file! : bytes 型が未実装（M29）のため現時点では未対応。M29 で bytes 実装時に追加し、
///   JS 側は Node `fs` 依存となる（docs/backends.md に明記）
pub const BUILTIN_HOST_EFFECTS: &[&str] = &["now", "elapsed", "trace"];

/// モック host: 呼び出しを記録しつつ、決定的な固定応答を返す。
/// 同じ effect 宣言・同じ呼び出し列なら、応答も記録も常に同じ（決定性 MUST）。
pub struct MockHost {
    calls: Rc<RefCell<Vec<HostCall>>>,
    obj: Value,
}

impl MockHost {
    /// effect 宣言からモック host の obj 値を構築する。
    /// 戻り型から既定値を構築できない effect（fn 戻りなど）は含めない。
    pub fn from_effects(effects: &BTreeMap<Rc<str>, Type>, aliases: &AliasEnv) -> MockHost {
        let calls: Rc<RefCell<Vec<HostCall>>> = Rc::new(RefCell::new(Vec::new()));
        let mut fields = BTreeMap::new();
        for (name, sig) in effects {
            let ret = match normalize_type(sig, aliases) {
                Type::FnT(_, ret) => (*ret).clone(),
                _ => continue,
            };
            let Some(default) = default_value(&ret, aliases) else {
                continue;
            };
            fields.insert(
                name.clone(),
                Value::Builtin(Rc::new(crate::value::Builtin {
                    name: Rc::from(format!("{name}!").as_str()),
                    kind: crate::value::BuiltinKind::HostMock {
                        effect: name.clone(),
                        default,
                        log: calls.clone(),
                    },
                })),
            );
        }
        MockHost {
            calls,
            obj: Value::Obj(fields),
        }
    }

    pub fn obj(&self) -> Value {
        self.obj.clone()
    }

    pub fn calls(&self) -> Vec<HostCall> {
        self.calls.borrow().clone()
    }

    /// 呼び出し記録の共有ハンドル（repl の Interp と log を共有し、trace! も host-log に載せる）
    pub fn log_handle(&self) -> Rc<RefCell<Vec<HostCall>>> {
        self.calls.clone()
    }
}

/// host 値を環境へ注入する（注入点: interp の全局環境。REPL では `host` という名前で束縛）。
pub fn inject_host(env: &Rc<Env>, name: &str, host: &Value) {
    env.define(Rc::from(name), host.clone());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{check_source, ModuleCache};
    use crate::eval::{global_env, Interp};
    use crate::value::print_value;

    fn effects_of(src: &str) -> (BTreeMap<Rc<str>, Type>, AliasEnv) {
        let mut cache = ModuleCache::default();
        let res = check_source(src, "test.lts", &mut cache);
        assert!(res.diagnostics.is_empty(), "{:?}", res.diagnostics);
        (res.effects.clone(), res.type_aliases.clone())
    }

    #[test]
    fn host_type_is_obj_of_effect_sigs() {
        let (effects, _) = effects_of(
            "(effect log : (fn (s : string) : nil))\n(effect db : (fn (op : string) (key : string) : (Option string)))",
        );
        match host_type(&effects) {
            Type::Obj(fields) => {
                assert!(fields.contains_key("log"));
                assert!(fields.contains_key("db"));
            }
            other => panic!("obj expected: {other:?}"),
        }
    }

    #[test]
    fn default_value_is_deterministic_by_type_shape() {
        let aliases = AliasEnv::new();
        assert_eq!(default_value(&Type::Nil, &aliases), Some(Value::Nil));
        assert_eq!(default_value(&Type::Number, &aliases), Some(Value::Num(0.0)));
        assert_eq!(default_value(&Type::Str, &aliases), Some(Value::Str(Rc::from(""))));
        assert_eq!(default_value(&Type::Bool, &aliases), Some(Value::Bool(false)));
        assert_eq!(
            default_value(&Type::Array(Box::new(Type::Number)), &aliases),
            Some(Value::Array(vec![]))
        );
        // (Option string) = (union string nil) → nil優先
        assert_eq!(
            default_value(
                &Type::Union(vec![Type::Str, Type::Nil]),
                &aliases
            ),
            Some(Value::Nil)
        );
        // Result number string → Err("")
        assert_eq!(
            default_value(
                &Type::ResultT(Box::new(Type::Number), Box::new(Type::Str)),
                &aliases
            ),
            Some(Value::Err(Box::new(Value::Str(Rc::from("")))))
        );
        // fn 戻りは構築不可
        assert_eq!(
            default_value(&Type::FnT(vec![], Box::new(Type::Nil)), &aliases),
            None
        );
    }

    #[test]
    fn mock_host_records_calls_and_responds_deterministically() {
        let (effects, aliases) = effects_of(
            "(effect log : (fn (s : string) : nil))\n(effect db : (fn (op : string) (key : string) : (Option string)))",
        );
        let mock = MockHost::from_effects(&effects, &aliases);
        let env = global_env();
        inject_host(&env, "host", &mock.obj());
        let host = env.lookup("host").expect("host injected");
        let Value::Obj(fields) = &host else {
            panic!("host is obj")
        };
        let log = fields.get("log").cloned().expect("log handler");
        let db = fields.get("db").cloned().expect("db handler");
        let mut interp = Interp::new();
        let r1 = interp
            .apply(log.clone(), vec![Value::Str(Rc::from("hi"))])
            .expect("log call");
        assert_eq!(r1, Value::Nil);
        let r2 = interp
            .apply(db, vec![Value::Str(Rc::from("get")), Value::Str(Rc::from("k"))])
            .expect("db call");
        assert_eq!(r2, Value::Nil, "(Option string) の既定応答は nil");
        let calls = mock.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].effect.as_ref(), "log");
        assert_eq!(print_value(&calls[0].args[0]), "hi"); // 文字列は引用符なし（§11.6）
        assert_eq!(calls[1].effect.as_ref(), "db");
        // 決定性: 同じ呼び出しをもう一度 → 同じ応答・記録は呼び出し順
        let r3 = interp.apply(log, vec![Value::Str(Rc::from("hi"))]).expect("log call");
        assert_eq!(r3, r1);
        assert_eq!(mock.calls().len(), 3);
        assert_eq!(mock.calls()[2], calls[0]);
    }

    #[test]
    fn mock_host_skips_effects_with_unconstructible_return() {
        let (effects, aliases) = effects_of("(effect cb : (fn (f : (fn (x : number) : nil)) : (fn (x : number) : nil)))");
        let mock = MockHost::from_effects(&effects, &aliases);
        let Value::Obj(fields) = mock.obj() else {
            panic!("obj")
        };
        assert!(fields.is_empty(), "fn 戻りの effect はモックに含めない");
    }
}
