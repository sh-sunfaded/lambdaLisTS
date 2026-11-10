use std::rc::Rc;

use serde::{Deserialize, Serialize};

use crate::ir::{Pat as CorePat, PatKind};
use crate::span::Span;
use crate::token::Trivia;
use crate::value::Value;

pub fn print_pattern(p: &CorePat) -> String {
    match &p.kind {
        PatKind::Wildcard => "_".to_string(),
        PatKind::Var(name) => name.to_string(),
        PatKind::Lit(v) => match v {
            Value::Nil => "nil".to_string(),
            Value::Str(s) => {
                serde_json::to_string(s.as_ref()).unwrap_or_else(|_| format!("\"{s}\""))
            }
            Value::Num(n) => format!("{n}"),
            Value::Bool(b) => format!("{b}"),
            _ => "nil".to_string(),
        },
        PatKind::Obj(fields) => format!(
            "(obj {})",
            fields
                .iter()
                .map(|(n, sub)| format!("({n} {})", print_pattern(sub)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        PatKind::Array(els) => format!(
            "(Array {})",
            els.iter().map(print_pattern).collect::<Vec<_>>().join(" ")
        ),
        PatKind::Or(ps) => format!(
            "(or {})",
            ps.iter().map(print_pattern).collect::<Vec<_>>().join(" ")
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LitValue {
    Num(f64),
    Dec(Rc<str>),
    Str(Rc<str>),
    Bool(bool),
    Char(char),
    Nil,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Prim {
    Number,
    Dec,
    Str,
    Bool,
    Char,
    Builder,
    Bytes,
    Nil,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub forms: Vec<TopForm>,
    pub trailing: Trivia,
    pub trivia: Trivia,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TopForm {
    Import {
        path: Rc<str>,
        names: Vec<Rc<str>>,
        trivia: Trivia,
        span: Span,
    },
    Export {
        names: Vec<Rc<str>>,
        trivia: Trivia,
        span: Span,
    },
    TypeDef {
        name: Rc<str>,
        ty: TypeExpr,
        trivia: Trivia,
        span: Span,
    },
    EffectDef {
        name: Rc<str>,
        ty: TypeExpr,
        trivia: Trivia,
        span: Span,
    },
    Defn {
        effectful: bool,
        name: Rc<str>,
        params: Vec<Param>,
        ret: TypeExpr,
        body: Box<Expr>,
        trivia: Trivia,
        span: Span,
    },
    Prop {
        name: Rc<str>,
        params: Vec<Param>,
        body: Box<Expr>,
        trivia: Trivia,
        span: Span,
    },
    Example {
        actual: Box<Expr>,
        expected: Box<Expr>,
        trivia: Trivia,
        span: Span,
    },
    Error {
        message: String,
        trivia: Trivia,
        span: Span,
    },
}

impl TopForm {
    pub fn span(&self) -> &Span {
        match self {
            TopForm::Import { span, .. }
            | TopForm::Export { span, .. }
            | TopForm::TypeDef { span, .. }
            | TopForm::EffectDef { span, .. }
            | TopForm::Defn { span, .. }
            | TopForm::Prop { span, .. }
            | TopForm::Example { span, .. }
            | TopForm::Error { span, .. } => span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: Rc<str>,
    pub ty: TypeExpr,
    pub trivia: Trivia,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TypeExpr {
    Prim {
        name: Prim,
        trivia: Trivia,
        span: Span,
    },
    Alias {
        name: Rc<str>,
        trivia: Trivia,
        span: Span,
    },
    Array {
        element: Box<TypeExpr>,
        trivia: Trivia,
        span: Span,
    },
    Union {
        members: Vec<TypeExpr>,
        trivia: Trivia,
        span: Span,
    },
    Obj {
        fields: Vec<TypeField>,
        trivia: Trivia,
        span: Span,
    },
    Map {
        key: Box<TypeExpr>,
        value: Box<TypeExpr>,
        trivia: Trivia,
        span: Span,
    },
    Set {
        key: Box<TypeExpr>,
        trivia: Trivia,
        span: Span,
    },
    Ref {
        inner: Box<TypeExpr>,
        trivia: Trivia,
        span: Span,
    },
    ResultT {
        ok: Box<TypeExpr>,
        err: Box<TypeExpr>,
        trivia: Trivia,
        span: Span,
    },
    FnT {
        params: Vec<Param>,
        ret: Box<TypeExpr>,
        trivia: Trivia,
        span: Span,
    },
    Error {
        message: String,
        trivia: Trivia,
        span: Span,
    },
}

impl TypeExpr {
    pub fn span(&self) -> &Span {
        match self {
            TypeExpr::Prim { span, .. }
            | TypeExpr::Alias { span, .. }
            | TypeExpr::Array { span, .. }
            | TypeExpr::Union { span, .. }
            | TypeExpr::Obj { span, .. }
            | TypeExpr::Map { span, .. }
            | TypeExpr::Set { span, .. }
            | TypeExpr::Ref { span, .. }
            | TypeExpr::ResultT { span, .. }
            | TypeExpr::FnT { span, .. }
            | TypeExpr::Error { span, .. } => span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TypeField {
    Field {
        name: Rc<str>,
        ty: TypeExpr,
        trivia: Trivia,
        span: Span,
    },
    Tag {
        name: Rc<str>,
        value: Rc<str>,
        trivia: Trivia,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    Lit {
        value: LitValue,
        trivia: Trivia,
        span: Span,
    },
    Sym {
        name: Rc<str>,
        effect: bool,
        trivia: Trivia,
        span: Span,
    },
    FieldAccess {
        parts: Vec<Rc<str>>,
        trivia: Trivia,
        span: Span,
    },
    App {
        name: Rc<str>,
        effect: bool,
        args: Vec<Expr>,
        trivia: Trivia,
        span: Span,
    },
    Let {
        binds: Vec<Bind>,
        body: Box<Expr>,
        trivia: Trivia,
        span: Span,
    },
    /// design/interp-capability.md §2.1: (letrec [name lam-expr] body...)
    Letrec {
        name: Rc<str>,
        lam: Box<Expr>,
        body: Vec<Expr>,
        trivia: Trivia,
        span: Span,
    },
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Box<Expr>,
        trivia: Trivia,
        span: Span,
    },
    When {
        cond: Box<Expr>,
        body: Box<Expr>,
        trivia: Trivia,
        span: Span,
    },
    Cond {
        arms: Vec<CondArm>,
        else_: Box<Expr>,
        trivia: Trivia,
        span: Span,
    },
    And {
        left: Box<Expr>,
        right: Box<Expr>,
        trivia: Trivia,
        span: Span,
    },
    Or {
        left: Box<Expr>,
        right: Box<Expr>,
        trivia: Trivia,
        span: Span,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        trivia: Trivia,
        span: Span,
    },
    Do {
        binds: Vec<DoBind>,
        body: Box<Expr>,
        trivia: Trivia,
        span: Span,
    },
    Obj {
        fields: Vec<ObjField>,
        trivia: Trivia,
        span: Span,
    },
    Fn {
        params: Vec<Param>,
        ret: Box<TypeExpr>,
        body: Box<Expr>,
        trivia: Trivia,
        span: Span,
    },
    Pipe {
        exprs: Vec<Expr>,
        trivia: Trivia,
        span: Span,
    },
    Error {
        message: String,
        trivia: Trivia,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> &Span {
        match self {
            Expr::Lit { span, .. }
            | Expr::Sym { span, .. }
            | Expr::FieldAccess { span, .. }
            | Expr::App { span, .. }
            | Expr::Let { span, .. }
            | Expr::Letrec { span, .. }
            | Expr::If { span, .. }
            | Expr::When { span, .. }
            | Expr::Cond { span, .. }
            | Expr::And { span, .. }
            | Expr::Or { span, .. }
            | Expr::Match { span, .. }
            | Expr::Do { span, .. }
            | Expr::Obj { span, .. }
            | Expr::Fn { span, .. }
            | Expr::Pipe { span, .. }
            | Expr::Error { span, .. } => span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bind {
    pub name: Rc<str>,
    pub expr: Box<Expr>,
    pub trivia: Trivia,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CondArm {
    pub cond: Box<Expr>,
    pub body: Box<Expr>,
    pub trivia: Trivia,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchArm {
    pub pattern: Pat,
    pub body: Box<Expr>,
    pub trivia: Trivia,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoBind {
    pub name: Rc<str>,
    pub expr: Box<Expr>,
    pub trivia: Trivia,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjField {
    pub name: Rc<str>,
    pub expr: Box<Expr>,
    pub trivia: Trivia,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Pat {
    Wildcard {
        trivia: Trivia,
        span: Span,
    },
    Var {
        name: Rc<str>,
        trivia: Trivia,
        span: Span,
    },
    Lit {
        value: LitValue,
        trivia: Trivia,
        span: Span,
    },
    Obj {
        fields: Vec<PatField>,
        trivia: Trivia,
        span: Span,
    },
    Array {
        elements: Vec<Pat>,
        trivia: Trivia,
        span: Span,
    },
    Or {
        patterns: Vec<Pat>,
        trivia: Trivia,
        span: Span,
    },
}

impl Pat {
    pub fn span(&self) -> &Span {
        match self {
            Pat::Wildcard { span, .. }
            | Pat::Var { span, .. }
            | Pat::Lit { span, .. }
            | Pat::Obj { span, .. }
            | Pat::Array { span, .. }
            | Pat::Or { span, .. } => span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatField {
    pub name: Rc<str>,
    pub pattern: Box<Pat>,
    pub trivia: Trivia,
    pub span: Span,
}
