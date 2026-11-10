use crate::types::{normalize_type, AliasEnv, Type};

pub type Subst = Vec<Option<Type>>;

pub fn prune(t: &Type, subst: &Subst) -> Type {
    let mut cur = t.clone();
    while let Type::Var(id) = cur {
        match subst.get(id).and_then(|o| o.as_ref()) {
            Some(next) => cur = next.clone(),
            None => return cur,
        }
    }
    cur
}

pub fn occurs(id: usize, t: &Type, subst: &Subst) -> bool {
    let p = prune(t, subst);
    match &p {
        Type::Var(v) => *v == id,
        Type::Array(el) => occurs(id, el, subst),
        Type::Map(k, v) => occurs(id, k, subst) || occurs(id, v, subst),
        Type::Set(k) => occurs(id, k, subst),
        Type::Ref(inner) => occurs(id, inner, subst),
        Type::Union(members) => members.iter().any(|m| occurs(id, m, subst)),
        Type::Obj(fields) => fields.values().any(|v| occurs(id, v, subst)),
        Type::ResultT(ok, err) => occurs(id, ok, subst) || occurs(id, err, subst),
        Type::FnT(params, ret) => {
            params.iter().any(|x| occurs(id, x, subst)) || occurs(id, ret, subst)
        }
        _ => false,
    }
}

pub fn apply_subst(t: &Type, subst: &Subst) -> Type {
    let p = prune(t, subst);
    match &p {
        Type::Var(_) => p,
        Type::Array(el) => Type::Array(Box::new(apply_subst(el, subst))),
        Type::Map(k, v) => Type::Map(Box::new(apply_subst(k, subst)), Box::new(apply_subst(v, subst))),
        Type::Set(k) => Type::Set(Box::new(apply_subst(k, subst))),
        Type::Ref(inner) => Type::Ref(Box::new(apply_subst(inner, subst))),
        Type::Union(members) => Type::Union(members.iter().map(|m| apply_subst(m, subst)).collect()),
        Type::Obj(fields) => Type::Obj(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), apply_subst(v, subst)))
                .collect(),
        ),
        Type::ResultT(ok, err) => Type::ResultT(
            Box::new(apply_subst(ok, subst)),
            Box::new(apply_subst(err, subst)),
        ),
        Type::FnT(params, ret) => Type::FnT(
            params.iter().map(|x| apply_subst(x, subst)).collect(),
            Box::new(apply_subst(ret, subst)),
        ),
        _ => p,
    }
}

fn set_var(subst: &mut Subst, id: usize, t: Type) {
    if subst.len() <= id {
        subst.resize(id + 1, None);
    }
    subst[id] = Some(t);
}

pub fn try_unify(a: &Type, b: &Type, subst: &mut Subst, aliases: &AliasEnv) -> bool {
    let snap = subst.clone();
    if unify(a, b, subst, aliases).is_some() {
        return true;
    }
    *subst = snap;
    false
}

pub fn unify(a0: &Type, b0: &Type, subst: &mut Subst, aliases: &AliasEnv) -> Option<Type> {
    let mut a = prune(a0, subst);
    let mut b = prune(b0, subst);
    if matches!(a, Type::Alias(_)) {
        a = prune(&normalize_type(&a, aliases), subst);
    }
    if matches!(b, Type::Alias(_)) {
        b = prune(&normalize_type(&b, aliases), subst);
    }

    if let Type::Var(id) = &a {
        let id = *id;
        if matches!(&b, Type::Var(bid) if *bid == id) {
            return Some(a.clone());
        }
        if occurs(id, &b, subst) {
            return None;
        }
        set_var(subst, id, b.clone());
        return Some(b);
    }
    if let Type::Var(id) = &b {
        let id = *id;
        if occurs(id, &a, subst) {
            return None;
        }
        set_var(subst, id, a.clone());
        return Some(a);
    }
    if let Type::Union(members) = &a {
        if let Type::Union(bm) = &b {
            for ma in members {
                if !bm.iter().any(|mb| try_unify(ma, mb, subst, aliases)) {
                    return None;
                }
            }
            return Some(a.clone());
        }
        for ma in members {
            if try_unify(ma, &b, subst, aliases) {
                return Some(a.clone());
            }
        }
        return None;
    }
    if let Type::Union(members) = &b {
        for mb in members {
            if try_unify(&a, mb, subst, aliases) {
                return Some(a.clone());
            }
        }
        return None;
    }
    if std::mem::discriminant(&a) != std::mem::discriminant(&b) {
        if matches!((&a, &b), (Type::Literal(_), Type::Str) | (Type::Str, Type::Literal(_))) {
            return Some(a.clone());
        }
        return None;
    }
    match (&a, &b) {
        (Type::Number, Type::Number)
        | (Type::Dec, Type::Dec)
        | (Type::Str, Type::Str)
        | (Type::Bool, Type::Bool)
        | (Type::Char, Type::Char)
        | (Type::Nil, Type::Nil) => Some(a.clone()),
        (Type::Literal(x), Type::Literal(y)) if x == y => Some(a.clone()),
        (Type::Array(x), Type::Array(y)) => unify(x, y, subst, aliases),
        (Type::Map(xk, xv), Type::Map(yk, yv)) => {
            unify(xk, yk, subst, aliases)?;
            unify(xv, yv, subst, aliases)?;
            Some(a.clone())
        }
        (Type::Set(x), Type::Set(y)) => {
            unify(x, y, subst, aliases)?;
            Some(a.clone())
        }
        (Type::Ref(x), Type::Ref(y)) => {
            unify(x, y, subst, aliases)?;
            Some(a.clone())
        }
        (Type::Builder, Type::Builder) => Some(a.clone()),
        (Type::Bytes, Type::Bytes) => Some(a.clone()),
        (Type::Obj(fa), Type::Obj(fb)) => {
            for (k, bv) in fb {
                let av = fa.get(k)?;
                unify(av, bv, subst, aliases)?;
            }
            Some(a.clone())
        }
        (Type::ResultT(x1, x2), Type::ResultT(y1, y2)) => {
            unify(x1, y1, subst, aliases)?;
            unify(x2, y2, subst, aliases)?;
            Some(a.clone())
        }
        (Type::FnT(xs, xr), Type::FnT(ys, yr)) => {
            if xs.len() != ys.len() {
                return None;
            }
            for (x, y) in xs.iter().zip(ys.iter()) {
                unify(x, y, subst, aliases)?;
            }
            unify(xr, yr, subst, aliases)?;
            Some(a.clone())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::rc::Rc;

    fn lit(s: &str) -> Type {
        Type::Literal(Rc::from(s))
    }
    fn v(id: usize) -> Type {
        Type::Var(id)
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
        Type::Obj(fields.iter().map(|(k, t)| (Rc::from(*k), t.clone())).collect())
    }
    fn res(ok: Type, err: Type) -> Type {
        Type::ResultT(Box::new(ok), Box::new(err))
    }
    fn fn_t(params: Vec<Type>, ret: Type) -> Type {
        Type::FnT(params, Box::new(ret))
    }
    fn u(a: &Type, b: &Type, subst: &mut Subst, aliases: &AliasEnv) -> bool {
        unify(a, b, subst, aliases).is_some()
    }

    #[test]
    fn structural_identical_and_prims() {
        let e = AliasEnv::new();
        assert!(u(&Type::Number, &Type::Number, &mut vec![], &e));
        assert!(!u(&Type::Number, &Type::Str, &mut vec![], &e));
        assert!(u(
            &res(Type::Number, Type::Str),
            &res(Type::Number, Type::Str),
            &mut vec![],
            &e
        ));
        assert!(u(
            &fn_t(vec![Type::Number], Type::Bool),
            &fn_t(vec![Type::Number], Type::Bool),
            &mut vec![],
            &e
        ));
        assert!(!u(
            &fn_t(vec![Type::Number], Type::Bool),
            &fn_t(vec![Type::Number, Type::Number], Type::Bool),
            &mut vec![],
            &e
        ));
    }

    #[test]
    fn literal_types() {
        let e = AliasEnv::new();
        assert!(u(&lit("A"), &lit("A"), &mut vec![], &e));
        assert!(!u(&lit("A"), &lit("B"), &mut vec![], &e));
        assert!(u(&lit("A"), &Type::Str, &mut vec![], &e));
        assert!(u(&Type::Str, &lit("A"), &mut vec![], &e));
    }

    #[test]
    fn type_against_union_picks_a_member() {
        let e = AliasEnv::new();
        assert!(u(
            &Type::Number,
            &union(vec![Type::Str, Type::Number]),
            &mut vec![],
            &e
        ));
        assert!(!u(
            &Type::Number,
            &union(vec![Type::Str, Type::Bool]),
            &mut vec![],
            &e
        ));
    }

    #[test]
    fn union_subset_rule() {
        let e = AliasEnv::new();
        let a = union(vec![Type::Number, Type::Str]);
        let b = union(vec![Type::Number, Type::Str, Type::Bool]);
        assert!(u(&a, &b, &mut vec![], &e));
        assert!(!u(&b, &a, &mut vec![], &e));
    }

    #[test]
    fn obj_width_subtyping() {
        let e = AliasEnv::new();
        let wide = obj(&[("a", Type::Number), ("b", Type::Str)]);
        let narrow = obj(&[("a", Type::Number)]);
        assert!(u(&wide, &narrow, &mut vec![], &e));
        assert!(!u(&narrow, &wide, &mut vec![], &e));
        assert!(!u(
            &obj(&[("a", Type::Number)]),
            &obj(&[("a", Type::Str)]),
            &mut vec![],
            &e
        ));
    }

    #[test]
    fn var_bind_and_occurs_check() {
        let e = AliasEnv::new();
        let mut subst: Subst = Vec::new();
        assert!(u(&v(1), &Type::Number, &mut subst, &e));
        assert_eq!(subst.get(1).and_then(|o| o.as_ref()), Some(&Type::Number));
        assert!(!u(&v(2), &arr(v(2)), &mut Vec::new(), &e));
        let mut s2: Subst = Vec::new();
        assert!(u(&v(1), &arr(Type::Number), &mut s2, &e));
        assert!(u(&v(1), &arr(Type::Number), &mut s2, &e));
        assert!(!u(&v(1), &Type::Str, &mut s2, &e));
    }

    #[test]
    fn map_and_set_unify() {
        let e = AliasEnv::new();
        assert!(u(
            &map_t(Type::Str, Type::Number),
            &map_t(Type::Str, Type::Number),
            &mut vec![],
            &e
        ));
        assert!(!u(
            &map_t(Type::Str, Type::Number),
            &map_t(Type::Bool, Type::Number),
            &mut vec![],
            &e
        ));
        assert!(!u(
            &map_t(Type::Str, Type::Number),
            &set_t(Type::Str),
            &mut vec![],
            &e
        ));
        assert!(!u(&set_t(Type::Str), &map_t(Type::Str, Type::Nil), &mut vec![], &e));
        // 変数キーは unify で具体化される
        let mut s: Subst = Vec::new();
        assert!(u(
            &map_t(v(1), v(2)),
            &map_t(Type::Str, Type::Number),
            &mut s,
            &e
        ));
        assert_eq!(prune(&v(1), &s), Type::Str);
        assert_eq!(prune(&v(2), &s), Type::Number);
        // occurs check: (map ?1 ?1) vs (map string (map string ?1)) は失敗
        let mut s2: Subst = Vec::new();
        assert!(!u(
            &map_t(v(1), v(1)),
            &map_t(Type::Str, map_t(Type::Str, v(1))),
            &mut s2,
            &e
        ));
    }

    #[test]
    fn array_and_alias_expansion() {
        assert!(u(
            &arr(Type::Number),
            &arr(Type::Number),
            &mut vec![],
            &AliasEnv::new()
        ));
        assert!(!u(
            &arr(Type::Number),
            &arr(Type::Str),
            &mut vec![],
            &AliasEnv::new()
        ));
        let mut aliases: BTreeMap<Rc<str>, Type> = BTreeMap::new();
        aliases.insert(Rc::from("Money"), obj(&[("amount", Type::Number)]));
        assert!(u(
            &Type::Alias(Rc::from("Money")),
            &obj(&[("amount", Type::Number)]),
            &mut vec![],
            &aliases
        ));
        assert!(!u(
            &Type::Alias(Rc::from("Money")),
            &obj(&[("amount", Type::Str)]),
            &mut vec![],
            &aliases
        ));
    }
}
