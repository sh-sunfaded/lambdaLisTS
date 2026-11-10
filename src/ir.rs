use std::rc::Rc;

use serde::{Deserialize, Serialize};

use crate::span::Span;
use crate::types::Type;
use crate::value::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Core {
    Lit { value: Value, span: Span },
    Var { name: Rc<str>, span: Span },
    Lam { params: Vec<Param>, ret: Type, body: Box<Core>, pure: bool, span: Span },
    App { f: Box<Core>, args: Vec<Core>, span: Span },
    Let { name: Rc<str>, rhs: Box<Core>, body: Box<Core>, span: Span },
    /// design/interp-capability.md §2.1: letrec。束縛対象は Lam のみ（check で検査）。
    /// body は末尾位置（§8.5 の Let.body と同じ扱い。SPEC 側の追記は別途報告のみ）。
    Letrec { name: Rc<str>, lam: Box<Core>, body: Box<Core>, span: Span },
    If { cond: Box<Core>, then: Box<Core>, else_: Box<Core>, span: Span },
    Match { scrutinee: Box<Core>, arms: Vec<MatchArm>, span: Span },
    Obj { fields: Vec<(Rc<str>, Core)>, span: Span },
    Field { obj: Box<Core>, name: Rc<str>, span: Span },
    EffectCall { host: Rc<str>, effect: Rc<str>, args: Vec<Core>, span: Span },
    Do { binds: Vec<DoBind>, body: Box<Core>, span: Span },
    Error { message: String, span: Span },
}

impl Core {
    pub fn span(&self) -> &Span {
        match self {
            Core::Lit { span, .. }
            | Core::Var { span, .. }
            | Core::Lam { span, .. }
            | Core::App { span, .. }
            | Core::Let { span, .. }
            | Core::Letrec { span, .. }
            | Core::If { span, .. }
            | Core::Match { span, .. }
            | Core::Obj { span, .. }
            | Core::Field { span, .. }
            | Core::EffectCall { span, .. }
            | Core::Do { span, .. }
            | Core::Error { span, .. } => span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: Rc<str>,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoBind {
    pub name: Rc<str>,
    pub expr: Core,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchArm {
    pub pattern: Pat,
    pub body: Core,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pat {
    pub kind: PatKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PatKind {
    Wildcard,
    Var(Rc<str>),
    Lit(Value),
    Obj(Vec<(Rc<str>, Pat)>),
    Array(Vec<Pat>),
    Or(Vec<Pat>),
}
