use std::collections::BTreeMap;
use std::rc::Rc;

use serde_json::json;

use crate::desugar::CoreDef;
use crate::diagnostic::{DiagKind, Diagnostic, Patch, Severity};
use crate::ir::Core;
use crate::span::Span;
use crate::types::{Scheme, Type};

/// design/interp-capability.md §1.5: ref は host 能力ではなく言語内完結の組み込みエフェクト。
/// effect 宣言なしで使える組み込みエフェクトのスキーム。
pub fn builtin_effect_scheme(name: &str) -> Option<Scheme> {
    let v = |id: usize| Type::Var(id);
    match name {
        // ref! : (fn (v) : (ref v))
        "ref" => Some(Scheme {
            vars: vec![1],
            body: Type::FnT(vec![v(1)], Box::new(Type::Ref(Box::new(v(1))))),
        }),
        // ref-get! : (fn ((ref v)) : v)
        "ref-get" => Some(Scheme {
            vars: vec![1],
            body: Type::FnT(vec![Type::Ref(Box::new(v(1)))], Box::new(v(1))),
        }),
        // ref-set! : (fn ((ref v) v) : unit) — 現 SPEC に unit は無いので nil（REPORT.md 記録）
        "ref-set" => Some(Scheme {
            vars: vec![1],
            body: Type::FnT(
                vec![Type::Ref(Box::new(v(1))), v(1)],
                Box::new(Type::Nil),
            ),
        }),
        // design §4: M20 host 能力の組み込みエフェクト（言語側で宣言済みとして扱う）。
        // now! : (fn () : num) — design の (-> unit num)。elapsed! も同じ。
        "now" => Some(Scheme {
            vars: vec![],
            body: Type::FnT(vec![], Box::new(Type::Number)),
        }),
        "elapsed" => Some(Scheme {
            vars: vec![],
            body: Type::FnT(vec![], Box::new(Type::Number)),
        }),
        // trace! : (fn (str) : unit) → nil。host-log の糖衣（design §4）。
        "trace" => Some(Scheme {
            vars: vec![],
            body: Type::FnT(vec![Type::Str], Box::new(Type::Nil)),
        }),
        // §4.1: read-file! : (fn (string) : (Result bytes (obj (tag "io-error"))))（M29 で bytes 実装時に有効化）
        "read-file" => Some(Scheme {
            vars: vec![],
            body: Type::FnT(
                vec![Type::Str],
                Box::new(Type::ResultT(
                    Box::new(Type::Bytes),
                    Box::new(Type::Obj(BTreeMap::from([(
                        Rc::from("tag"),
                        Type::Literal(Rc::from("io-error")),
                    )]))),
                )),
            ),
        }),
        _ => None,
    }
}

/// 組み込みエフェクト（ref 系 + host 能力 now/elapsed/trace/read-file）。effect 宣言不要・host ハンドラ不要。
pub fn is_builtin_effect(name: &str) -> bool {
    matches!(
        name,
        "ref" | "ref-get" | "ref-set" | "now" | "elapsed" | "trace" | "read-file"
    )
}

pub fn is_builtin_ref_effect(name: &str) -> bool {
    matches!(name, "ref" | "ref-get" | "ref-set")
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectCallSite {
    pub host: Rc<str>,
    pub effect: Rc<str>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DefnCallSite {
    pub callee: Rc<str>,
    pub span: Span,
}

pub fn find_effect_calls(e: &Core, out: &mut Vec<EffectCallSite>) {
    match e {
        Core::EffectCall {
            host,
            effect,
            args,
            span,
            ..
        } => {
            out.push(EffectCallSite {
                host: host.clone(),
                effect: effect.clone(),
                span: span.clone(),
            });
            for a in args {
                find_effect_calls(a, out);
            }
        }
        Core::Lam { body, .. } => find_effect_calls(body, out),
        Core::App { f, args, .. } => {
            find_effect_calls(f, out);
            for a in args {
                find_effect_calls(a, out);
            }
        }
        Core::Let { rhs, body, .. } => {
            find_effect_calls(rhs, out);
            find_effect_calls(body, out);
        }
        Core::Letrec { lam, body, .. } => {
            find_effect_calls(lam, out);
            find_effect_calls(body, out);
        }
        Core::If {
            cond,
            then,
            else_,
            ..
        } => {
            find_effect_calls(cond, out);
            find_effect_calls(then, out);
            find_effect_calls(else_, out);
        }
        Core::Match {
            scrutinee, arms, ..
        } => {
            find_effect_calls(scrutinee, out);
            for a in arms {
                find_effect_calls(&a.body, out);
            }
        }
        Core::Obj { fields, .. } => {
            for (_, c) in fields {
                find_effect_calls(c, out);
            }
        }
        Core::Field { obj, .. } => find_effect_calls(obj, out),
        Core::Do { binds, body, .. } => {
            for b in binds {
                find_effect_calls(&b.expr, out);
            }
            find_effect_calls(body, out);
        }
        _ => {}
    }
}

pub fn find_defn_calls(e: &Core, out: &mut Vec<DefnCallSite>) {
    match e {
        Core::App { f, args, span, .. } => {
            if let Core::Var { name, .. } = f.as_ref() {
                out.push(DefnCallSite {
                    callee: name.clone(),
                    span: span.clone(),
                });
            }
            find_defn_calls(f, out);
            for a in args {
                find_defn_calls(a, out);
            }
        }
        Core::Lam { body, .. } => find_defn_calls(body, out),
        Core::Let { rhs, body, .. } => {
            find_defn_calls(rhs, out);
            find_defn_calls(body, out);
        }
        Core::Letrec { lam, body, .. } => {
            find_defn_calls(lam, out);
            find_defn_calls(body, out);
        }
        Core::If {
            cond,
            then,
            else_,
            ..
        } => {
            find_defn_calls(cond, out);
            find_defn_calls(then, out);
            find_defn_calls(else_, out);
        }
        Core::Match {
            scrutinee, arms, ..
        } => {
            find_defn_calls(scrutinee, out);
            for a in arms {
                find_defn_calls(&a.body, out);
            }
        }
        Core::Obj { fields, .. } => {
            for (_, c) in fields {
                find_defn_calls(c, out);
            }
        }
        Core::Field { obj, .. } => find_defn_calls(obj, out),
        Core::EffectCall { args, .. } => {
            for a in args {
                find_defn_calls(a, out);
            }
        }
        Core::Do { binds, body, .. } => {
            for b in binds {
                find_defn_calls(&b.expr, out);
            }
            find_defn_calls(body, out);
        }
        _ => {}
    }
}

struct LocalDef<'a> {
    pure: bool,
    has_params: bool,
    body: &'a Core,
    path: String,
}

/// SPEC §7.2/§7.4 のエフェクト検査。`purity` は defn 名 -> 純粋かどうか（自モジュール＋import 先を含む）。
/// - 純粋 defn は eff = ∅ でなければならない（直接の EffectCall も defn! への呼び出しも不可）。
/// - エフェクト呼び出しを行う defn! は第1引数として host を受け取る必要がある（§12.6）。
/// - example 等 defn! の外でのエフェクト呼び出しは host が存在しないため不可。
pub fn check_effects(defs: &[CoreDef], purity: &BTreeMap<Rc<str>, bool>) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let mut pure_map: BTreeMap<Rc<str>, bool> = purity.clone();
    let mut locals: Vec<(Rc<str>, LocalDef)> = Vec::new();
    for d in defs {
        if let CoreDef::Defn {
            name,
            body,
            pure,
            path,
            params,
            ..
        } = d
        {
            pure_map.insert(name.clone(), *pure);
            locals.push((
                name.clone(),
                LocalDef {
                    pure: *pure,
                    has_params: !params.is_empty(),
                    body,
                    path: path.clone(),
                },
            ));
        }
    }
    // eff の不动点: defn! はエフェクト源。純粋 defn がエフェクトへ到達可能なら違反。
    let mut tainted: BTreeMap<Rc<str>, bool> = BTreeMap::new();
    for (name, l) in &locals {
        tainted.insert(name.clone(), !l.pure);
    }
    loop {
        let mut changed = false;
        for (name, l) in &locals {
            if tainted.get(name).copied().unwrap_or(false) {
                continue;
            }
            let mut sites = Vec::new();
            find_effect_calls(l.body, &mut sites);
            let mut calls = Vec::new();
            find_defn_calls(l.body, &mut calls);
            let reaches = !sites.is_empty()
                || calls.iter().any(|c| {
                    pure_map.get(&c.callee).copied() == Some(false)
                        || tainted.get(&c.callee).copied() == Some(true)
                });
            if reaches {
                tainted.insert(name.clone(), true);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for (name, l) in &locals {
        if l.pure && tainted.get(name).copied() == Some(true) {
            let mut sites = Vec::new();
            find_effect_calls(l.body, &mut sites);
            for site in &sites {
                diags.push(Diagnostic {
                    kind: DiagKind::EffectViolation,
                    severity: Severity::Error,
                    span: site.span.clone(),
                    message: format!(
                        "純粋関数 '{name}' >本体でエフェクト '{effect}!' を呼んでいます（defn! が必要です §7.4）",
                        effect = site.effect
                    ),
                    data: Some(json!({ "path": l.path, "effect": site.effect })),
                    fix: Some(Patch::Replace {
                        path: l.path.clone(),
                        template: "defn!".to_string(),
                    }),
                });
            }
            let mut calls = Vec::new();
            find_defn_calls(l.body, &mut calls);
            for c in &calls {
                let callee_impure = pure_map.get(&c.callee).copied() == Some(false);
                let callee_tainted = tainted.get(&c.callee).copied() == Some(true);
                if callee_impure || callee_tainted {
                    let message = if callee_impure {
                        format!(
                            "純粋関数 '{name}' が defn! '{callee}' を呼び出しています（defn から defn! への呼び出しは禁止です §7.4）",
                            callee = c.callee
                        )
                    } else {
                        format!(
                            "純粋関数 '{name}' がエフェクトを含む関数 '{callee}' を呼び出しています（defn! が必要です §7.2）",
                            callee = c.callee
                        )
                    };
                    diags.push(Diagnostic {
                        kind: DiagKind::EffectViolation,
                        severity: Severity::Error,
                        span: c.span.clone(),
                        message,
                        data: Some(json!({ "path": l.path, "callee": c.callee })),
                        fix: Some(Patch::Replace {
                            path: l.path.clone(),
                            template: "defn!".to_string(),
                        }),
                    });
                }
            }
        }
        if !l.pure {
            let mut sites = Vec::new();
            find_effect_calls(l.body, &mut sites);
            if !sites.is_empty() && !l.has_params {
                for site in &sites {
                    diags.push(Diagnostic {
                        kind: DiagKind::EffectViolation,
                        severity: Severity::Error,
                        span: site.span.clone(),
                        message: format!(
                            "defn! '{name}' はエフェクト '{effect}!' を呼ぶため第1引数として host を受け取る必要があります（§12.6）",
                            effect = site.effect
                        ),
                        data: Some(json!({ "path": l.path, "effect": site.effect })),
                        fix: None,
                    });
                }
            }
        }
    }
    for d in defs {
        // prop 本体も defn! の外なので host が無い（純粋であること）
        if let CoreDef::Prop { body, path, .. } = d {
            let mut sites = Vec::new();
            find_effect_calls(body, &mut sites);
            for site in &sites {
                diags.push(Diagnostic {
                    kind: DiagKind::EffectViolation,
                    severity: Severity::Error,
                    span: site.span.clone(),
                    message: format!(
                        "エフェクト呼び出し '{effect}!' は defn! 本体内でのみ可能です（host は defn! の第1引数として渡されます §12.6）",
                        effect = site.effect
                    ),
                    data: Some(json!({ "path": path, "effect": site.effect })),
                    fix: None,
                });
            }
            continue;
        }
        if let CoreDef::Example {
            actual,
            expected,
            path,
            ..
        } = d
        {
            for e in [actual, expected] {
                let mut sites = Vec::new();
                find_effect_calls(e, &mut sites);
                for site in &sites {
                    diags.push(Diagnostic {
                        kind: DiagKind::EffectViolation,
                        severity: Severity::Error,
                        span: site.span.clone(),
                        message: format!(
                            "エフェクト呼び出し '{effect}!' は defn! 本体内でのみ可能です（host は defn! の第1引数として渡されます §12.6）",
                            effect = site.effect
                        ),
                        data: Some(json!({ "path": path, "effect": site.effect })),
                        fix: None,
                    });
                }
            }
        }
    }
    diags
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{check_file, check_source, ModuleCache};
    use crate::desugar::desugar;
    use crate::lexer::tokenize;
    use crate::parser::parse_program;

    fn defs_of(src: &str) -> Vec<CoreDef> {
        let lex = tokenize(src, "test.lts");
        let (program, _) = parse_program(&lex);
        desugar(&program).0
    }

    fn check_src(src: &str) -> crate::check::ModuleResult {
        let mut cache = ModuleCache::default();
        check_source(src, "test.lts", &mut cache)
    }

    fn fixture_path(name: &str) -> String {
        format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
    }

    #[test]
    fn effects_lts_pure_run_log_is_violation_with_defn_bang_fix() {
        let mut cache = ModuleCache::default();
        let r = check_file(&fixture_path("effects.lts"), &mut cache);
        let d = r
            .diagnostics
            .iter()
            .find(|d| d.kind == DiagKind::EffectViolation)
            .expect("effect-violation");
        assert_eq!(d.severity, Severity::Error);
        assert!(d.message.contains("log!"));
        let data = d.data.as_ref().expect("data");
        assert_eq!(data["path"], "(defn pure-run)");
        assert_eq!(data["effect"], "log");
        assert!(matches!(&d.fix, Some(Patch::Replace { path, template })
            if path == "(defn pure-run)" && template == "defn!"));
    }

    #[test]
    fn defn_bang_body_effect_call_is_not_violation() {
        let r = check_src("(effect log : (fn (s : string) : nil))\n(defn! f (s : string) : nil (log! s))");
        assert!(
            !r.diagnostics
                .iter()
                .any(|d| d.kind == DiagKind::EffectViolation),
            "{:?}",
            r.diagnostics
        );
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    }

    #[test]
    fn undeclared_effect_call_is_unknown_effect() {
        let r = check_src("(effect log : (fn (s : string) : nil))\n(defn! f () : nil (db! \"x\"))");
        let d = r
            .diagnostics
            .iter()
            .find(|d| d.kind == DiagKind::UnknownEffect)
            .expect("unknown-effect");
        assert!(d.message.contains("db!"));
    }

    #[test]
    fn effect_call_args_checked_against_declaration() {
        let r = check_src("(effect log : (fn (s : string) : nil))\n(defn! f () : nil (log! 1))");
        assert!(r
            .diagnostics
            .iter()
            .any(|d| d.kind == DiagKind::TypeMismatch));
    }

    #[test]
    fn find_effect_calls_sees_nested_forms() {
        let defs = defs_of(
            "(defn f (x : number) : number (if (< x 0) (log! \"a\") (let [y (db! \"k\")] (+ x 1))))",
        );
        let CoreDef::Defn { body, .. } = &defs[0] else {
            panic!("not defn")
        };
        let mut sites = Vec::new();
        find_effect_calls(body, &mut sites);
        let mut names: Vec<&str> = sites.iter().map(|s| &*s.effect).collect();
        names.sort_unstable();
        assert_eq!(names, ["db", "log"]);
        let diags = check_effects(&defs, &BTreeMap::new());
        assert_eq!(diags.len(), 2);
        assert!(diags.iter().all(|d| d.kind == DiagKind::EffectViolation));
    }

    #[test]
    fn pure_defn_without_effect_calls_has_no_violation() {
        let r = check_src("(defn f (x : number) : number (+ x 1))");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    }

    #[test]
    fn pure_defn_calling_defn_bang_is_violation() {
        let r = check_src(
            "(effect log : (fn (s : string) : nil))\n(defn! g (host : (obj (log : (fn (s : string) : nil)))) : nil (log! \"x\"))\n(defn f (x : number) : nil (do [_ (g (obj (log (fn (s : string) : nil nil))))] nil))",
        );
        let d = r
            .diagnostics
            .iter()
            .find(|d| d.kind == DiagKind::EffectViolation)
            .expect("effect-violation");
        assert!(d.message.contains("defn! 'g'"), "{}", d.message);
        assert!(matches!(&d.fix, Some(Patch::Replace { path, template })
            if path == "(defn f)" && template == "defn!"));
    }

    #[test]
    fn pure_defn_reaching_effect_transitively_is_violation() {
        let src = "(effect log : (fn (s : string) : nil))\n(defn! k (host : (obj (log : (fn (s : string) : nil)))) : nil (log! \"x\"))\n(defn h (x : number) : (Result number nil) (do [_ (k (obj (log (fn (s : string) : nil nil))))] (ok x)))\n(defn f (x : number) : (Result number nil) (h x))";
        let r = check_src(src);
        let msgs: Vec<&str> = r
            .diagnostics
            .iter()
            .filter(|d| d.kind == DiagKind::EffectViolation)
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(msgs.len(), 2, "{:?}", r.diagnostics);
        assert!(msgs.iter().any(|m| m.contains("defn! 'k'")), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.contains("'h'")), "{msgs:?}");
    }

    #[test]
    fn defn_bang_without_params_and_effect_call_needs_host() {
        let r = check_src("(effect log : (fn (s : string) : nil))\n(defn! f () : nil (log! \"x\"))");
        assert!(r
            .diagnostics
            .iter()
            .any(|d| d.kind == DiagKind::EffectViolation && d.message.contains("host")),
            "{:?}",
            r.diagnostics);
    }

    #[test]
    fn effect_call_in_example_is_violation() {
        let r = check_src("(effect log : (fn (s : string) : nil))\n(example (log! \"x\") => nil)");
        assert!(r
            .diagnostics
            .iter()
            .any(|d| d.kind == DiagKind::EffectViolation && d.message.contains("defn!")),
            "{:?}",
            r.diagnostics);
    }

    #[test]
    fn defn_bang_may_call_pure_defn() {
        let r = check_src(
            "(effect log : (fn (s : string) : nil))\n(defn h (x : number) : number (+ x 1))\n(defn! f (host : (obj (log : (fn (s : string) : nil)))) (x : number) : (Result number nil) (do [_ (log! (str x))] (ok (h x))))",
        );
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    }
}
