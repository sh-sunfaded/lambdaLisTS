use std::collections::BTreeSet;
use std::rc::Rc;

use serde_json::json;

use crate::ast::{Expr, Param, Pat, Program, TopForm, TypeExpr, TypeField};
use crate::diagnostic::{ast_path, top_form_path, Diagnostic, DiagKind, Patch, Severity};
use crate::span::Span;

/// §2.3: 値名前空間のシンボル名は kebab-case（MUST）。違反は「大文字を含む」または「`_` を含む」（`_` 単体はワイルドカード専用で除く）。
pub fn value_name_violation(name: &str) -> bool {
    if name == "_" {
        return false;
    }
    name.chars().any(|c| c.is_ascii_uppercase()) || name.contains('_')
}

/// 型名前空間（§6.6）は PascalCase が規範（付録A/B の型名の例も PascalCase）なので、大文字を含むこと自体は違反にしない。
/// snake_case（`_` を含む）と camelCase（小文字始まりで大文字を含む）は違反とする。
pub fn type_name_violation(name: &str) -> bool {
    if name == "_" {
        return false;
    }
    if name.contains('_') {
        return true;
    }
    let starts_lower = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase());
    starts_lower && name.chars().any(|c| c.is_ascii_uppercase())
}

/// camelCase / snake_case → kebab-case 変換（§9.3 の fix テンプレート用）。
pub fn to_kebab(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c == '_' {
            if !out.is_empty() && !out.ends_with('-') {
                out.push('-');
            }
        } else if c.is_ascii_uppercase() {
            if i > 0 && !out.is_empty() && !out.ends_with('-') {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// 検査フェーズ（AST 構築後）の name-convention チェック。パーサ回復（Error ノード）とは独立に動作する。
pub fn check(program: &Program, type_aliases: &BTreeSet<Rc<str>>) -> Vec<Diagnostic> {
    let mut w = Walker {
        diags: Vec::new(),
        type_aliases,
    };
    let mut example_index = 0usize;
    for form in &program.forms {
        let index = match form {
            TopForm::Example { .. } => {
                let i = example_index;
                example_index += 1;
                i
            }
            _ => 0,
        };
        w.top(form, index);
    }
    w.diags
}

struct Walker<'a> {
    diags: Vec<Diagnostic>,
    type_aliases: &'a BTreeSet<Rc<str>>,
}

impl<'a> Walker<'a> {
    fn report(&mut self, name: &str, span: &Span, path: String, value_ns: bool) {
        let violation = if value_ns {
            value_name_violation(name)
        } else {
            type_name_violation(name)
        };
        if !violation {
            return;
        }
        let kebab = to_kebab(name);
        let ns = if value_ns { "値" } else { "型" };
        self.diags.push(Diagnostic {
            kind: DiagKind::NameConvention,
            severity: Severity::Error,
            span: span.clone(),
            message: format!(
                "{ns}名前空間のシンボル名 '{name}' は kebab-case ではありません（§2.3）。修正候補: '{kebab}'"
            ),
            data: Some(json!({ "path": path })),
            fix: Some(Patch::Replace {
                path,
                template: kebab,
            }),
        });
    }

    fn top(&mut self, form: &TopForm, index: usize) {
        let base = top_form_path(form, index);
        match form {
            TopForm::Import { names, span, .. } | TopForm::Export { names, span, .. } => {
                for (i, n) in names.iter().enumerate() {
                    // §6.6: 型名前空間と値名前空間は分離され、値名は kebab-case（大文字不可）なので、
                    // 大文字を含む import/export 名は型名とみなす（import 失敗時にも誤報しないため）。
                    let value_ns = !self.type_aliases.contains(n)
                        && !n.chars().any(|c| c.is_ascii_uppercase());
                    self.report(n, span, ast_path(&base, &["names", &i.to_string()]), value_ns);
                }
            }
            TopForm::TypeDef { name, ty, span, .. } => {
                self.report(name, span, base.clone(), false);
                self.ty(ty, &ast_path(&base, &["type"]));
            }
            TopForm::EffectDef { name, ty, span, .. } => {
                self.report(name, span, base.clone(), true);
                self.ty(ty, &ast_path(&base, &["type"]));
            }
            TopForm::Defn {
                name,
                params,
                ret,
                body,
                span,
                ..
            } => {
                self.report(name, span, base.clone(), true);
                for (i, p) in params.iter().enumerate() {
                    self.param(p, &ast_path(&base, &["params", &i.to_string()]));
                }
                self.ty(ret, &ast_path(&base, &["ret"]));
                self.expr(body, &ast_path(&base, &["body"]));
            }
            TopForm::Prop {
                params, body, ..
            } => {
                for (i, p) in params.iter().enumerate() {
                    self.param(p, &ast_path(&base, &["params", &i.to_string()]));
                }
                self.expr(body, &ast_path(&base, &["body"]));
            }
            TopForm::Example {
                actual, expected, ..
            } => {
                self.expr(actual, &ast_path(&base, &["args", "0"]));
                self.expr(expected, &ast_path(&base, &["args", "2"]));
            }
            TopForm::Error { .. } => {}
        }
    }

    fn param(&mut self, p: &Param, path: &str) {
        self.report(&p.name, &p.span, path.to_string(), true);
        self.ty(&p.ty, &ast_path(path, &["type"]));
    }

    fn expr(&mut self, e: &Expr, path: &str) {
        match e {
            Expr::Lit { .. } | Expr::Sym { .. } | Expr::Error { .. } => {}
            Expr::App { args, .. } | Expr::Pipe { exprs: args, .. } => {
                for (i, a) in args.iter().enumerate() {
                    self.expr(a, &ast_path(path, &["args", &i.to_string()]));
                }
            }
            Expr::FieldAccess { parts, span, .. } => {
                for (i, p) in parts.iter().enumerate() {
                    self.report(p, span, ast_path(path, &["parts", &i.to_string()]), true);
                }
            }
            Expr::Let { binds, body, .. } => {
                for (i, b) in binds.iter().enumerate() {
                    let bp = ast_path(path, &["binds", &i.to_string()]);
                    self.report(&b.name, &b.span, bp.clone(), true);
                    self.expr(&b.expr, &ast_path(&bp, &["expr"]));
                }
                self.expr(body, &ast_path(path, &["body"]));
            }
            Expr::Letrec {
                name,
                lam,
                body,
                span,
                ..
            } => {
                self.report(name, span, ast_path(path, &["name"]), true);
                self.expr(lam, &ast_path(path, &["lam"]));
                for (i, e) in body.iter().enumerate() {
                    self.expr(e, &ast_path(path, &["body", &i.to_string()]));
                }
            }
            Expr::Do { binds, body, .. } => {
                for (i, b) in binds.iter().enumerate() {
                    let bp = ast_path(path, &["binds", &i.to_string()]);
                    self.report(&b.name, &b.span, bp.clone(), true);
                    self.expr(&b.expr, &ast_path(&bp, &["expr"]));
                }
                self.expr(body, &ast_path(path, &["body"]));
            }
            Expr::If {
                cond,
                then,
                else_,
                ..
            } => {
                self.expr(cond, &ast_path(path, &["cond"]));
                self.expr(then, &ast_path(path, &["then"]));
                self.expr(else_, &ast_path(path, &["else"]));
            }
            Expr::When { cond, body, .. } => {
                self.expr(cond, &ast_path(path, &["cond"]));
                self.expr(body, &ast_path(path, &["body"]));
            }
            Expr::Cond { arms, else_, .. } => {
                for (i, arm) in arms.iter().enumerate() {
                    let ap = ast_path(path, &["arms", &i.to_string()]);
                    self.expr(&arm.cond, &ast_path(&ap, &["cond"]));
                    self.expr(&arm.body, &ast_path(&ap, &["body"]));
                }
                self.expr(else_, &ast_path(path, &["else"]));
            }
            Expr::And { left, right, .. } | Expr::Or { left, right, .. } => {
                self.expr(left, &ast_path(path, &["left"]));
                self.expr(right, &ast_path(path, &["right"]));
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                self.expr(scrutinee, &ast_path(path, &["scrutinee"]));
                for (i, arm) in arms.iter().enumerate() {
                    let ap = ast_path(path, &["arms", &i.to_string()]);
                    self.pat(&arm.pattern, &ast_path(&ap, &["pattern"]));
                    self.expr(&arm.body, &ast_path(&ap, &["body"]));
                }
            }
            Expr::Obj { fields, .. } => {
                for (i, f) in fields.iter().enumerate() {
                    let fp = ast_path(path, &["fields", &i.to_string()]);
                    self.report(&f.name, &f.span, fp.clone(), true);
                    self.expr(&f.expr, &ast_path(&fp, &["expr"]));
                }
            }
            Expr::Fn {
                params,
                ret,
                body,
                ..
            } => {
                for (i, p) in params.iter().enumerate() {
                    self.param(p, &ast_path(path, &["params", &i.to_string()]));
                }
                self.ty(ret, &ast_path(path, &["ret"]));
                self.expr(body, &ast_path(path, &["body"]));
            }
        }
    }

    fn pat(&mut self, p: &Pat, path: &str) {
        match p {
            Pat::Wildcard { .. } | Pat::Lit { .. } => {}
            Pat::Var { name, span, .. } => self.report(name, span, path.to_string(), true),
            Pat::Obj { fields, .. } => {
                for (i, f) in fields.iter().enumerate() {
                    let fp = ast_path(path, &["fields", &i.to_string()]);
                    self.report(&f.name, &f.span, fp.clone(), true);
                    self.pat(&f.pattern, &ast_path(&fp, &["pattern"]));
                }
            }
            Pat::Array { elements, .. } => {
                for (i, e) in elements.iter().enumerate() {
                    self.pat(e, &ast_path(path, &["elements", &i.to_string()]));
                }
            }
            Pat::Or { patterns, .. } => {
                for (i, p) in patterns.iter().enumerate() {
                    self.pat(p, &ast_path(path, &["patterns", &i.to_string()]));
                }
            }
        }
    }

    fn ty(&mut self, t: &TypeExpr, path: &str) {
        match t {
            TypeExpr::Prim { .. } | TypeExpr::Error { .. } => {}
            TypeExpr::Alias { name, span, .. } => {
                self.report(name, span, path.to_string(), false)
            }
            TypeExpr::Array { element, .. } => self.ty(element, &ast_path(path, &["element"])),
            TypeExpr::Map { key, value, .. } => {
                self.ty(key, &ast_path(path, &["key"]));
                self.ty(value, &ast_path(path, &["value"]));
            }
            TypeExpr::Set { key, .. } => self.ty(key, &ast_path(path, &["key"])),
            TypeExpr::Ref { inner, .. } => self.ty(inner, &ast_path(path, &["inner"])),
            TypeExpr::Union { members, .. } => {
                for (i, m) in members.iter().enumerate() {
                    self.ty(m, &ast_path(path, &["members", &i.to_string()]));
                }
            }
            TypeExpr::Obj { fields, .. } => {
                for (i, f) in fields.iter().enumerate() {
                    let fp = ast_path(path, &["fields", &i.to_string()]);
                    match f {
                        TypeField::Field { name, ty, span, .. } => {
                            self.report(name, span, fp.clone(), true);
                            self.ty(ty, &ast_path(&fp, &["type"]));
                        }
                        TypeField::Tag { name, span, .. } => {
                            self.report(name, span, fp, true);
                        }
                    }
                }
            }
            TypeExpr::ResultT { ok, err, .. } => {
                self.ty(ok, &ast_path(path, &["ok"]));
                self.ty(err, &ast_path(path, &["err"]));
            }
            TypeExpr::FnT { params, ret, .. } => {
                for (i, p) in params.iter().enumerate() {
                    self.param(p, &ast_path(path, &["params", &i.to_string()]));
                }
                self.ty(ret, &ast_path(path, &["ret"]));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{check_source, new_module_cache};
    use crate::lexer::tokenize;
    use crate::parser::parse_program;

    fn diags_for(src: &str) -> Vec<Diagnostic> {
        let lex = tokenize(src, "test.lts");
        let (program, parse_diags) = parse_program(&lex);
        assert!(parse_diags.is_empty(), "{parse_diags:?}");
        check(&program, &BTreeSet::new())
    }

    #[test]
    fn value_rule_flags_camel_pascal_snake_allows_kebab() {
        assert!(value_name_violation("badName"));
        assert!(value_name_violation("BadName"));
        assert!(value_name_violation("bad_name"));
        assert!(!value_name_violation("_"));
        assert!(!value_name_violation("add-money"));
        assert!(!value_name_violation("contains?"));
        assert!(!value_name_violation("x2"));
        assert!(!value_name_violation("log"));
    }

    #[test]
    fn type_rule_allows_pascal_flags_camel_snake() {
        assert!(!type_name_violation("Money"));
        assert!(!type_name_violation("CartItem"));
        assert!(!type_name_violation("Array"));
        assert!(type_name_violation("badName"));
        assert!(type_name_violation("bad_name"));
        assert!(!type_name_violation("_"));
    }

    #[test]
    fn kebab_conversion() {
        assert_eq!(to_kebab("badName"), "bad-name");
        assert_eq!(to_kebab("bad_name"), "bad-name");
        assert_eq!(to_kebab("BadName"), "bad-name");
        assert_eq!(to_kebab("Bad_Type"), "bad-type");
        assert_eq!(to_kebab("add-money"), "add-money");
        assert_eq!(to_kebab("x2"), "x2");
    }

    #[test]
    fn repro_case_reports_export_and_defn_with_fix_template() {
        let src = "(export badName)\n(defn badName (x : number) : number x)\n";
        let diags = diags_for(src);
        assert_eq!(diags.len(), 2, "{diags:?}");
        assert!(diags
            .iter()
            .all(|d| d.kind == DiagKind::NameConvention && d.severity == Severity::Error));
        assert!(diags.iter().all(|d| matches!(
            &d.fix,
            Some(Patch::Replace { template, .. }) if template == "bad-name"
        )));
    }

    #[test]
    fn all_violation_positions_are_reported() {
        let src = "(type Bad_Type (obj (myField : number)))\n\
                   (effect myEffect : (fn () : number))\n\
                   (defn f (myParam : number) : number\n\
                   \x20 (let [badBind 1]\n\
                   \x20   (do [doBad 2]\n\
                   \x20     (match (obj (aField 1))\n\
                   \x20       [(obj (patField badVar)) badVar]\n\
                   \x20       [_ (fn (fnParam : number) : number fnParam)]))))\n\
                   (import \"./x.lts\" (badImport))\n\
                   (export badExport)\n";
        let diags = diags_for(src);
        let names: Vec<&str> = diags
            .iter()
            .map(|d| match &d.fix {
                Some(Patch::Replace { template, .. }) => template.as_str(),
                _ => "",
            })
            .collect();
        for want in [
            "bad-type",
            "my-field",
            "my-effect",
            "my-param",
            "bad-bind",
            "do-bad",
            "a-field",
            "pat-field",
            "bad-var",
            "fn-param",
            "bad-import",
            "bad-export",
        ] {
            assert!(names.contains(&want), "missing {want} in {names:?}");
        }
    }

    #[test]
    fn kebab_names_and_wildcard_pass_and_pascal_types_pass() {
        let src = "(type Money (obj (amount : number)))\n\
                   (export add-money contains? x2)\n\
                   (defn add-money (x : number) : number\n\
                   \x20 (match x [_ 0] [y y]))\n";
        let diags = diags_for(src);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn check_source_reports_name_convention_in_check_phase() {
        let mut cache = new_module_cache();
        let r = check_source("(defn badName (x : number) : number x)", "test.lts", &mut cache);
        assert!(r
            .diagnostics
            .iter()
            .any(|d| d.kind == DiagKind::NameConvention));
    }

    #[test]
    fn dotted_segments_are_checked() {
        let src = "(defn f (o : number) : number o.badField.x)\n";
        let diags = diags_for(src);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(matches!(
            &diags[0].fix,
            Some(Patch::Replace { template, .. }) if template == "bad-field"
        ));
    }
}
