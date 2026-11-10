use std::collections::BTreeMap;
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::ast::{Expr, LitValue, Pat, Prim, Program, TopForm, TypeExpr, TypeField};
use crate::diagnostic::{ast_path, top_form_path, AstPath, DiagKind, Diagnostic, Severity};
use crate::ir::{Core, DoBind, MatchArm, Param, Pat as CorePat, PatKind};
use crate::span::Span;
use crate::types::Type;
use crate::value::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CoreDef {
    Defn {
        name: Rc<str>,
        params: Vec<Param>,
        ret: Type,
        body: Core,
        pure: bool,
        span: Span,
        path: AstPath,
    },
    Type {
        name: Rc<str>,
        ty: Type,
        span: Span,
        path: AstPath,
    },
    Effect {
        name: Rc<str>,
        ty: Type,
        span: Span,
        path: AstPath,
    },
    Prop {
        name: Rc<str>,
        params: Vec<Param>,
        body: Core,
        span: Span,
        path: AstPath,
    },
    Example {
        actual: Core,
        expected: Core,
        span: Span,
        path: AstPath,
    },
    Import {
        path: Rc<str>,
        names: Vec<Rc<str>>,
        span: Span,
    },
    Export {
        names: Vec<Rc<str>>,
        span: Span,
    },
    Error {
        message: String,
        span: Span,
        path: AstPath,
    },
}

pub fn desugar(program: &Program) -> (Vec<CoreDef>, Vec<Diagnostic>) {
    Desugarer::default().run(program)
}

impl CoreDef {
    pub fn span(&self) -> &Span {
        match self {
            CoreDef::Defn { span, .. }
            | CoreDef::Type { span, .. }
            | CoreDef::Effect { span, .. }
            | CoreDef::Prop { span, .. }
            | CoreDef::Example { span, .. }
            | CoreDef::Import { span, .. }
            | CoreDef::Export { span, .. }
            | CoreDef::Error { span, .. } => span,
        }
    }
}

fn lit_value(v: &LitValue) -> Value {
    match v {
        LitValue::Num(n) => Value::Num(*n),
        LitValue::Dec(text) => Value::Dec(crate::decimal::parse_dec_scientific(text).unwrap_or(0)),
        LitValue::Str(s) => Value::Str(s.clone()),
        LitValue::Bool(b) => Value::Bool(*b),
        LitValue::Char(c) => Value::Char(*c),
        LitValue::Nil => Value::Nil,
    }
}

#[derive(Default)]
struct Desugarer {
    diagnostics: Vec<Diagnostic>,
    fresh: usize,
    host: Option<Rc<str>>,
}

impl Desugarer {
    fn run(mut self, program: &Program) -> (Vec<CoreDef>, Vec<Diagnostic>) {
        let mut defs = Vec::new();
        let mut prop_index = 0usize;
        let mut example_index = 0usize;
        for form in &program.forms {
            let index = match form {
                TopForm::Prop { .. } => {
                    let i = prop_index;
                    prop_index += 1;
                    i
                }
                TopForm::Example { .. } => {
                    let i = example_index;
                    example_index += 1;
                    i
                }
                _ => 0,
            };
            let path = top_form_path(form, index);
            defs.push(self.top_form(form, &path));
        }
        (defs, self.diagnostics)
    }

    fn next_fresh(&mut self, base: &str) -> String {
        let name = if self.fresh == 0 {
            base.to_string()
        } else {
            format!("{base}{}", self.fresh)
        };
        self.fresh += 1;
        name
    }

    fn error(&mut self, span: Span, message: &str, path: &str) {
        self.diagnostics.push(Diagnostic {
            kind: DiagKind::ParseError,
            severity: Severity::Error,
            span,
            message: message.to_string(),
            data: Some(json!({ "path": path })),
            fix: None,
        });
    }

    fn top_form(&mut self, form: &TopForm, path: &str) -> CoreDef {
        match form {
            TopForm::Import {
                path: p,
                names,
                span,
                ..
            } => CoreDef::Import {
                path: p.clone(),
                names: names.clone(),
                span: span.clone(),
            },
            TopForm::Export { names, span, .. } => CoreDef::Export {
                names: names.clone(),
                span: span.clone(),
            },
            TopForm::TypeDef {
                name, ty, span, ..
            } => CoreDef::Type {
                name: name.clone(),
                ty: self.type_(ty, &ast_path(path, &["type"])),
                span: span.clone(),
                path: path.to_string(),
            },
            TopForm::EffectDef {
                name, ty, span, ..
            } => CoreDef::Effect {
                name: name.clone(),
                ty: self.type_(ty, &ast_path(path, &["type"])),
                span: span.clone(),
                path: path.to_string(),
            },
            TopForm::Defn {
                effectful,
                name,
                params,
                ret,
                body,
                span,
                ..
            } => {
                let params_c: Vec<Param> = params
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        self.param(p, &ast_path(path, &["params", i.to_string().as_str()]))
                    })
                    .collect();
                let ret_c = self.type_(ret, &ast_path(path, &["ret"]));
                let saved_host = self.host.take();
                if *effectful {
                    self.host = params.first().map(|p| p.name.clone());
                }
                let body_c = self.expr(body, &ast_path(path, &["body"]));
                self.host = saved_host;
                CoreDef::Defn {
                    name: name.clone(),
                    params: params_c,
                    ret: ret_c,
                    body: body_c,
                    pure: !effectful,
                    span: span.clone(),
                    path: path.to_string(),
                }
            }
            TopForm::Prop {
                name,
                params,
                body,
                span,
                ..
            } => CoreDef::Prop {
                name: name.clone(),
                params: params
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        self.param(p, &ast_path(path, &["params", i.to_string().as_str()]))
                    })
                    .collect(),
                body: self.expr(body, &ast_path(path, &["body"])),
                span: span.clone(),
                path: path.to_string(),
            },
            TopForm::Example {
                actual,
                expected,
                span,
                ..
            } => CoreDef::Example {
                actual: self.expr(actual, &ast_path(path, &["args", "0"])),
                expected: self.expr(expected, &ast_path(path, &["args", "2"])),
                span: span.clone(),
                path: path.to_string(),
            },
            TopForm::Error {
                message, span, ..
            } => {
                self.error(span.clone(), message, path);
                CoreDef::Error {
                    message: message.clone(),
                    span: span.clone(),
                    path: path.to_string(),
                }
            }
        }
    }

    fn param(&mut self, p: &crate::ast::Param, path: &str) -> Param {
        Param {
            name: p.name.clone(),
            ty: self.type_(&p.ty, &ast_path(path, &["type"])),
        }
    }

    fn type_(&mut self, t: &TypeExpr, path: &str) -> Type {
        match t {
            TypeExpr::Prim { name, .. } => match name {
                Prim::Number => Type::Number,
                Prim::Dec => Type::Dec,
                Prim::Str => Type::Str,
                Prim::Bool => Type::Bool,
                Prim::Char => Type::Char,
                Prim::Builder => Type::Builder,
                Prim::Bytes => Type::Bytes,
                Prim::Nil => Type::Nil,
            },
            TypeExpr::Alias { name, .. } => Type::Alias(name.clone()),
            TypeExpr::Array { element, .. } => Type::Array(Box::new(self.type_(
                element,
                &ast_path(path, &["element"]),
            ))),
            TypeExpr::Union { members, .. } => Type::Union(
                members
                    .iter()
                    .enumerate()
                    .map(|(i, m)| {
                        self.type_(m, &ast_path(path, &["members", i.to_string().as_str()]))
                    })
                    .collect(),
            ),
            TypeExpr::Obj { fields, .. } => {
                let mut map: BTreeMap<Rc<str>, Type> = BTreeMap::new();
                for (i, f) in fields.iter().enumerate() {
                    match f {
                        TypeField::Field { name, ty, .. } => {
                            map.insert(
                                name.clone(),
                                self.type_(
                                    ty,
                                    &ast_path(path, &["fields", i.to_string().as_str(), "type"]),
                                ),
                            );
                        }
                        TypeField::Tag { name, value, .. } => {
                            map.insert(name.clone(), Type::Literal(value.clone()));
                        }
                    }
                }
                Type::Obj(map)
            }
            TypeExpr::Map { key, value, .. } => Type::Map(
                Box::new(self.type_(key, &ast_path(path, &["key"]))),
                Box::new(self.type_(value, &ast_path(path, &["value"]))),
            ),
            TypeExpr::Set { key, .. } => Type::Set(Box::new(self.type_(key, &ast_path(path, &["key"])))),
            TypeExpr::Ref { inner, .. } => {
                Type::Ref(Box::new(self.type_(inner, &ast_path(path, &["inner"]))))
            }
            TypeExpr::ResultT { ok, err, .. } => Type::ResultT(
                Box::new(self.type_(ok, &ast_path(path, &["ok"]))),
                Box::new(self.type_(err, &ast_path(path, &["err"]))),
            ),
            TypeExpr::FnT { params, ret, .. } => Type::FnT(
                params
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        self.type_(
                            &p.ty,
                            &ast_path(path, &["params", i.to_string().as_str(), "type"]),
                        )
                    })
                    .collect(),
                Box::new(self.type_(ret, &ast_path(path, &["ret"]))),
            ),
            TypeExpr::Error {
                message, span, ..
            } => {
                self.error(span.clone(), message, path);
                Type::Alias(Rc::from("<parse-error>"))
            }
        }
    }

    fn expr(&mut self, e: &Expr, path: &str) -> Core {
        match e {
            Expr::Lit { value, span, .. } => Core::Lit {
                value: lit_value(value),
                span: span.clone(),
            },
            Expr::Sym {
                name,
                effect,
                span,
                ..
            } => {
                if *effect {
                    Core::EffectCall {
                        host: self.host.clone().unwrap_or_default(),
                        effect: Rc::from(name.trim_end_matches('!')),
                        args: Vec::new(),
                        span: span.clone(),
                    }
                } else {
                    Core::Var {
                        name: name.clone(),
                        span: span.clone(),
                    }
                }
            }
            Expr::FieldAccess { parts, span, .. } => {
                let mut acc = Core::Var {
                    name: parts[0].clone(),
                    span: span.clone(),
                };
                for p in &parts[1..] {
                    acc = Core::Field {
                        obj: Box::new(acc),
                        name: p.clone(),
                        span: span.clone(),
                    };
                }
                acc
            }
            Expr::App {
                name,
                effect,
                args,
                span,
                ..
            } => {
                let args_c: Vec<Core> = args
                    .iter()
                    .enumerate()
                    .map(|(i, a)| self.expr(a, &ast_path(path, &["args", i.to_string().as_str()])))
                    .collect();
                if *effect {
                    Core::EffectCall {
                        host: self.host.clone().unwrap_or_default(),
                        effect: Rc::from(name.trim_end_matches('!')),
                        args: args_c,
                        span: span.clone(),
                    }
                } else {
                    Core::App {
                        f: Box::new(Core::Var {
                            name: name.clone(),
                            span: span.clone(),
                        }),
                        args: args_c,
                        span: span.clone(),
                    }
                }
            }
            Expr::Let {
                binds, body, span, ..
            } => {
                let mut acc = self.expr(body, &ast_path(path, &["body"]));
                for (i, b) in binds.iter().enumerate().rev() {
                    acc = Core::Let {
                        name: b.name.clone(),
                        rhs: Box::new(self.expr(
                            &b.expr,
                            &ast_path(path, &["binds", i.to_string().as_str()]),
                        )),
                        body: Box::new(acc),
                        span: span.clone(),
                    };
                }
                acc
            }
            Expr::Letrec {
                name,
                lam,
                body,
                span,
                ..
            } => {
                // body... は最後の式が値。それ以前は let _ の連鎖として脱糖する。
                let mut acc = self.expr(
                    body.last().expect("letrec body non-empty"),
                    &ast_path(path, &["body", &(body.len() - 1).to_string()]),
                );
                for (i, e) in body.iter().enumerate().rev().skip(1) {
                    acc = Core::Let {
                        name: Rc::from("_"),
                        rhs: Box::new(self.expr(e, &ast_path(path, &["body", &i.to_string()]))),
                        body: Box::new(acc),
                        span: span.clone(),
                    };
                }
                Core::Letrec {
                    name: name.clone(),
                    lam: Box::new(self.expr(lam, &ast_path(path, &["lam"]))),
                    body: Box::new(acc),
                    span: span.clone(),
                }
            }
            Expr::If {
                cond,
                then,
                else_,
                span,
                ..
            } => Core::If {
                cond: Box::new(self.expr(cond, &ast_path(path, &["cond"]))),
                then: Box::new(self.expr(then, &ast_path(path, &["then"]))),
                else_: Box::new(self.expr(else_, &ast_path(path, &["else"]))),
                span: span.clone(),
            },
            Expr::When {
                cond,
                body,
                span,
                ..
            } => Core::If {
                cond: Box::new(self.expr(cond, &ast_path(path, &["cond"]))),
                then: Box::new(self.expr(body, &ast_path(path, &["body"]))),
                else_: Box::new(Core::Lit {
                    value: Value::Nil,
                    span: span.clone(),
                }),
                span: span.clone(),
            },
            Expr::Cond {
                arms, else_, span, ..
            } => {
                let mut acc = self.expr(else_, &ast_path(path, &["arms", arms.len().to_string().as_str()]));
                for (i, arm) in arms.iter().enumerate().rev() {
                    acc = Core::If {
                        cond: Box::new(self.expr(
                            &arm.cond,
                            &ast_path(path, &["arms", i.to_string().as_str()]),
                        )),
                        then: Box::new(self.expr(
                            &arm.body,
                            &ast_path(path, &["arms", i.to_string().as_str()]),
                        )),
                        else_: Box::new(acc),
                        span: span.clone(),
                    };
                }
                acc
            }
            Expr::And {
                left,
                right,
                span,
                ..
            } => Core::If {
                cond: Box::new(self.expr(left, &ast_path(path, &["args", "0"]))),
                then: Box::new(self.expr(right, &ast_path(path, &["args", "1"]))),
                else_: Box::new(Core::Lit {
                    value: Value::Bool(false),
                    span: span.clone(),
                }),
                span: span.clone(),
            },
            Expr::Or {
                left,
                right,
                span,
                ..
            } => {
                let t = self.next_fresh("$t");
                let rhs = self.expr(left, &ast_path(path, &["args", "0"]));
                Core::Let {
                    name: Rc::from(t.as_str()),
                    rhs: Box::new(rhs),
                    body: Box::new(Core::If {
                        cond: Box::new(Core::Var {
                            name: Rc::from(t.as_str()),
                            span: span.clone(),
                        }),
                        then: Box::new(Core::Lit {
                            value: Value::Bool(true),
                            span: span.clone(),
                        }),
                        else_: Box::new(self.expr(right, &ast_path(path, &["args", "1"]))),
                        span: span.clone(),
                    }),
                    span: span.clone(),
                }
            }
            Expr::Match {
                scrutinee,
                arms,
                span,
                ..
            } => Core::Match {
                scrutinee: Box::new(self.expr(scrutinee, &ast_path(path, &["scrutinee"]))),
                arms: arms
                    .iter()
                    .enumerate()
                    .map(|(i, a)| MatchArm {
                        pattern: self.pattern(&a.pattern),
                        body: self.expr(&a.body, &ast_path(path, &["arms", i.to_string().as_str()])),
                    })
                    .collect(),
                span: span.clone(),
            },
            Expr::Do {
                binds, body, span, ..
            } => Core::Do {
                binds: binds
                    .iter()
                    .enumerate()
                    .map(|(i, b)| DoBind {
                        name: b.name.clone(),
                        expr: self.expr(
                            &b.expr,
                            &ast_path(path, &["binds", i.to_string().as_str()]),
                        ),
                    })
                    .collect(),
                body: Box::new(self.expr(body, &ast_path(path, &["body"]))),
                span: span.clone(),
            },
            Expr::Obj { fields, span, .. } => Core::Obj {
                fields: fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| {
                        (
                            f.name.clone(),
                            self.expr(&f.expr, &ast_path(path, &["fields", i.to_string().as_str()])),
                        )
                    })
                    .collect(),
                span: span.clone(),
            },
            Expr::Fn {
                params,
                ret,
                body,
                span,
                ..
            } => Core::Lam {
                params: params
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        self.param(p, &ast_path(path, &["params", i.to_string().as_str()]))
                    })
                    .collect(),
                ret: self.type_(ret, &ast_path(path, &["ret"])),
                body: Box::new(self.expr(body, &ast_path(path, &["body"]))),
                pure: true,
                span: span.clone(),
            },
            Expr::Pipe { exprs, span, .. } => {
                let mut acc = self.expr(&exprs[0], &ast_path(path, &["args", "0"]));
                for (i, e) in exprs.iter().enumerate().skip(1) {
                    acc = Core::App {
                        f: Box::new(self.expr(e, &ast_path(path, &["args", i.to_string().as_str()]))),
                        args: vec![acc],
                        span: span.clone(),
                    };
                }
                acc
            }
            Expr::Error {
                message, span, ..
            } => {
                self.error(span.clone(), message, path);
                Core::Error {
                    message: message.clone(),
                    span: span.clone(),
                }
            }
        }
    }

    fn pattern(&mut self, p: &Pat) -> CorePat {
        match p {
            Pat::Wildcard { span, .. } => CorePat {
                kind: PatKind::Wildcard,
                span: span.clone(),
            },
            Pat::Var { name, span, .. } => CorePat {
                kind: PatKind::Var(name.clone()),
                span: span.clone(),
            },
            Pat::Lit { value, span, .. } => CorePat {
                kind: PatKind::Lit(lit_value(value)),
                span: span.clone(),
            },
            Pat::Obj { fields, span, .. } => CorePat {
                kind: PatKind::Obj(
                    fields
                        .iter()
                        .map(|f| (f.name.clone(), self.pattern(&f.pattern)))
                        .collect(),
                ),
                span: span.clone(),
            },
            Pat::Array { elements, span, .. } => CorePat {
                kind: PatKind::Array(elements.iter().map(|e| self.pattern(e)).collect()),
                span: span.clone(),
            },
            Pat::Or { patterns, span, .. } => CorePat {
                kind: PatKind::Or(patterns.iter().map(|e| self.pattern(e)).collect()),
                span: span.clone(),
            },
        }
    }
}

pub fn desugar_expr(e: &crate::ast::Expr) -> Core {
    Desugarer::default().expr(e, "(expr)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{ast_path, top_form_path};
    use crate::lexer::tokenize;
    use crate::parser::parse_program;
    use serde_json::{json, Value as Json};

    fn desugar_src(src: &str) -> (Vec<CoreDef>, Vec<Diagnostic>) {
        let lex = tokenize(src, "test.lts");
        let (program, parse_diags) = parse_program(&lex);
        assert!(parse_diags.is_empty(), "{parse_diags:?}");
        desugar(&program)
    }

    fn desugar_fixture(name: &str) -> (Vec<CoreDef>, Vec<Diagnostic>) {
        let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
        let src = std::fs::read_to_string(&path).expect("fixture readable");
        let lex = tokenize(&src, name);
        let (program, _) = parse_program(&lex);
        desugar(&program)
    }

    fn body_of(src: &str) -> Core {
        let (defs, _) = desugar_src(src);
        match &defs[0] {
            CoreDef::Defn { body, .. } => body.clone(),
            other => panic!("not a defn: {other:?}"),
        }
    }

    fn strip(v: Json) -> Json {
        match v {
            Json::Object(m) => Json::Object(
                m.into_iter()
                    .filter(|(k, _)| k != "span" && k != "trivia" && k != "leading")
                    .map(|(k, v)| (k, strip(v)))
                    .collect(),
            ),
            Json::Array(a) => Json::Array(a.into_iter().map(strip).collect()),
            other => other,
        }
    }

    fn norm(core: &Core) -> Json {
        strip(serde_json::to_value(core).expect("serialize"))
    }

    fn norm_ty(t: &Type) -> Json {
        strip(serde_json::to_value(t).expect("serialize"))
    }

    fn lit(v: Value) -> Json {
        json!({ "Lit": { "value": serde_json::to_value(&v).expect("ser") } })
    }

    fn var(name: &str) -> Json {
        json!({ "Var": { "name": name } })
    }

    fn app(f: Json, args: Vec<Json>) -> Json {
        json!({ "App": { "f": f, "args": args } })
    }

    #[test]
    fn let_multiple_binds_nest_into_single_let() {
        let body = norm(&body_of("(defn f () : number (let [a 1 b 2] (+ a b)))"));
        assert_eq!(
            body,
            json!({ "Let": {
                "name": "a",
                "rhs": lit(Value::Num(1.0)),
                "body": { "Let": {
                    "name": "b",
                    "rhs": lit(Value::Num(2.0)),
                    "body": app(var("+"), vec![var("a"), var("b")])
                }}
            }})
        );
    }

    #[test]
    fn when_desugars_to_if_with_nil_else() {
        let body = norm(&body_of("(defn f (x : number) : number (when (< x 0) (- 0 x)))"));
        assert_eq!(
            body,
            json!({ "If": {
                "cond": app(var("<"), vec![var("x"), lit(Value::Num(0.0))]),
                "then": app(var("-"), vec![lit(Value::Num(0.0)), var("x")]),
                "else_": lit(Value::Nil)
            }})
        );
    }

    #[test]
    fn cond_desugars_to_nested_if_with_else_last() {
        let body = norm(&body_of(
            "(defn f (x : number) : number (cond [(< x 0) -1] [true 1] [:else 0]))",
        ));
        assert_eq!(
            body,
            json!({ "If": {
                "cond": app(var("<"), vec![var("x"), lit(Value::Num(0.0))]),
                "then": lit(Value::Num(-1.0)),
                "else_": { "If": {
                    "cond": lit(Value::Bool(true)),
                    "then": lit(Value::Num(1.0)),
                    "else_": lit(Value::Num(0.0))
                }}
            }})
        );
    }

    #[test]
    fn and_desugars_to_if_with_false_else() {
        let body = norm(&body_of("(defn f (a : bool) (b : bool) : bool (and a b))"));
        assert_eq!(
            body,
            json!({ "If": { "cond": var("a"), "then": var("b"), "else_": lit(Value::Bool(false)) } })
        );
    }

    #[test]
    fn or_desugars_to_let_with_fresh_name() {
        let body = norm(&body_of("(defn f (a : bool) (b : bool) : bool (or a b))"));
        assert_eq!(
            body,
            json!({ "Let": {
                "name": "$t",
                "rhs": var("a"),
                "body": { "If": { "cond": var("$t"), "then": lit(Value::Bool(true)), "else_": var("b") } }
            }})
        );
        let nested =
            norm(&body_of("(defn f (a : bool) (b : bool) (c : bool) : bool (or (or a b) c))"));
        assert_eq!(nested["Let"]["name"], json!("$t"));
        assert!(serde_json::to_string(&nested).expect("json").contains("$t1"));
    }

    #[test]
    fn pipe_desugars_to_nested_app() {
        let body = norm(&body_of("(defn f (x : number) : number (-> x g h))"));
        assert_eq!(body, app(var("h"), vec![app(var("g"), vec![var("x")])]));
        let single = norm(&body_of("(defn f (x : number) : number (-> x g))"));
        assert_eq!(single, app(var("g"), vec![var("x")]));
    }

    #[test]
    fn obj_construction_and_dotted_field_access() {
        let body = norm(&body_of("(defn f () : (obj (amount : number)) (obj (amount 0)))"));
        assert_eq!(
            body,
            json!({ "Obj": { "fields": [["amount", lit(Value::Num(0.0))]] } })
        );

        let body = norm(&body_of(
            "(defn f (i : (obj (price : (obj (amount : number))))) : number i.price.amount)",
        ));
        assert_eq!(
            body,
            json!({ "Field": {
                "obj": { "Field": { "obj": var("i"), "name": "price" } },
                "name": "amount"
            }})
        );
    }

    #[test]
    fn option_type_desugars_to_union_with_nil_and_tag_to_literal() {
        let (defs, _) = desugar_src("(type X (Option number))");
        let CoreDef::Type { ty, .. } = &defs[0] else {
            panic!()
        };
        assert_eq!(norm_ty(ty), json!({ "Union": ["Number", "Nil"] }));

        let (defs, _) = desugar_src(
            "(type E (union (obj (tag \"EmptyCart\")) (obj (tag \"NegativeQty\") (id : string))))",
        );
        let CoreDef::Type { ty, .. } = &defs[0] else {
            panic!()
        };
        assert_eq!(
            norm_ty(ty),
            json!({ "Union": [
                { "Obj": { "tag": { "Literal": "EmptyCart" } } },
                { "Obj": { "id": "Str", "tag": { "Literal": "NegativeQty" } } }
            ]})
        );
    }

    #[test]
    fn do_binds_keep_underscore_as_is() {
        let body = norm(&body_of("(defn f () : number (do [_ (g 1)] [t (h 2)] (+ t 1)))"));
        assert_eq!(
            body,
            json!({ "Do": {
                "binds": [
                    { "name": "_", "expr": app(var("g"), vec![lit(Value::Num(1.0))]) },
                    { "name": "t", "expr": app(var("h"), vec![lit(Value::Num(2.0))]) }
                ],
                "body": app(var("+"), vec![var("t"), lit(Value::Num(1.0))])
            }})
        );
    }

    #[test]
    fn fn_becomes_pure_lam_and_defn_bang_is_impure() {
        let body = norm(&body_of("(defn f () : number (fn (x : number) : number x))"));
        assert_eq!(
            body,
            json!({ "Lam": {
                "params": [{ "name": "x", "ty": "Number" }],
                "ret": "Number",
                "body": var("x"),
                "pure": true
            }})
        );
        let (defs, _) = desugar_src("(defn! f () : nil (ok nil))");
        let CoreDef::Defn { pure, body, .. } = &defs[0] else {
            panic!()
        };
        assert!(!pure);
        assert_eq!(norm(body), app(var("ok"), vec![lit(Value::Nil)]));
    }

    #[test]
    fn effect_call_desugars_to_effect_call() {
        let body = norm(&body_of("(defn f (x : number) : nil (log! (str x)))"));
        assert_eq!(
            body,
            json!({ "EffectCall": {
                "host": "",
                "effect": "log",
                "args": [app(var("str"), vec![var("x")])]
            }})
        );
        let body = norm(&body_of("(defn! f (host : (obj (log : (fn (s : string) : nil)))) (x : number) : nil (log! (str x)))"));
        assert_eq!(
            body,
            json!({ "EffectCall": {
                "host": "host",
                "effect": "log",
                "args": [app(var("str"), vec![var("x")])]
            }})
        );
    }

    #[test]
    fn match_desugars_patterns_to_core() {
        let body = norm(&body_of(
            "(defn f (e : (union (obj (tag \"A\")) (obj (tag \"B\") (x : number)))) : number (match e [(obj (tag \"A\")) 0] [(obj (tag \"B\") (x p)) p]))",
        ));
        assert_eq!(
            body,
            json!({ "Match": {
                "scrutinee": var("e"),
                "arms": [
                    { "pattern": { "kind": { "Obj": [["tag", { "kind": { "Lit": { "Str": "A" } } }]] } }, "body": lit(Value::Num(0.0)) },
                    { "pattern": { "kind": { "Obj": [["tag", { "kind": { "Lit": { "Str": "B" } } }], ["x", { "kind": { "Var": "p" } }]] } }, "body": var("p") }
                ]
            }})
        );
    }

    #[test]
    fn ast_paths_are_attached_and_built() {
        let lex = tokenize("(defn add-money (a : number) : number (+ a 1))", "test.lts");
        let (program, _) = parse_program(&lex);
        let base = top_form_path(&program.forms[0], 0);
        assert_eq!(base, "(defn add-money)");
        assert_eq!(
            ast_path(&base, &["body", "2", "then"]),
            "(defn add-money).body.2.then"
        );
        assert_eq!(ast_path(&base, &[] as &[&str]), "(defn add-money)");

        let (defs, _) = desugar(&program);
        let CoreDef::Defn { path, .. } = &defs[0] else {
            panic!()
        };
        assert_eq!(path, "(defn add-money)");
    }

    #[test]
    fn broken_lts_error_propagates_to_desugar_diagnostics() {
        let (defs, diags) = desugar_fixture("broken.lts");
        assert!(!diags.is_empty());
        assert!(diags.iter().all(|d| d.kind == DiagKind::ParseError));
        let broken = defs
            .iter()
            .find(|d| matches!(d, CoreDef::Defn { name, .. } if &**name == "broken"))
            .expect("broken defn");
        let CoreDef::Defn { body, .. } = broken else {
            panic!()
        };
        assert!(matches!(body, Core::Error { .. }));
        let ok = defs
            .iter()
            .find(|d| matches!(d, CoreDef::Defn { name, .. } if &**name == "ok"))
            .expect("ok defn");
        let CoreDef::Defn { body, .. } = ok else {
            panic!()
        };
        assert!(matches!(body, Core::App { .. }));
    }
}
