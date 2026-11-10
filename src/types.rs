use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Type {
    Number,
    Dec,
    Str,
    Bool,
    Char,                           // Unicode スカラー値1つ（design/interp-capability.md §1.2）
    Nil,
    Literal(Rc<str>),
    Array(Box<Type>),
    Union(Vec<Type>),
    Obj(BTreeMap<Rc<str>, Type>),
    Map(Box<Type>, Box<Type>),      // (map K V) — design/interp-capability.md §1.1
    Set(Box<Type>),                 // (set K)
    Ref(Box<Type>),                 // (ref T) — §1.5（エフェクト型のセル）
    Builder,                        // builder プリミティブ — §3.2
    Bytes,                          // bytes プリミティブ（不変バイト列）— §1.4
    ResultT(Box<Type>, Box<Type>),
    FnT(Vec<Type>, Box<Type>),
    Var(usize),
    Alias(Rc<str>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scheme {
    pub vars: Vec<usize>,
    pub body: Type,
}

pub type AliasEnv = BTreeMap<Rc<str>, Type>;

pub fn structural_equal(a: &Type, b: &Type) -> bool {
    if std::mem::discriminant(a) != std::mem::discriminant(b) {
        return false;
    }
    match (a, b) {
        (Type::Number, Type::Number)
        | (Type::Dec, Type::Dec)
        | (Type::Str, Type::Str)
        | (Type::Bool, Type::Bool)
        | (Type::Char, Type::Char)
        | (Type::Nil, Type::Nil) => true,
        (Type::Literal(x), Type::Literal(y)) => x == y,
        (Type::Var(x), Type::Var(y)) => x == y,
        (Type::Alias(x), Type::Alias(y)) => x == y,
        (Type::Array(x), Type::Array(y)) => structural_equal(x, y),
        (Type::Map(xk, xv), Type::Map(yk, yv)) => {
            structural_equal(xk, yk) && structural_equal(xv, yv)
        }
        (Type::Set(x), Type::Set(y)) => structural_equal(x, y),
        (Type::Ref(x), Type::Ref(y)) => structural_equal(x, y),
        (Type::Builder, Type::Builder) => true,
        (Type::Bytes, Type::Bytes) => true,
        (Type::Union(xs), Type::Union(ys)) => {
            if xs.len() != ys.len() {
                return false;
            }
            let mut used = vec![false; ys.len()];
            for x in xs {
                let mut found = false;
                for (i, y) in ys.iter().enumerate() {
                    if !used[i] && structural_equal(x, y) {
                        used[i] = true;
                        found = true;
                        break;
                    }
                }
                if !found {
                    return false;
                }
            }
            true
        }
        (Type::Obj(x), Type::Obj(y)) => {
            if x.len() != y.len() {
                return false;
            }
            x.iter().all(|(k, v)| y.get(k).is_some_and(|yv| structural_equal(v, yv)))
        }
        (Type::ResultT(x1, x2), Type::ResultT(y1, y2)) => {
            structural_equal(x1, y1) && structural_equal(x2, y2)
        }
        (Type::FnT(xs, xr), Type::FnT(ys, yr)) => {
            xs.len() == ys.len()
                && xs.iter().zip(ys.iter()).all(|(x, y)| structural_equal(x, y))
                && structural_equal(xr, yr)
        }
        _ => false,
    }
}

pub fn normalize_union(members: Vec<Type>, aliases: &AliasEnv) -> Vec<Type> {
    let mut out: Vec<Type> = Vec::new();
    for m in members {
        if !out.iter().any(|o| structural_equal(o, &m)) {
            out.push(m);
        }
    }
    out.sort_by_key(|t| print_type(t, aliases));
    out
}

pub fn normalize_type(t: &Type, aliases: &AliasEnv) -> Type {
    let mut seen = BTreeSet::new();
    normalize_type_seen(t, aliases, &mut seen)
}

fn normalize_type_seen(t: &Type, aliases: &AliasEnv, seen: &mut BTreeSet<Rc<str>>) -> Type {
    match t {
        Type::Alias(name) => {
            if seen.contains(name) {
                return t.clone();
            }
            match aliases.get(name) {
                None => t.clone(),
                Some(def) => {
                    seen.insert(name.clone());
                    let r = normalize_type_seen(def, aliases, seen);
                    seen.remove(name);
                    r
                }
            }
        }
        Type::Union(members) => {
            let mut flat: Vec<Type> = Vec::new();
            for m in members {
                let n = normalize_type_seen(m, aliases, seen);
                if let Type::Union(inner) = n {
                    flat.extend(inner);
                } else {
                    flat.push(n);
                }
            }
            Type::Union(normalize_union(flat, aliases))
        }
        Type::Array(el) => Type::Array(Box::new(normalize_type_seen(el, aliases, seen))),
        Type::Map(k, v) => Type::Map(
            Box::new(normalize_type_seen(k, aliases, seen)),
            Box::new(normalize_type_seen(v, aliases, seen)),
        ),
        Type::Set(k) => Type::Set(Box::new(normalize_type_seen(k, aliases, seen))),
        Type::Ref(inner) => Type::Ref(Box::new(normalize_type_seen(inner, aliases, seen))),
        Type::Builder => Type::Builder,
        Type::Obj(fields) => Type::Obj(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), normalize_type_seen(v, aliases, seen)))
                .collect(),
        ),
        Type::ResultT(ok, err) => Type::ResultT(
            Box::new(normalize_type_seen(ok, aliases, seen)),
            Box::new(normalize_type_seen(err, aliases, seen)),
        ),
        Type::FnT(params, ret) => Type::FnT(
            params
                .iter()
                .map(|p| normalize_type_seen(p, aliases, seen))
                .collect(),
            Box::new(normalize_type_seen(ret, aliases, seen)),
        ),
        _ => t.clone(),
    }
}

pub fn find_unresolved_alias(t: &Type) -> Option<Rc<str>> {
    match t {
        Type::Alias(name) => Some(name.clone()),
        Type::Array(el) => find_unresolved_alias(el),
        Type::Map(k, v) => find_unresolved_alias(k).or_else(|| find_unresolved_alias(v)),
        Type::Set(k) => find_unresolved_alias(k),
        Type::Ref(inner) => find_unresolved_alias(inner),
        Type::Union(members) => members.iter().find_map(find_unresolved_alias),
        Type::Obj(fields) => fields.values().find_map(find_unresolved_alias),
        Type::ResultT(ok, err) => find_unresolved_alias(ok).or_else(|| find_unresolved_alias(err)),
        Type::FnT(params, ret) => params
            .iter()
            .find_map(find_unresolved_alias)
            .or_else(|| find_unresolved_alias(ret)),
        _ => None,
    }
}

pub fn print_type(t: &Type, aliases: &AliasEnv) -> String {
    let mut seen = BTreeSet::new();
    print_type_seen(t, aliases, &mut seen)
}

pub fn print_type_plain(t: &Type) -> String {
    print_type(t, &AliasEnv::new())
}

fn print_type_seen(t: &Type, aliases: &AliasEnv, seen: &mut BTreeSet<Rc<str>>) -> String {
    match t {
        Type::Number => "number".to_string(),
        Type::Dec => "decimal".to_string(),
        Type::Str => "string".to_string(),
        Type::Bool => "bool".to_string(),
        Type::Char => "char".to_string(),
        Type::Nil => "nil".to_string(),
        Type::Literal(v) => serde_json::to_string(v.as_ref()).unwrap_or_else(|_| format!("\"{v}\"")),
        Type::Var(id) => format!("?{id}"),
        Type::Alias(name) => match aliases.get(name) {
            Some(def) if !seen.contains(name) => {
                seen.insert(name.clone());
                let r = print_type_seen(&normalize_type(def, aliases), aliases, seen);
                seen.remove(name);
                r
            }
            _ => name.to_string(),
        },
        Type::Array(el) => format!("(Array {})", print_type_seen(el, aliases, seen)),
        Type::Map(k, v) => format!(
            "(map {} {})",
            print_type_seen(k, aliases, seen),
            print_type_seen(v, aliases, seen)
        ),
        Type::Set(k) => format!("(set {})", print_type_seen(k, aliases, seen)),
        Type::Ref(inner) => format!("(ref {})", print_type_seen(inner, aliases, seen)),
        Type::Builder => "builder".to_string(),
        Type::Bytes => "bytes".to_string(),
        Type::Union(members) => {
            let members = normalize_union(members.clone(), aliases);
            let non_nil: Vec<&Type> = members.iter().filter(|m| !matches!(m, Type::Nil)).collect();
            if non_nil.len() == 1 && members.len() == 2 {
                return format!("(Option {})", print_type_seen(non_nil[0], aliases, seen));
            }
            format!(
                "(union {})",
                members
                    .iter()
                    .map(|m| print_type_seen(m, aliases, seen))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        }
        Type::Obj(fields) => {
            let mut entries: Vec<(&Rc<str>, &Type)> = fields.iter().collect();
            entries.sort_by_key(|(k, _)| (*k).clone());
            let body = entries
                .iter()
                .map(|(k, v)| match v {
                    Type::Literal(lit) => format!("({k} {})", serde_json::to_string(lit.as_ref()).unwrap_or_else(|_| format!("\"{lit}\""))),
                    _ => format!("({k} : {})", print_type_seen(v, aliases, seen)),
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!("(obj {body})")
        }
        Type::ResultT(ok, err) => format!(
            "(Result {} {})",
            print_type_seen(ok, aliases, seen),
            print_type_seen(err, aliases, seen)
        ),
        Type::FnT(params, ret) => format!(
            "(fn ({}) : {})",
            params
                .iter()
                .map(|p| print_type_seen(p, aliases, seen))
                .collect::<Vec<_>>()
                .join(" "),
            print_type_seen(ret, aliases, seen)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(s: &str) -> Type {
        Type::Literal(Rc::from(s))
    }
    fn arr(el: Type) -> Type {
        Type::Array(Box::new(el))
    }
    fn map_t(k: Type, v: Type) -> Type {
        Type::Map(Box::new(k), Box::new(v))
    }
    fn set_t(k: Type) -> Type {
        Type::Set(Box::new(k))
    }
    fn union(ms: Vec<Type>) -> Type {
        Type::Union(ms)
    }
    fn obj(fields: &[(&str, Type)]) -> Type {
        Type::Obj(fields.iter().map(|(k, v)| (Rc::from(*k), v.clone())).collect())
    }
    fn res(ok: Type, err: Type) -> Type {
        Type::ResultT(Box::new(ok), Box::new(err))
    }
    fn fn_t(params: Vec<Type>, ret: Type) -> Type {
        Type::FnT(params, Box::new(ret))
    }

    #[test]
    fn normalize_union_dedups_and_sorts_by_print_bytes() {
        let ms = normalize_union(
            vec![Type::Str, Type::Number, Type::Str, Type::Nil],
            &AliasEnv::new(),
        );
        let printed: Vec<String> = ms.iter().map(print_type_plain).collect();
        assert_eq!(printed, ["nil", "number", "string"]);
    }

    #[test]
    fn print_type_canonical_forms() {
        assert_eq!(print_type_plain(&Type::Number), "number");
        assert_eq!(print_type_plain(&Type::Char), "char");
        assert!(structural_equal(&Type::Char, &Type::Char));
        assert!(!structural_equal(&Type::Char, &Type::Str));
        assert_eq!(print_type_plain(&lit("EmptyCart")), "\"EmptyCart\"");
        assert_eq!(print_type_plain(&arr(Type::Number)), "(Array number)");
        assert_eq!(
            print_type_plain(&res(Type::Number, Type::Str)),
            "(Result number string)"
        );
        assert_eq!(
            print_type_plain(&union(vec![Type::Number, Type::Nil])),
            "(Option number)"
        );
        assert_eq!(
            print_type_plain(&union(vec![Type::Number, Type::Str, Type::Nil])),
            "(union nil number string)"
        );
        assert_eq!(
            print_type_plain(&obj(&[("b", Type::Str), ("a", Type::Number)])),
            "(obj (a : number) (b : string))"
        );
        assert_eq!(
            print_type_plain(&fn_t(vec![Type::Number, Type::Str], Type::Bool)),
            "(fn (number string) : bool)"
        );
        assert_eq!(print_type_plain(&fn_t(vec![], Type::Number)), "(fn () : number)");
        assert_eq!(print_type_plain(&Type::Var(7)), "?7");
        assert_eq!(
            print_type_plain(&map_t(Type::Str, Type::Number)),
            "(map string number)"
        );
        assert_eq!(print_type_plain(&set_t(Type::Number)), "(set number)");
    }

    #[test]
    fn map_set_structural_equal_and_union_sort() {
        let e = AliasEnv::new();
        assert!(structural_equal(
            &map_t(Type::Str, Type::Number),
            &map_t(Type::Str, Type::Number)
        ));
        assert!(!structural_equal(
            &map_t(Type::Str, Type::Number),
            &map_t(Type::Number, Type::Number)
        ));
        assert!(!structural_equal(&set_t(Type::Str), &map_t(Type::Str, Type::Nil)));
        let ms = normalize_union(
            vec![map_t(Type::Str, Type::Number), Type::Str, set_t(Type::Str)],
            &e,
        );
        let printed: Vec<String> = ms.iter().map(print_type_plain).collect();
        assert_eq!(printed, ["(map string number)", "(set string)", "string"]);
    }

    #[test]
    fn print_type_alias_expansion() {
        let mut aliases = AliasEnv::new();
        aliases.insert(
            Rc::from("Money"),
            obj(&[("amount", Type::Number), ("currency", Type::Str)]),
        );
        assert_eq!(
            print_type(&Type::Alias(Rc::from("Money")), &aliases),
            "(obj (amount : number) (currency : string))"
        );
        assert_eq!(print_type_plain(&Type::Alias(Rc::from("Money"))), "Money");
    }

    #[test]
    fn normalize_type_flattens_union_aliases() {
        let mut aliases = AliasEnv::new();
        aliases.insert(
            Rc::from("E"),
            union(vec![
                obj(&[("tag", lit("A"))]),
                obj(&[("tag", lit("B"))]),
            ]),
        );
        let n = normalize_type(&union(vec![Type::Alias(Rc::from("E")), Type::Str]), &aliases);
        assert_eq!(
            print_type(&n, &aliases),
            "(union (obj (tag \"A\")) (obj (tag \"B\")) string)"
        );
    }
}
