use crate::ir::{Pat, PatKind};
use crate::types::{normalize_type, AliasEnv, Type};
use crate::unify::{prune, Subst};
use crate::value::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct ExhaustiveResult {
    pub missing: Vec<Pat>,
    pub redundant: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
enum Case {
    Value(Value),
    Type(Type),
}

fn lit_type_of(value: &Value) -> Type {
    match value {
        Value::Nil => Type::Nil,
        Value::Dec(_) => Type::Dec,
        Value::Str(s) => Type::Literal(s.clone()),
        Value::Num(_) => Type::Number,
        Value::Char(_) => Type::Char,
        Value::Bool(_) => Type::Bool,
        _ => Type::Bool,
    }
}

fn cases_of(t: &Type, subst: &Subst, aliases: &AliasEnv) -> Vec<Case> {
    let tt = prune(&normalize_type(t, aliases), subst);
    match &tt {
        Type::Union(members) => members
            .iter()
            .flat_map(|m| cases_of(m, subst, aliases))
            .collect(),
        Type::Bool => vec![
            Case::Value(Value::Bool(true)),
            Case::Value(Value::Bool(false)),
        ],
        Type::Nil => vec![Case::Value(Value::Nil)],
        Type::Literal(v) => vec![Case::Value(Value::Str(v.clone()))],
        _ => vec![Case::Type(tt)],
    }
}

fn case_type(c: &Case, subst: &Subst, aliases: &AliasEnv) -> Option<Type> {
    match c {
        Case::Value(v) => Some(lit_type_of(v)),
        Case::Type(t) => Some(prune(&normalize_type(t, aliases), subst)),
    }
}

fn pattern_covers_case(p: &Pat, c: &Case, subst: &Subst, aliases: &AliasEnv) -> bool {
    match &p.kind {
        PatKind::Wildcard | PatKind::Var(_) => true,
        PatKind::Lit(v) => {
            if let Case::Value(cv) = c {
                return v == cv;
            }
            let Some(ct) = case_type(c, subst, aliases) else {
                return false;
            };
            match v {
                Value::Nil => matches!(ct, Type::Nil),
                Value::Str(s) => matches!(&ct, Type::Literal(l) if l == s),
                _ => false,
            }
        }
        PatKind::Or(ps) => ps
            .iter()
            .any(|sub| pattern_covers_case(sub, c, subst, aliases)),
        PatKind::Obj(fields) => {
            let Case::Type(t) = c else {
                return false;
            };
            let t = prune(&normalize_type(t, aliases), subst);
            let Type::Obj(tf) = &t else {
                return false;
            };
            fields.iter().all(|(n, sub)| {
                tf.get(n).is_some_and(|ft| {
                    pattern_covers_case(sub, &Case::Type(ft.clone()), subst, aliases)
                })
            })
        }
        PatKind::Array(_) => false,
    }
}

fn case_to_pattern(c: &Case) -> Pat {
    let wildcard = || Pat {
        kind: PatKind::Wildcard,
        span: crate::span::Span {
            file: std::rc::Rc::from(""),
            start: 0,
            end: 0,
            line: 1,
            col: 1,
        },
    };
    match c {
        Case::Value(v) => Pat {
            kind: PatKind::Lit(v.clone()),
            span: wildcard().span,
        },
        Case::Type(t) => match t {
            Type::Literal(v) => Pat {
                kind: PatKind::Lit(Value::Str(v.clone())),
                span: wildcard().span,
            },
            Type::Obj(fields) => Pat {
                kind: PatKind::Obj(
                    fields
                        .iter()
                        .map(|(n, ft)| (n.clone(), case_to_pattern(&Case::Type(ft.clone()))))
                        .collect(),
                ),
                span: wildcard().span,
            },
            Type::Union(members) => Pat {
                kind: PatKind::Or(
                    members
                        .iter()
                        .map(|m| case_to_pattern(&Case::Type(m.clone())))
                        .collect(),
                ),
                span: wildcard().span,
            },
            _ => wildcard(),
        },
    }
}

fn pattern_equal(a: &Pat, b: &Pat) -> bool {
    if std::mem::discriminant(&a.kind) != std::mem::discriminant(&b.kind) {
        return false;
    }
    match (&a.kind, &b.kind) {
        (PatKind::Wildcard, PatKind::Wildcard) => true,
        (PatKind::Var(x), PatKind::Var(y)) => x == y,
        (PatKind::Lit(x), PatKind::Lit(y)) => x == y,
        (PatKind::Obj(xs), PatKind::Obj(ys)) => {
            xs.len() == ys.len()
                && xs
                    .iter()
                    .zip(ys.iter())
                    .all(|((n, x), (m, y))| n == m && pattern_equal(x, y))
        }
        (PatKind::Array(xs), PatKind::Array(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys.iter()).all(|(x, y)| pattern_equal(x, y))
        }
        (PatKind::Or(xs), PatKind::Or(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys.iter()).all(|(x, y)| pattern_equal(x, y))
        }
        _ => false,
    }
}

fn patterns_equal(a: &[Pat], b: &[Pat]) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| pattern_equal(x, y))
}

fn missing_cases(t: &Type, arms: &[Pat], subst: &Subst, aliases: &AliasEnv) -> Vec<Pat> {
    let cases = cases_of(t, subst, aliases);
    let mut out: Vec<Pat> = Vec::new();
    for c in &cases {
        if !arms.iter().any(|a| pattern_covers_case(a, c, subst, aliases)) {
            let p = case_to_pattern(c);
            if !out.iter().any(|o| pattern_equal(o, &p)) {
                out.push(p);
            }
        }
    }
    out
}

pub fn check_exhaustive(
    scrutinee: &Type,
    arms: &[Pat],
    subst: &mut Subst,
    aliases: &AliasEnv,
) -> ExhaustiveResult {
    let snap = subst.clone();
    let missing = missing_cases(scrutinee, arms, subst, aliases);
    let mut redundant: Vec<usize> = Vec::new();
    for i in 1..arms.len() {
        let before = missing_cases(scrutinee, &arms[..i], subst, aliases);
        let after = missing_cases(scrutinee, &arms[..=i], subst, aliases);
        if patterns_equal(&before, &after) {
            redundant.push(i);
        }
    }
    *subst = snap;
    ExhaustiveResult { missing, redundant }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::print_pattern;
    use crate::check::{check_file, check_source, ModuleCache};
    use crate::diagnostic::{DiagKind, Patch};
    use crate::span::Span;
    use std::collections::BTreeMap;
    use std::rc::Rc;

    fn dummy_span() -> Span {
        Span {
            file: Rc::from(""),
            start: 0,
            end: 0,
            line: 1,
            col: 1,
        }
    }

    fn p_wild() -> Pat {
        Pat {
            kind: PatKind::Wildcard,
            span: dummy_span(),
        }
    }

    fn p_var(name: &str) -> Pat {
        Pat {
            kind: PatKind::Var(Rc::from(name)),
            span: dummy_span(),
        }
    }

    fn p_lit(v: Value) -> Pat {
        Pat {
            kind: PatKind::Lit(v),
            span: dummy_span(),
        }
    }

    fn p_obj(fields: Vec<(&str, Pat)>) -> Pat {
        Pat {
            kind: PatKind::Obj(
                fields
                    .into_iter()
                    .map(|(n, p)| (Rc::from(n), p))
                    .collect(),
            ),
            span: dummy_span(),
        }
    }

    fn p_arr(els: Vec<Pat>) -> Pat {
        Pat {
            kind: PatKind::Array(els),
            span: dummy_span(),
        }
    }

    fn p_or(ps: Vec<Pat>) -> Pat {
        Pat {
            kind: PatKind::Or(ps),
            span: dummy_span(),
        }
    }

    fn lit_t(s: &str) -> Type {
        Type::Literal(Rc::from(s))
    }

    fn obj_t(fields: Vec<(&str, Type)>) -> Type {
        Type::Obj(
            fields
                .into_iter()
                .map(|(k, v)| (Rc::from(k), v))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    fn union_t(ms: Vec<Type>) -> Type {
        Type::Union(ms)
    }

    fn tag_obj(tag: &str, extra: Vec<(&str, Type)>) -> Type {
        let mut fields = vec![("tag", lit_t(tag))];
        fields.extend(extra);
        obj_t(fields)
    }

    fn tag_pat(tag: &str, extra: Vec<(&str, Pat)>) -> Pat {
        let mut fields = vec![("tag", p_lit(Value::Str(Rc::from(tag))))];
        fields.extend(extra);
        p_obj(fields)
    }

    fn run(t: &Type, arms: Vec<Pat>) -> ExhaustiveResult {
        check_exhaustive(t, &arms, &mut Vec::new(), &AliasEnv::new())
    }

    #[test]
    fn tag_union_full_cover() {
        let e = union_t(vec![tag_obj("A", vec![]), tag_obj("B", vec![])]);
        let r = run(&e, vec![tag_pat("A", vec![]), tag_pat("B", vec![])]);
        assert!(r.missing.is_empty());
        assert!(r.redundant.is_empty());
    }

    #[test]
    fn tag_union_missing_member() {
        let e = union_t(vec![tag_obj("A", vec![]), tag_obj("B", vec![])]);
        let r = run(&e, vec![tag_pat("A", vec![])]);
        let printed: Vec<String> = r.missing.iter().map(print_pattern).collect();
        assert_eq!(printed, ["(obj (tag \"B\"))"]);
    }

    #[test]
    fn or_pattern_covers() {
        let e = union_t(vec![tag_obj("A", vec![]), tag_obj("B", vec![])]);
        let r = run(&e, vec![p_or(vec![tag_pat("A", vec![]), tag_pat("B", vec![])])]);
        assert!(r.missing.is_empty());
    }

    #[test]
    fn wildcard_and_var_cover_everything() {
        let e = union_t(vec![obj_t(vec![("id", Type::Str)]), Type::Nil]);
        let r = run(&e, vec![p_lit(Value::Nil), p_var("i")]);
        assert!(r.missing.is_empty());
    }

    #[test]
    fn bool_cases() {
        assert!(run(&Type::Bool, vec![p_lit(Value::Bool(true)), p_lit(Value::Bool(false))])
            .missing
            .is_empty());
        let r = run(&Type::Bool, vec![p_lit(Value::Bool(true))]);
        let printed: Vec<String> = r.missing.iter().map(print_pattern).collect();
        assert_eq!(printed, ["false"]);
    }

    #[test]
    fn array_pattern_does_not_cover() {
        let r = run(
            &Type::Array(Box::new(Type::Number)),
            vec![p_arr(vec![p_var("x")])],
        );
        let printed: Vec<String> = r.missing.iter().map(print_pattern).collect();
        assert_eq!(printed, ["_"]);
    }

    #[test]
    fn redundant_arm_detection() {
        let r = run(
            &Type::Bool,
            vec![
                p_lit(Value::Bool(true)),
                p_lit(Value::Bool(false)),
                p_lit(Value::Bool(true)),
            ],
        );
        assert!(r.missing.is_empty());
        assert_eq!(r.redundant, vec![2]);

        let r = run(&Type::Bool, vec![p_var("x"), p_lit(Value::Bool(true))]);
        assert_eq!(r.redundant, vec![1]);
    }

    #[test]
    fn obj_pattern_missing_extra_field_in_template() {
        let e = union_t(vec![
            tag_obj("A", vec![]),
            tag_obj("B", vec![("new-price", Type::Number)]),
        ]);
        let r = run(&e, vec![tag_pat("A", vec![])]);
        let printed: Vec<String> = r.missing.iter().map(print_pattern).collect();
        assert_eq!(printed, ["(obj (new-price _) (tag \"B\"))"]);
    }

    #[test]
    fn print_pattern_forms() {
        assert_eq!(print_pattern(&p_wild()), "_");
        assert_eq!(print_pattern(&p_var("p")), "p");
        assert_eq!(print_pattern(&p_lit(Value::Str(Rc::from("A")))), "\"A\"");
        assert_eq!(print_pattern(&p_lit(Value::Nil)), "nil");
        assert_eq!(
            print_pattern(&tag_pat("PriceChanged", vec![("new-price", p_var("p"))])),
            "(obj (tag \"PriceChanged\") (new-price p))"
        );
        assert_eq!(
            print_pattern(&p_arr(vec![p_var("x"), p_wild()])),
            "(Array x _)"
        );
        assert_eq!(
            print_pattern(&p_or(vec![p_lit(Value::Num(1.0)), p_wild()])),
            "(or 1 _)"
        );
    }

    fn fixture_path(name: &str) -> String {
        format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
    }

    #[test]
    fn stock_lts_missing_out_of_stock_arm_with_insert_fix() {
        let mut cache = ModuleCache::default();
        let r = check_file(&fixture_path("stock.lts"), &mut cache);
        let d = r
            .diagnostics
            .iter()
            .find(|d| d.kind == DiagKind::NonExhaustive)
            .expect("non-exhaustive");
        assert_eq!(d.severity, crate::diagnostic::Severity::Error);
        let data = d.data.as_ref().expect("data");
        assert_eq!(data["missing"], serde_json::json!(["(obj (tag \"OutOfStock\"))"]));
        assert!(matches!(&d.fix, Some(Patch::Insert { path, template })
            if path == "(defn show-error).body.arms" && template.contains("[(obj (tag \"OutOfStock\")) \"...\"]")));
        assert!(!r
            .diagnostics
            .iter()
            .any(|d| d.kind == DiagKind::RedundantArm));
    }

    #[test]
    fn cart_lts_check_items_match_is_exhaustive() {
        let mut cache = ModuleCache::default();
        let r = check_file(&fixture_path("cart.lts"), &mut cache);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    }

    #[test]
    fn source_tag_full_cover_is_clean() {
        let mut cache = ModuleCache::default();
        let r = check_source(
            "(type E (union (obj (tag \"A\")) (obj (tag \"B\")))) (defn f (e : E) : number (match e [(obj (tag \"A\")) 1] [(obj (tag \"B\")) 2]))",
            "test.lts",
            &mut cache,
        );
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    }

    #[test]
    fn source_tag_partial_cover_reports_missing_arm() {
        let mut cache = ModuleCache::default();
        let r = check_source(
            "(type E (union (obj (tag \"A\")) (obj (tag \"B\")))) (defn f (e : E) : number (match e [(obj (tag \"A\")) 1]))",
            "test.lts",
            &mut cache,
        );
        let d = r
            .diagnostics
            .iter()
            .find(|d| d.kind == DiagKind::NonExhaustive)
            .expect("non-exhaustive");
        let data = d.data.as_ref().expect("data");
        assert_eq!(data["missing"], serde_json::json!(["(obj (tag \"B\"))"]));
        assert!(matches!(&d.fix, Some(Patch::Insert { template, .. })
            if template.contains("[(obj (tag \"B\")) \"...\"]")));
    }

    #[test]
    fn source_redundant_arm_is_warning_with_delete_fix() {
        let mut cache = ModuleCache::default();
        let r = check_source(
            "(defn f (x : bool) : bool (match x [true true] [false false] [true 0]))",
            "test.lts",
            &mut cache,
        );
        let d = r
            .diagnostics
            .iter()
            .find(|d| d.kind == DiagKind::RedundantArm)
            .expect("redundant-arm");
        assert_eq!(d.severity, crate::diagnostic::Severity::Warning);
        assert!(matches!(&d.fix, Some(Patch::Delete { path })
            if path == "(defn f).body.arms.2"));
    }

    #[test]
    fn source_array_pattern_is_non_exhaustive() {
        let mut cache = ModuleCache::default();
        let r = check_source(
            "(defn f (xs : (Array number)) : number (match xs [(Array x) x]))",
            "test.lts",
            &mut cache,
        );
        assert!(r
            .diagnostics
            .iter()
            .any(|d| d.kind == DiagKind::NonExhaustive));
    }
}
