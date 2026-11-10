use std::rc::Rc;

use serde_json::json;

use crate::ast::{
    Bind, CondArm, DoBind, Expr, LitValue, MatchArm, ObjField, Param, Pat, PatField, Prim, Program,
    TopForm, TypeExpr, TypeField,
};
use crate::diagnostic::{DiagKind, Diagnostic, Patch, Severity};
use crate::lexer::{tokenize, LexResult};
use crate::span::Span;
use crate::token::{Token, TokenKind, TokenValue, Trivia};

const TOP_HEADS: &[&str] = &[
    "defn", "defn!", "type", "effect", "import", "export", "prop", "example",
];
const SPECIAL_HEADS: &[&str] = &[
    "let", "letrec", "if", "when", "cond", "and", "or", "match", "do", "obj", "fn",
];
const RESERVED: &[&str] = &[
    "defn", "defn!", "fn", "let", "if", "when", "cond", "and", "or", "match", "do", "obj",
    "type", "effect", "prop", "forall", "example", "import", "export", "Array", "union",
    "Result", "Option", "letrec",
];
const PRIMS: &[&str] = &["number", "string", "bool", "decimal", "char", "builder", "bytes"];
const OPERATORS: &[&str] = &["+", "-", "*", "/", "<", ">", "<=", ">=", "="];

fn is_reserved(text: &str) -> bool {
    RESERVED.contains(&text)
}

enum Raw<'a> {
    Atom(&'a Token),
    List(ListRaw<'a>),
    Error(&'a Token),
}

struct ListRaw<'a> {
    items: Vec<Raw<'a>>,
    close: Option<&'a Token>,
    span: Span,
    trivia: Trivia,
    /// `[` で開いたか（do の束縛は `[...]` のみ。`(...)` は式）
    bracket: bool,
}

fn as_atom<'a>(r: &Raw<'a>) -> Option<&'a Token> {
    match r {
        Raw::Atom(t) => Some(*t),
        _ => None,
    }
}

fn as_list<'a, 'b>(r: &'b Raw<'a>) -> Option<&'b ListRaw<'a>> {
    match r {
        Raw::List(l) => Some(l),
        _ => None,
    }
}

fn is_symbol_tok(t: &Token) -> bool {
    t.kind == TokenKind::Symbol
}

fn is_punct_tok(t: &Token, s: &str) -> bool {
    t.kind == TokenKind::Punct && &*t.text == s
}

fn type_error(message: &str, span: Span, trivia: Trivia) -> TypeExpr {
    TypeExpr::Error {
        message: message.to_string(),
        trivia,
        span,
    }
}

fn expr_error(message: &str, span: Span, trivia: Trivia) -> Expr {
    Expr::Error {
        message: message.to_string(),
        trivia,
        span,
    }
}

fn err_node(message: &str, raw: &ListRaw) -> Expr {
    expr_error(message, raw.span.clone(), raw.trivia.clone())
}

pub fn parse_program(lex: &LexResult) -> (Program, Vec<Diagnostic>) {
    let mut parser = Parser::new(&lex.tokens);
    let raws = parser.read_all();
    let mut counters = Counters { prop: 0, example: 0 };
    let forms: Vec<TopForm> = raws
        .into_iter()
        .map(|r| parser.build_top_form(r, &mut counters))
        .collect();
    let span = match (lex.tokens.first(), lex.tokens.last()) {
        (Some(f), Some(l)) => Span {
            file: f.span.file.clone(),
            start: f.span.start,
            end: l.span.end,
            line: f.span.line,
            col: f.span.col,
        },
        _ => Span {
            file: Rc::from(""),
            start: 0,
            end: 0,
            line: 1,
            col: 1,
        },
    };
    let program = Program {
        forms,
        trailing: lex.trailing.clone(),
        trivia: Trivia::default(),
        span,
    };
    let mut diagnostics = lex.diagnostics.clone();
    diagnostics.extend(parser.diagnostics);
    (program, diagnostics)
}

pub fn parse_tokens(tokens: &[Token]) -> (Program, Vec<Diagnostic>) {
    let lex = LexResult {
        tokens: tokens.to_vec(),
        diagnostics: Vec::new(),
        trailing: Trivia::default(),
    };
    parse_program(&lex)
}

struct Counters {
    prop: usize,
    example: usize,
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Parser {
            tokens,
            pos: 0,
            diagnostics: Vec::new(),
        }
    }

    fn peek(&self) -> Option<&'a Token> {
        self.tokens.get(self.pos)
    }

    fn peek_n(&self, n: usize) -> Option<&'a Token> {
        self.tokens.get(self.pos + n)
    }

    fn advance(&mut self) {
        self.pos += 1;
    }

    fn error(
        &mut self,
        span: Span,
        message: String,
        data: Option<serde_json::Value>,
        fix: Option<Patch>,
    ) {
        self.diagnostics.push(Diagnostic {
            kind: DiagKind::ParseError,
            severity: Severity::Error,
            span,
            message,
            data,
            fix,
        });
    }

    fn raw_span_of(&self, r: &Raw) -> Span {
        match r {
            Raw::List(l) => l.span.clone(),
            Raw::Atom(t) | Raw::Error(t) => t.span.clone(),
        }
    }

    fn join_span(a: &Span, b: &Span) -> Span {
        Span {
            file: a.file.clone(),
            start: a.start,
            end: b.end,
            line: a.line,
            col: a.col,
        }
    }

    fn read_all(&mut self) -> Vec<Raw<'a>> {
        let mut raws = Vec::new();
        while let Some(t) = self.peek() {
            raws.push(self.read_raw(t));
        }
        raws
    }

    fn is_top_head(&self, t: Option<&'a Token>) -> bool {
        t.is_some_and(|t| is_symbol_tok(t) && TOP_HEADS.contains(&&*t.text))
    }

    fn read_raw(&mut self, t: &'a Token) -> Raw<'a> {
        match t.kind {
            TokenKind::Error => {
                self.advance();
                Raw::Error(t)
            }
            TokenKind::OpenBrace => {
                self.error(
                    t.span.clone(),
                    "'{' は予約されており P0 では使用できません (§3.2)".to_string(),
                    None,
                    None,
                );
                let mut d = 0usize;
                loop {
                    match self.peek() {
                        None => break,
                        Some(u) => {
                            self.advance();
                            if u.kind == TokenKind::OpenBrace {
                                d += 1;
                            }
                            if u.kind == TokenKind::CloseBrace {
                                if d == 0 {
                                    break;
                                }
                                d -= 1;
                            }
                        }
                    }
                }
                Raw::Error(t)
            }
            TokenKind::CloseBrace => {
                self.error(t.span.clone(), "対応する '{' がありません".to_string(), None, None);
                self.advance();
                Raw::Error(t)
            }
            TokenKind::OpenParen | TokenKind::OpenBracket => self.read_list(t),
            TokenKind::CloseParen | TokenKind::CloseBracket => {
                self.error(
                    t.span.clone(),
                    format!("対応する開き括弧がありません: '{}'", t.text),
                    None,
                    None,
                );
                self.advance();
                Raw::Error(t)
            }
            _ => {
                self.advance();
                Raw::Atom(t)
            }
        }
    }

    fn read_list(&mut self, open: &'a Token) -> Raw<'a> {
        self.advance();
        let mut items: Vec<Raw<'a>> = Vec::new();
        let mut close: Option<&'a Token> = None;
        loop {
            let t = match self.peek() {
                None => {
                    self.error(
                        open.span.clone(),
                        format!("括弧が閉じていません: '{}' が閉じられていません", open.text),
                        None,
                        None,
                    );
                    break;
                }
                Some(t) => t,
            };
            if matches!(t.kind, TokenKind::CloseParen | TokenKind::CloseBracket) {
                close = Some(t);
                self.advance();
                break;
            }
            if t.kind == TokenKind::OpenParen && self.is_top_head(self.peek_n(1)) {
                self.error(
                    open.span.clone(),
                    "括弧が閉じていません: 次のトップレベルフォームの直前で強制クローズしました (§9.4)"
                        .to_string(),
                    None,
                    None,
                );
                break;
            }
            items.push(self.read_raw(t));
        }
        let end = match close {
            Some(c) => c.span.end,
            None => match items.last() {
                Some(r) => self.raw_span_of(r).end,
                None => open.span.end,
            },
        };
        let span = Span {
            file: open.span.file.clone(),
            start: open.span.start,
            end,
            line: open.span.line,
            col: open.span.col,
        };
        Raw::List(ListRaw {
            items,
            close,
            span,
            trivia: open.trivia.clone(),
            bracket: open.kind == TokenKind::OpenBracket,
        })
    }

    fn peek_name(&self, args: &[Raw]) -> String {
        match args.first().and_then(as_atom) {
            Some(t) if is_symbol_tok(t) => t.text.to_string(),
            _ => "?".to_string(),
        }
    }

    fn build_top_form(&mut self, raw: Raw<'a>, counters: &mut Counters) -> TopForm {
        match raw {
            Raw::Error(t) => TopForm::Error {
                message: "不正なトークンです".to_string(),
                trivia: t.trivia.clone(),
                span: t.span.clone(),
            },
            Raw::Atom(t) => {
                self.error(
                    t.span.clone(),
                    "トップレベルには (...) >フォームが必要です".to_string(),
                    None,
                    None,
                );
                TopForm::Error {
                    message: "フォームではありません".to_string(),
                    trivia: t.trivia.clone(),
                    span: t.span.clone(),
                }
            }
            Raw::List(l) => {
                if l.items.is_empty() {
                    self.error(l.span.clone(), "空のフォームです".to_string(), None, None);
                    return TopForm::Error {
                        message: "空のフォームです".to_string(),
                        trivia: l.trivia.clone(),
                        span: l.span.clone(),
                    };
                }
                let head = &l.items[0];
                let args = &l.items[1..];
                let head_tok = match as_atom(head) {
                    Some(t) if is_symbol_tok(t) => t,
                    _ => {
                        self.error(
                            l.span.clone(),
                            "トップレベルフォームの先頭はシンボルでなければなりません".to_string(),
                            None,
                            None,
                        );
                        return TopForm::Error {
                            message: "先頭がシンボルではありません".to_string(),
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        };
                    }
                };
                let name = head_tok.text.clone();
                match &*name {
                    "import" => self.build_import(&l, args),
                    "export" => self.build_export(&l, args),
                    "type" => self.build_type_def(&l, args),
                    "effect" => self.build_effect_def(&l, args),
                    "defn" | "defn!" => {
                        let path = format!("(defn {})", self.peek_name(args));
                        self.build_defn(&l, head_tok, args, &path)
                    }
                    "prop" => {
                        let path = format!("(prop {})", counters.prop);
                        counters.prop += 1;
                        self.build_prop(&l, args, &path)
                    }
                    "example" => {
                        let path = format!("(example {})", counters.example);
                        counters.example += 1;
                        self.build_example(&l, args, &path)
                    }
                    _ => {
                        self.error(
                            head_tok.span.clone(),
                            format!("不明なトップレベルフォームです: '{}'", name),
                            None,
                            None,
                        );
                        TopForm::Error {
                            message: format!("不明なフォーム: {}", name),
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        }
                    }
                }
            }
        }
    }

    fn build_import(&mut self, raw: &ListRaw<'a>, args: &[Raw<'a>]) -> TopForm {
        let mut path: Rc<str> = Rc::from("");
        match args.first().and_then(as_atom) {
            Some(t) if t.kind == TokenKind::Str => {
                if let Some(TokenValue::Str(s)) = &t.value {
                    path = Rc::from(s.as_str());
                }
            }
            _ => {
                self.error(
                    raw.span.clone(),
                    "import は (import \"path\" (name*)) >形です".to_string(),
                    None,
                    None,
                );
            }
        }
        let mut names: Vec<Rc<str>> = Vec::new();
        match args.get(1) {
            Some(Raw::List(names_raw)) => {
                for item in &names_raw.items {
                    match as_atom(item) {
                        Some(t) if is_symbol_tok(t) => names.push(t.text.clone()),
                        Some(t) if is_punct_tok(t, "*") => {
                            self.error(
                                t.span.clone(),
                                "import >ワイルドカード '*' は禁止されています (§3.8)".to_string(),
                                None,
                                None,
                            );
                        }
                        _ => {
                            self.error(
                                self.raw_span_of(item),
                                "import >名前はシンボルでなければなりません".to_string(),
                                None,
                                None,
                            );
                        }
                    }
                }
            }
            _ => {
                self.error(
                    raw.span.clone(),
                    "import は (import \"path\" (name*)) >形です".to_string(),
                    None,
                    None,
                );
            }
        }
        if args.len() != 2 {
            self.error(raw.span.clone(), "import >引数が不正です".to_string(), None, None);
        }
        TopForm::Import {
            path,
            names,
            trivia: raw.trivia.clone(),
            span: raw.span.clone(),
        }
    }

    fn build_export(&mut self, raw: &ListRaw<'a>, args: &[Raw<'a>]) -> TopForm {
        let mut names: Vec<Rc<str>> = Vec::new();
        for item in args {
            match as_atom(item) {
                Some(t) if is_symbol_tok(t) => {
                    if is_reserved(&t.text) {
                        self.error(
                            t.span.clone(),
                            format!("予約語 '{}' は export できません", t.text),
                            None,
                            None,
                        );
                    }
                    names.push(t.text.clone());
                }
                _ => {
                    self.error(
                        self.raw_span_of(item),
                        "export >引数はシンボルでなければなりません".to_string(),
                        None,
                        None,
                    );
                }
            }
        }
        TopForm::Export {
            names,
            trivia: raw.trivia.clone(),
            span: raw.span.clone(),
        }
    }

    fn build_type_def(&mut self, raw: &ListRaw<'a>, args: &[Raw<'a>]) -> TopForm {
        let name = self.peek_name(args);
        match args.first().and_then(as_atom) {
            Some(t) if is_symbol_tok(t) => {
                if args.len() != 2 {
                    self.error(
                        raw.span.clone(),
                        "type は (type Name TypeExpr) >形です".to_string(),
                        None,
                        None,
                    );
                }
                if is_reserved(&t.text) {
                    self.error(
                        t.span.clone(),
                        format!("予約語 '{}' は型名に使えません", t.text),
                        None,
                        None,
                    );
                }
            }
            _ => {
                self.error(
                    raw.span.clone(),
                    "type は (type Name TypeExpr) >形です".to_string(),
                    None,
                    None,
                );
            }
        }
        let ty = match args.get(1) {
            Some(t) => self.build_type(t, "(type).type"),
            None => type_error("型がありません", raw.span.clone(), raw.trivia.clone()),
        };
        TopForm::TypeDef {
            name: Rc::from(name.as_str()),
            ty,
            trivia: raw.trivia.clone(),
            span: raw.span.clone(),
        }
    }

    fn build_effect_def(&mut self, raw: &ListRaw<'a>, args: &[Raw<'a>]) -> TopForm {
        let name = match args.first().and_then(as_atom) {
            Some(t) if is_symbol_tok(t) => t.text.clone(),
            _ => Rc::from(""),
        };
        if args.len() != 3 || name.is_empty() {
            self.error(
                raw.span.clone(),
                "effect は (effect name : TypeExpr) >形です".to_string(),
                None,
                None,
            );
        }
        if !args.get(1).and_then(as_atom).is_some_and(|t| is_punct_tok(t, ":")) {
            self.error(raw.span.clone(), "effect には ':' が必要です".to_string(), None, None);
        }
        let ty = match args.get(2) {
            Some(t) => self.build_type(t, "(effect).type"),
            None => type_error("型がありません", raw.span.clone(), raw.trivia.clone()),
        };
        TopForm::EffectDef {
            name,
            ty,
            trivia: raw.trivia.clone(),
            span: raw.span.clone(),
        }
    }

    fn build_defn(&mut self, raw: &ListRaw<'a>, head: &'a Token, args: &[Raw<'a>], path: &str) -> TopForm {
        let effectful = &*head.text == "defn!";
        let mut i = 0;
        let mut name: Rc<str> = Rc::from("");
        match args.first().and_then(as_atom) {
            Some(t) if is_symbol_tok(t) => {
                name = t.text.clone();
                if is_reserved(&t.text) {
                    self.error(
                        t.span.clone(),
                        format!("予約語 '{}' は定義名に使えません", t.text),
                        None,
                        None,
                    );
                }
                i = 1;
            }
            _ => {
                self.error(raw.span.clone(), "defn には名前が必要です".to_string(), None, None);
            }
        }
        let mut params: Vec<Param> = Vec::new();
        while let Some(r @ Raw::List(l)) = args.get(i) {
            if l.items.is_empty() {
                i += 1;
                continue;
            }
            params.push(self.build_param(r, &format!("{path}.params.{}", params.len())));
            i += 1;
        }
        if !args.get(i).and_then(as_atom).is_some_and(|t| is_punct_tok(t, ":")) {
            self.error(
                raw.span.clone(),
                "defn には ':' と戻り値の型が必要です".to_string(),
                Some(json!({ "expected": "':' Type body", "actual": "missing" })),
                None,
            );
            return TopForm::Defn {
                effectful,
                name,
                params,
                ret: type_error("':' と型がありません", raw.span.clone(), raw.trivia.clone()),
                body: Box::new(expr_error("本体がありません", raw.span.clone(), raw.trivia.clone())),
                trivia: raw.trivia.clone(),
                span: raw.span.clone(),
            };
        }
        i += 1;
        if i >= args.len() {
            self.error(raw.span.clone(), "defn には戻り値の型が必要です".to_string(), None, None);
            return TopForm::Defn {
                effectful,
                name,
                params,
                ret: type_error("型がありません", raw.span.clone(), raw.trivia.clone()),
                body: Box::new(expr_error("本体がありません", raw.span.clone(), raw.trivia.clone())),
                trivia: raw.trivia.clone(),
                span: raw.span.clone(),
            };
        }
        let ret = self.build_type(&args[i], &format!("{path}.ret"));
        i += 1;
        if i >= args.len() {
            self.error(raw.span.clone(), "defn には本体が必要です".to_string(), None, None);
            return TopForm::Defn {
                effectful,
                name,
                params,
                ret,
                body: Box::new(expr_error("本体がありません", raw.span.clone(), raw.trivia.clone())),
                trivia: raw.trivia.clone(),
                span: raw.span.clone(),
            };
        }
        let body = Box::new(self.build_expr(&args[i], &format!("{path}.body")));
        i += 1;
        if i < args.len() {
            self.error(raw.span.clone(), "defn >引数が多すぎます".to_string(), None, None);
        }
        TopForm::Defn {
            effectful,
            name,
            params,
            ret,
            body,
            trivia: raw.trivia.clone(),
            span: raw.span.clone(),
        }
    }

    fn build_prop(&mut self, raw: &ListRaw<'a>, args: &[Raw<'a>], path: &str) -> TopForm {
        let mut name: Rc<str> = Rc::from("");
        match args.first().and_then(as_atom) {
            Some(t) if t.kind == TokenKind::Str => {
                if let Some(TokenValue::Str(s)) = &t.value {
                    name = Rc::from(s.as_str());
                }
            }
            _ => {
                self.error(
                    raw.span.clone(),
                    "prop は (prop \"name\" (forall [param*] expr)) >形です".to_string(),
                    None,
                    None,
                );
            }
        }
        let mut params: Vec<Param> = Vec::new();
        let mut body = expr_error("本体がありません", raw.span.clone(), raw.trivia.clone());
        let forall_ok = match args.get(1) {
            Some(Raw::List(fa)) => {
                !fa.items.is_empty()
                    && matches!(&fa.items[0], Raw::Atom(t) if is_symbol_tok(t) && &*t.text == "forall")
            }
            _ => false,
        };
        if forall_ok {
            if let Some(fa) = as_list(&args[1]) {
                let fa_args = &fa.items[1..];
                match fa_args.first() {
                    Some(Raw::List(quant)) => {
                        for item in &quant.items {
                            if matches!(item, Raw::List(l) if l.items.is_empty()) {
                                continue;
                            }
                            params.push(
                                self.build_param(item, &format!("{path}.params.{}", params.len())),
                            );
                        }
                    }
                    _ => {
                        self.error(
                            raw.span.clone(),
                            "forall には [param*] >量化子ベクタが必要です".to_string(),
                            None,
                            None,
                        );
                    }
                }
                match fa_args.get(1) {
                    Some(b) => body = self.build_expr(b, &format!("{path}.body")),
                    None => {
                        self.error(
                            raw.span.clone(),
                            "forall には本体が必要です".to_string(),
                            None,
                            None,
                        );
                    }
                }
            }
        } else {
            self.error(
                raw.span.clone(),
                "prop >第2引数は (forall [param*] expr) でなければなりません".to_string(),
                None,
                None,
            );
        }
        if args.len() != 2 {
            self.error(raw.span.clone(), "prop >引数が不正です".to_string(), None, None);
        }
        TopForm::Prop {
            name,
            params,
            body: Box::new(body),
            trivia: raw.trivia.clone(),
            span: raw.span.clone(),
        }
    }

    fn build_example(&mut self, raw: &ListRaw<'a>, args: &[Raw<'a>], path: &str) -> TopForm {
        let arrow_ok = args.get(1).and_then(as_atom).is_some_and(|t| is_punct_tok(t, "=>"));
        if args.len() != 3 || !arrow_ok {
            self.error(
                raw.span.clone(),
                "example は (example expr => expr) >形です".to_string(),
                Some(json!({ "expected": "expr '=>' expr", "actual": args.len() })),
                None,
            );
        }
        let actual = match args.first() {
            Some(a) => self.build_expr(a, &format!("{path}.args.0")),
            None => expr_error("左辺がありません", raw.span.clone(), raw.trivia.clone()),
        };
        let expected = match args.get(2) {
            Some(e) => self.build_expr(e, &format!("{path}.args.2")),
            None => expr_error("右辺がありません", raw.span.clone(), raw.trivia.clone()),
        };
        TopForm::Example {
            actual: Box::new(actual),
            expected: Box::new(expected),
            trivia: raw.trivia.clone(),
            span: raw.span.clone(),
        }
    }

    fn build_param(&mut self, raw: &Raw<'a>, path: &str) -> Param {
        if let Some(l) = as_list(raw) {
            if l.items.len() == 3 {
                if let (Some(n), Some(c)) = (as_atom(&l.items[0]), as_atom(&l.items[1])) {
                    if is_symbol_tok(n) && is_punct_tok(c, ":") {
                        if is_reserved(&n.text) {
                            self.error(
                                n.span.clone(),
                                format!("予約語 '{}' は仮引数名に使えません", n.text),
                                None,
                                None,
                            );
                        }
                        let ty = self.build_type(&l.items[2], &format!("{path}.type"));
                        return Param {
                            name: n.text.clone(),
                            ty,
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        };
                    }
                }
            }
        }
        let span = self.raw_span_of(raw);
        self.error(span.clone(), "param は (name : Type) >形です".to_string(), None, None);
        Param {
            name: Rc::from(""),
            ty: type_error("不正な param です", span.clone(), Trivia::default()),
            trivia: Trivia::default(),
            span,
        }
    }

    fn build_type(&mut self, raw: &Raw<'a>, path: &str) -> TypeExpr {
        match raw {
            Raw::Error(t) => type_error("不正なトークンです", t.span.clone(), t.trivia.clone()),
            Raw::Atom(t) => match t.kind {
                TokenKind::Nil => TypeExpr::Prim {
                    name: Prim::Nil,
                    trivia: t.trivia.clone(),
                    span: t.span.clone(),
                },
                TokenKind::Symbol => {
                    if PRIMS.contains(&&*t.text) {
                        let name = match &*t.text {
                            "number" => Prim::Number,
                            "decimal" => Prim::Dec,
                            "string" => Prim::Str,
                            "bool" => Prim::Bool,
                            "char" => Prim::Char,
                            "builder" => Prim::Builder,
                            "bytes" => Prim::Bytes,
                            _ => Prim::Nil,
                        };
                        TypeExpr::Prim {
                            name,
                            trivia: t.trivia.clone(),
                            span: t.span.clone(),
                        }
                    } else if is_reserved(&t.text) {
                        self.error(
                            t.span.clone(),
                            format!("予約語 '{}' は型名として使えません", t.text),
                            None,
                            None,
                        );
                        type_error(
                            &format!("予約語 '{}' は型名として使えません", t.text),
                            t.span.clone(),
                            t.trivia.clone(),
                        )
                    } else {
                        TypeExpr::Alias {
                            name: t.text.clone(),
                            trivia: t.trivia.clone(),
                            span: t.span.clone(),
                        }
                    }
                }
                _ => {
                    self.error(
                        t.span.clone(),
                        format!("トークン '{}' は型になれません", t.text),
                        None,
                        None,
                    );
                    type_error(
                        &format!("トークン '{}' は型になれません", t.text),
                        t.span.clone(),
                        t.trivia.clone(),
                    )
                }
            },
            Raw::List(l) => {
                if l.close.is_none() {
                    return type_error("括弧が閉じていません", l.span.clone(), l.trivia.clone());
                }
                if l.items.is_empty() {
                    self.error(l.span.clone(), "空の型フォームです".to_string(), None, None);
                    return type_error("空の型フォームです", l.span.clone(), l.trivia.clone());
                }
                let head = &l.items[0];
                let args = &l.items[1..];
                let head_tok = match as_atom(head) {
                    Some(t) if is_symbol_tok(t) => t,
                    _ => {
                        self.error(
                            l.span.clone(),
                            "型フォームの先頭はシンボルでなければなりません".to_string(),
                            None,
                            None,
                        );
                        return type_error(
                            "型フォームの先頭はシンボルではありません",
                            l.span.clone(),
                            l.trivia.clone(),
                        );
                    }
                };
                let name = head_tok.text.clone();
                match &*name {
                    "Array" => {
                        if args.len() != 1 {
                            self.error(
                                l.span.clone(),
                                "Array は (Array T) >形です".to_string(),
                                Some(json!({ "expected": 1, "actual": args.len() })),
                                None,
                            );
                        }
                        let element = match args.first() {
                            Some(a) => self.build_type(a, &format!("{path}.element")),
                            None => type_error("要素型がありません", l.span.clone(), l.trivia.clone()),
                        };
                        TypeExpr::Array {
                            element: Box::new(element),
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        }
                    }
                    "union" => {
                        if args.is_empty() {
                            self.error(
                                l.span.clone(),
                                "union には1つ以上の型が必要です".to_string(),
                                None,
                                None,
                            );
                        }
                        let members = args
                            .iter()
                            .enumerate()
                            .map(|(i, a)| self.build_type(a, &format!("{path}.members.{i}")))
                            .collect();
                        TypeExpr::Union {
                            members,
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        }
                    }
                    "obj" => {
                        let mut fields: Vec<TypeField> = Vec::new();
                        for (i, f) in args.iter().enumerate() {
                            if let Some(fl) = as_list(f) {
                                if fl.items.len() == 3 {
                                    if let (Some(n), Some(c)) =
                                        (as_atom(&fl.items[0]), as_atom(&fl.items[1]))
                                    {
                                        if is_symbol_tok(n) && is_punct_tok(c, ":") {
                                            let ty = self.build_type(
                                                &fl.items[2],
                                                &format!("{path}.fields.{i}.type"),
                                            );
                                            fields.push(TypeField::Field {
                                                name: n.text.clone(),
                                                ty,
                                                trivia: fl.trivia.clone(),
                                                span: fl.span.clone(),
                                            });
                                            continue;
                                        }
                                    }
                                }
                                if fl.items.len() == 2 {
                                    if let (Some(n), Some(v)) =
                                        (as_atom(&fl.items[0]), as_atom(&fl.items[1]))
                                    {
                                        if is_symbol_tok(n) && v.kind == TokenKind::Str {
                                            let value = match &v.value {
                                                Some(TokenValue::Str(s)) => Rc::from(s.as_str()),
                                                _ => Rc::from(""),
                                            };
                                            fields.push(TypeField::Tag {
                                                name: n.text.clone(),
                                                value,
                                                trivia: fl.trivia.clone(),
                                                span: fl.span.clone(),
                                            });
                                            continue;
                                        }
                                    }
                                }
                            }
                            self.error(
                                self.raw_span_of(f),
                                "obj >フィールドは (name : Type) または (name \"tag\") >形です"
                                    .to_string(),
                                None,
                                None,
                            );
                        }
                        TypeExpr::Obj {
                            fields,
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        }
                    }
                    "map" => {
                        if args.len() != 2 {
                            self.error(
                                l.span.clone(),
                                "map は (map K V) 形です".to_string(),
                                Some(json!({ "expected": 2, "actual": args.len() })),
                                None,
                            );
                        }
                        let key = match args.first() {
                            Some(a) => self.build_type(a, &format!("{path}.key")),
                            None => type_error("キー型がありますません", l.span.clone(), l.trivia.clone()),
                        };
                        let value = match args.get(1) {
                            Some(a) => self.build_type(a, &format!("{path}.value")),
                            None => type_error("値型がありますません", l.span.clone(), l.trivia.clone()),
                        };
                        TypeExpr::Map {
                            key: Box::new(key),
                            value: Box::new(value),
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        }
                    }
                    "set" => {
                        if args.len() != 1 {
                            self.error(
                                l.span.clone(),
                                "set は (set K) 形です".to_string(),
                                Some(json!({ "expected": 1, "actual": args.len() })),
                                None,
                            );
                        }
                        let key = match args.first() {
                            Some(a) => self.build_type(a, &format!("{path}.key")),
                            None => type_error("キー型がありますません", l.span.clone(), l.trivia.clone()),
                        };
                        TypeExpr::Set {
                            key: Box::new(key),
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        }
                    }
                    "ref" => {
                        if args.len() != 1 {
                            self.error(
                                l.span.clone(),
                                "ref は (ref T) 形です".to_string(),
                                Some(json!({ "expected": 1, "actual": args.len() })),
                                None,
                            );
                        }
                        let inner = match args.first() {
                            Some(a) => self.build_type(a, &format!("{path}.inner")),
                            None => type_error("要素型がありますません", l.span.clone(), l.trivia.clone()),
                        };
                        TypeExpr::Ref {
                            inner: Box::new(inner),
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        }
                    }
                    "Result" => {
                        if args.len() != 2 {
                            self.error(
                                l.span.clone(),
                                "Result は (Result T E) >形です".to_string(),
                                Some(json!({ "expected": 2, "actual": args.len() })),
                                None,
                            );
                        }
                        let ok = match args.first() {
                            Some(a) => self.build_type(a, &format!("{path}.ok")),
                            None => type_error("Ok 型がありません", l.span.clone(), l.trivia.clone()),
                        };
                        let err = match args.get(1) {
                            Some(e) => self.build_type(e, &format!("{path}.err")),
                            None => type_error("Err 型がありません", l.span.clone(), l.trivia.clone()),
                        };
                        TypeExpr::ResultT {
                            ok: Box::new(ok),
                            err: Box::new(err),
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        }
                    }
                    "Option" => {
                        if args.len() != 1 {
                            self.error(
                                l.span.clone(),
                                "Option は (Option T) >形です".to_string(),
                                Some(json!({ "expected": 1, "actual": args.len() })),
                                None,
                            );
                        }
                        let t = match args.first() {
                            Some(a) => self.build_type(a, &format!("{path}.element")),
                            None => type_error("要素型がありません", l.span.clone(), l.trivia.clone()),
                        };
                        TypeExpr::Union {
                            members: vec![
                                t,
                                TypeExpr::Prim {
                                    name: Prim::Nil,
                                    trivia: l.trivia.clone(),
                                    span: l.span.clone(),
                                },
                            ],
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        }
                    }
                    "fn" => {
                        let mut params: Vec<Param> = Vec::new();
                        let mut i = 0;
                        while let Some(r @ Raw::List(l)) = args.get(i) {
                            if l.items.is_empty() {
                                i += 1;
                                continue;
                            }
                            params.push(
                                self.build_param(r, &format!("{path}.params.{}", params.len())),
                            );
                            i += 1;
                        }
                        if !args.get(i).and_then(as_atom).is_some_and(|t| is_punct_tok(t, ":")) {
                            self.error(
                                l.span.clone(),
                                "fn 型には ':' と戻り値の型が必要です".to_string(),
                                None,
                                None,
                            );
                            return TypeExpr::FnT {
                                params,
                                ret: Box::new(type_error(
                                    "戻り値の型がありません",
                                    l.span.clone(),
                                    l.trivia.clone(),
                                )),
                                trivia: l.trivia.clone(),
                                span: l.span.clone(),
                            };
                        }
                        i += 1;
                        if i >= args.len() {
                            self.error(
                                l.span.clone(),
                                "fn 型には戻り値の型が必要です".to_string(),
                                None,
                                None,
                            );
                            return TypeExpr::FnT {
                                params,
                                ret: Box::new(type_error(
                                    "戻り値の型がありません",
                                    l.span.clone(),
                                    l.trivia.clone(),
                                )),
                                trivia: l.trivia.clone(),
                                span: l.span.clone(),
                            };
                        }
                        let ret = self.build_type(&args[i], &format!("{path}.ret"));
                        TypeExpr::FnT {
                            params,
                            ret: Box::new(ret),
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        }
                    }
                    _ => {
                        if is_reserved(&name) {
                            self.error(
                                head_tok.span.clone(),
                                format!("予約語 '{}' は型コンストラクタとして不正です", name),
                                None,
                                None,
                            );
                        } else {
                            self.error(
                                head_tok.span.clone(),
                                format!("不明な型コンストラクタです: '{}'", name),
                                None,
                                None,
                            );
                        }
                        type_error(
                            &format!("不明な型コンストラクタ: {}", name),
                            l.span.clone(),
                            l.trivia.clone(),
                        )
                    }
                }
            }
        }
    }

    fn build_expr(&mut self, raw: &Raw<'a>, path: &str) -> Expr {
        match raw {
            Raw::Error(t) => expr_error("不正なトークンです", t.span.clone(), t.trivia.clone()),
            Raw::Atom(t) => self.build_atom(t),
            Raw::List(l) => {
                if l.close.is_none() {
                    return expr_error("括弧が閉じていません", l.span.clone(), l.trivia.clone());
                }
                if l.items.is_empty() {
                    self.error(l.span.clone(), "空のフォームです".to_string(), None, None);
                    return expr_error("空のフォームです", l.span.clone(), l.trivia.clone());
                }
                let head = &l.items[0];
                let args = &l.items[1..];
                if let Some(t) = as_atom(head) {
                    if is_punct_tok(t, "->") {
                        if args.len() < 2 {
                            self.error(
                                l.span.clone(),
                                "'->' には2つ以上の式が必要です".to_string(),
                                Some(json!({ "expected": "2+", "actual": args.len() })),
                                None,
                            );
                            return err_node("-> >引数が不足しています", l);
                        }
                        let exprs = args
                            .iter()
                            .enumerate()
                            .map(|(i, a)| self.build_expr(a, &format!("{path}.args.{i}")))
                            .collect();
                        return Expr::Pipe {
                            exprs,
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        };
                    }
                    if t.kind == TokenKind::Punct && OPERATORS.contains(&&*t.text) {
                        let expr_args = args
                            .iter()
                            .enumerate()
                            .map(|(i, a)| self.build_expr(a, &format!("{path}.args.{i}")))
                            .collect();
                        return Expr::App {
                            name: t.text.clone(),
                            effect: false,
                            args: expr_args,
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        };
                    }
                }
                let head_tok = match as_atom(head) {
                    Some(t) if is_symbol_tok(t) => t,
                    _ => {
                        self.error(
                            l.span.clone(),
                            "フォームの先頭はシンボルでなければなりません".to_string(),
                            None,
                            None,
                        );
                        return err_node("先頭がシンボルではありません", l);
                    }
                };
                let name = head_tok.text.clone();
                if SPECIAL_HEADS.contains(&&*name) {
                    return self.build_special(&name, l, args, path);
                }
                if is_reserved(&name) {
                    self.error(
                        head_tok.span.clone(),
                        format!("予約語 '{}' は式として使用できません", name),
                        None,
                        None,
                    );
                    return expr_error(
                        &format!("予約語 '{}' は使えません", name),
                        l.span.clone(),
                        l.trivia.clone(),
                    );
                }
                let expr_args = args
                    .iter()
                    .enumerate()
                    .map(|(i, a)| self.build_expr(a, &format!("{path}.args.{i}")))
                    .collect();
                Expr::App {
                    name,
                    effect: head_tok.text.ends_with('!'),
                    args: expr_args,
                    trivia: l.trivia.clone(),
                    span: l.span.clone(),
                }
            }
        }
    }

    fn build_atom(&mut self, t: &'a Token) -> Expr {
        match t.kind {
            TokenKind::Number => {
                let v = match &t.value {
                    Some(TokenValue::Num(v)) => *v,
                    _ => 0.0,
                };
                Expr::Lit {
                    value: LitValue::Num(v),
                    trivia: t.trivia.clone(),
                    span: t.span.clone(),
                }
            }
            TokenKind::Dec => {
                let v = match &t.value {
                    Some(TokenValue::Dec(s)) => s.clone(),
                    _ => Rc::from("0"),
                };
                Expr::Lit {
                    value: LitValue::Dec(v),
                    trivia: t.trivia.clone(),
                    span: t.span.clone(),
                }
            }
            TokenKind::Str => {
                let s = match &t.value {
                    Some(TokenValue::Str(s)) => Rc::from(s.as_str()),
                    _ => Rc::from(""),
                };
                Expr::Lit {
                    value: LitValue::Str(s),
                    trivia: t.trivia.clone(),
                    span: t.span.clone(),
                }
            }
            TokenKind::Bool => {
                let b = match &t.value {
                    Some(TokenValue::Bool(b)) => *b,
                    _ => false,
                };
                Expr::Lit {
                    value: LitValue::Bool(b),
                    trivia: t.trivia.clone(),
                    span: t.span.clone(),
                }
            }
            TokenKind::Char => {
                let c = match &t.value {
                    Some(TokenValue::Char(c)) => *c,
                    _ => 'a',
                };
                Expr::Lit {
                    value: LitValue::Char(c),
                    trivia: t.trivia.clone(),
                    span: t.span.clone(),
                }
            }
            TokenKind::Nil => Expr::Lit {
                value: LitValue::Nil,
                trivia: t.trivia.clone(),
                span: t.span.clone(),
            },
            TokenKind::DottedSymbol => {
                let parts = match &t.value {
                    Some(TokenValue::Dotted(parts)) => parts.clone(),
                    _ => t.text.split('.').map(Rc::from).collect(),
                };
                Expr::FieldAccess {
                    parts,
                    trivia: t.trivia.clone(),
                    span: t.span.clone(),
                }
            }
            TokenKind::Symbol => {
                if is_reserved(&t.text) {
                    self.error(
                        t.span.clone(),
                        format!("予約語 '{}' は式として使用できません", t.text),
                        None,
                        None,
                    );
                    return expr_error(
                        &format!("予約語 '{}' は使えません", t.text),
                        t.span.clone(),
                        t.trivia.clone(),
                    );
                }
                Expr::Sym {
                    name: t.text.clone(),
                    effect: t.text.ends_with('!'),
                    trivia: t.trivia.clone(),
                    span: t.span.clone(),
                }
            }
            _ => {
                self.error(
                    t.span.clone(),
                    format!("トークン '{}' は式になれません", t.text),
                    None,
                    None,
                );
                expr_error(
                    &format!("トークン '{}' は式になれません", t.text),
                    t.span.clone(),
                    t.trivia.clone(),
                )
            }
        }
    }

    fn build_special(&mut self, name: &str, raw: &ListRaw<'a>, args: &[Raw<'a>], path: &str) -> Expr {
        match name {
            "let" => {
                if args.len() != 2 {
                    self.error(
                        raw.span.clone(),
                        "let は (let [bind*] body) >形です".to_string(),
                        Some(json!({ "expected": "[bind*] body", "actual": args.len() })),
                        None,
                    );
                    return err_node("let >構文が不正です", raw);
                }
                let vec = match as_list(&args[0]) {
                    Some(l) => l,
                    None => {
                        self.error(
                            raw.span.clone(),
                            "let は (let [bind*] body) >形です".to_string(),
                            Some(json!({ "expected": "[bind*] body", "actual": args.len() })),
                            None,
                        );
                        return err_node("let >構文が不正です", raw);
                    }
                };
                if vec.items.len() % 2 != 0 {
                    self.error(
                        vec.span.clone(),
                        "let > bind は [name expr] >ペアでなければなりません".to_string(),
                        None,
                        None,
                    );
                }
                let mut binds: Vec<Bind> = Vec::new();
                let mut i = 0;
                while i + 1 < vec.items.len() {
                    let n = &vec.items[i];
                    let e = &vec.items[i + 1];
                    match as_atom(n) {
                        Some(t) if is_symbol_tok(t) => {
                            if is_reserved(&t.text) {
                                self.error(
                                    t.span.clone(),
                                    format!("予約語 '{}' は束縛名に使えません", t.text),
                                    None,
                                    None,
                                );
                            }
                            let expr = self.build_expr(e, &format!("{path}.binds.{}", binds.len()));
                            let span = Parser::join_span(&t.span, expr.span());
                            binds.push(Bind {
                                name: t.text.clone(),
                                expr: Box::new(expr),
                                trivia: t.trivia.clone(),
                                span,
                            });
                        }
                        _ => {
                            self.error(
                                self.raw_span_of(n),
                                "let > bind 名はシンボルでなければなりません".to_string(),
                                None,
                                None,
                            );
                        }
                    }
                    i += 2;
                }
                let body = self.build_expr(&args[1], &format!("{path}.body"));
                Expr::Let {
                    binds,
                    body: Box::new(body),
                    trivia: raw.trivia.clone(),
                    span: raw.span.clone(),
                }
            }
            "letrec" => {
                // (letrec [name lam-expr] body...) — design/interp-capability.md §2.1
                if args.is_empty() {
                    self.error(
                        raw.span.clone(),
                        "letrec は (letrec [name lam] body...) 形です".to_string(),
                        None,
                        None,
                    );
                    return err_node("letrec の構文が不正です", raw);
                }
                let vec = match as_list(&args[0]) {
                    Some(l) if l.bracket => l,
                    _ => {
                        self.error(
                            raw.span.clone(),
                            "letrec は (letrec [name lam] body...) 形です".to_string(),
                            None,
                            None,
                        );
                        return err_node("letrec の束縛は [name lam] 形です", raw);
                    }
                };
                if vec.items.len() != 2 {
                    self.error(
                        vec.span.clone(),
                        "letrec の束縛は [name lam] の1ペアです".to_string(),
                        Some(json!({ "expected": 2, "actual": vec.items.len() })),
                        None,
                    );
                }
                let name_tok = match as_atom(&vec.items[0]) {
                    Some(t) if is_symbol_tok(t) => t,
                    _ => {
                        self.error(
                            self.raw_span_of(&vec.items[0]),
                            "letrec の束縛名はシンボルです".to_string(),
                            None,
                            None,
                        );
                        return err_node("letrec の束縛名が不正です", raw);
                    }
                };
                if is_reserved(&name_tok.text) {
                    self.error(
                        name_tok.span.clone(),
                        format!("予約語 '{}' は束縛名に使えません", name_tok.text),
                        None,
                        None,
                    );
                }
                let lam = self.build_expr(&vec.items[1], &format!("{path}.lam"));
                let body: Vec<Expr> = args[1..]
                    .iter()
                    .enumerate()
                    .map(|(i, a)| self.build_expr(a, &format!("{path}.body.{i}")))
                    .collect();
                if body.is_empty() {
                    self.error(
                        raw.span.clone(),
                        "letrec には body が1つ以上必要です".to_string(),
                        None,
                        None,
                    );
                }
                Expr::Letrec {
                    name: name_tok.text.clone(),
                    lam: Box::new(lam),
                    body,
                    trivia: raw.trivia.clone(),
                    span: raw.span.clone(),
                }
            }
            "if" => {
                if args.len() != 3 {
                    self.error(
                        raw.span.clone(),
                        "if は (if cond then else) >形です".to_string(),
                        Some(json!({ "expected": 3, "actual": args.len() })),
                        None,
                    );
                    return err_node("if >引数が不正です", raw);
                }
                let cond = self.build_expr(&args[0], &format!("{path}.cond"));
                let then = self.build_expr(&args[1], &format!("{path}.then"));
                let else_ = self.build_expr(&args[2], &format!("{path}.else"));
                Expr::If {
                    cond: Box::new(cond),
                    then: Box::new(then),
                    else_: Box::new(else_),
                    trivia: raw.trivia.clone(),
                    span: raw.span.clone(),
                }
            }
            "when" => {
                if args.len() != 2 {
                    self.error(
                        raw.span.clone(),
                        "when は (when cond body) >形です".to_string(),
                        Some(json!({ "expected": 2, "actual": args.len() })),
                        None,
                    );
                    return err_node("when >引数が不正です", raw);
                }
                let cond = self.build_expr(&args[0], &format!("{path}.cond"));
                let body = self.build_expr(&args[1], &format!("{path}.body"));
                Expr::When {
                    cond: Box::new(cond),
                    body: Box::new(body),
                    trivia: raw.trivia.clone(),
                    span: raw.span.clone(),
                }
            }
            "and" | "or" => {
                if args.len() != 2 {
                    self.error(
                        raw.span.clone(),
                        format!("{} は ({} a b) >形です", name, name),
                        Some(json!({ "expected": 2, "actual": args.len() })),
                        None,
                    );
                    return err_node(&format!("{} >引数が不正です", name), raw);
                }
                let left = self.build_expr(&args[0], &format!("{path}.args.0"));
                let right = self.build_expr(&args[1], &format!("{path}.args.1"));
                if name == "and" {
                    Expr::And {
                        left: Box::new(left),
                        right: Box::new(right),
                        trivia: raw.trivia.clone(),
                        span: raw.span.clone(),
                    }
                } else {
                    Expr::Or {
                        left: Box::new(left),
                        right: Box::new(right),
                        trivia: raw.trivia.clone(),
                        span: raw.span.clone(),
                    }
                }
            }
            "cond" => {
                if args.is_empty() {
                    self.error(raw.span.clone(), "cond にはアームが必要です".to_string(), None, None);
                    return err_node("cond にアームがありません", raw);
                }
                let mut arms: Vec<CondArm> = Vec::new();
                let mut else_expr: Option<Expr> = None;
                for (i, arm_raw) in args.iter().enumerate() {
                    let arm = match arm_raw {
                        Raw::List(l) if l.items.len() == 2 => l,
                        _ => {
                            self.error(
                                self.raw_span_of(arm_raw),
                                "cond >アームは [cond expr] >形です".to_string(),
                                None,
                                None,
                            );
                            continue;
                        }
                    };
                    let c = &arm.items[0];
                    let b = &arm.items[1];
                    let is_else = i == args.len() - 1
                        && matches!(as_atom(c), Some(t) if t.kind == TokenKind::Keyword && &*t.text == ":else");
                    if is_else {
                        else_expr = Some(self.build_expr(b, &format!("{path}.arms.{i}")));
                    } else {
                        let cond = self.build_expr(c, &format!("{path}.arms.{i}"));
                        let body = self.build_expr(b, &format!("{path}.arms.{i}"));
                        arms.push(CondArm {
                            cond: Box::new(cond),
                            body: Box::new(body),
                            trivia: arm.trivia.clone(),
                            span: arm.span.clone(),
                        });
                    }
                }
                let else_ = match else_expr {
                    Some(e) => e,
                    None => {
                        self.error(
                            raw.span.clone(),
                            "cond >最後のアームは [:else expr] でなければなりません（網羅性のため §3.5）"
                                .to_string(),
                            None,
                            Some(Patch::Insert {
                                path: format!("{path}.arms"),
                                template: "[:else nil]".to_string(),
                            }),
                        );
                        expr_error("[:else ...] がありません", raw.span.clone(), raw.trivia.clone())
                    }
                };
                Expr::Cond {
                    arms,
                    else_: Box::new(else_),
                    trivia: raw.trivia.clone(),
                    span: raw.span.clone(),
                }
            }
            "match" => {
                if args.len() < 2 {
                    self.error(
                        raw.span.clone(),
                        "match は (match scrutinee arm+) >形です".to_string(),
                        Some(json!({ "expected": "scrutinee + arm", "actual": args.len() })),
                        None,
                    );
                    return err_node("match >引数が不正です", raw);
                }
                let scrutinee = self.build_expr(&args[0], &format!("{path}.scrutinee"));
                let mut arms: Vec<MatchArm> = Vec::new();
                for (i, a) in args.iter().enumerate().skip(1) {
                    let arm = match a {
                        Raw::List(l) if l.items.len() == 2 => l,
                        _ => {
                            self.error(
                                self.raw_span_of(a),
                                "match >アームは [pattern expr] >形です".to_string(),
                                None,
                                None,
                            );
                            continue;
                        }
                    };
                    let pattern = self.build_pattern(&arm.items[0], &format!("{path}.arms.{}", i - 1));
                    let body = self.build_expr(&arm.items[1], &format!("{path}.arms.{}", i - 1));
                    arms.push(MatchArm {
                        pattern,
                        body: Box::new(body),
                        trivia: arm.trivia.clone(),
                        span: arm.span.clone(),
                    });
                }
                Expr::Match {
                    scrutinee: Box::new(scrutinee),
                    arms,
                    trivia: raw.trivia.clone(),
                    span: raw.span.clone(),
                }
            }
            "do" => {
                if args.is_empty() {
                    self.error(raw.span.clone(), "do には本体が必要です".to_string(), None, None);
                    return err_node("do に本体がありません", raw);
                }
                let mut binds: Vec<DoBind> = Vec::new();
                for (i, b) in args[..args.len() - 1].iter().enumerate() {
                    let bind_path = format!("{path}.binds.{i}");
                    // do-io (P1, §3.5): [name expr] 以外の要素は名前なし束縛として `_` に糖衣する
                    let named = match b {
                        Raw::List(l) if l.bracket && l.items.len() == 2 => match as_atom(&l.items[0]) {
                            Some(t) if is_symbol_tok(t) => Some((l, t)),
                            _ => None,
                        },
                        _ => None,
                    };
                    match named {
                        Some((bl, t)) => {
                            if is_reserved(&t.text) {
                                self.error(
                                    t.span.clone(),
                                    format!("予約語 '{}' は束縛名に使えません", t.text),
                                    None,
                                    None,
                                );
                            }
                            let expr = self.build_expr(&bl.items[1], &bind_path);
                            binds.push(DoBind {
                                name: t.text.clone(),
                                expr: Box::new(expr),
                                trivia: bl.trivia.clone(),
                                span: bl.span.clone(),
                            });
                        }
                        None => {
                            let expr = self.build_expr(b, &bind_path);
                            binds.push(DoBind {
                                name: Rc::from("_"),
                                expr: Box::new(expr),
                                trivia: Trivia::default(),
                                span: self.raw_span_of(b),
                            });
                        }
                    }
                }
                let body = self.build_expr(&args[args.len() - 1], &format!("{path}.body"));
                Expr::Do {
                    binds,
                    body: Box::new(body),
                    trivia: raw.trivia.clone(),
                    span: raw.span.clone(),
                }
            }
            "obj" => {
                let mut fields: Vec<ObjField> = Vec::new();
                for (i, f) in args.iter().enumerate() {
                    let fl = match f {
                        Raw::List(l) if l.items.len() == 2 => l,
                        _ => {
                            self.error(
                                self.raw_span_of(f),
                                "obj >フィールドは (name expr) >形です".to_string(),
                                None,
                                None,
                            );
                            continue;
                        }
                    };
                    match as_atom(&fl.items[0]) {
                        Some(t) if is_symbol_tok(t) => {
                            let expr = self.build_expr(&fl.items[1], &format!("{path}.fields.{i}"));
                            fields.push(ObjField {
                                name: t.text.clone(),
                                expr: Box::new(expr),
                                trivia: fl.trivia.clone(),
                                span: fl.span.clone(),
                            });
                        }
                        _ => {
                            self.error(
                                self.raw_span_of(f),
                                "obj >フィールドは (name expr) >形です".to_string(),
                                None,
                                None,
                            );
                        }
                    }
                }
                Expr::Obj {
                    fields,
                    trivia: raw.trivia.clone(),
                    span: raw.span.clone(),
                }
            }
            "fn" => {
                let mut params: Vec<Param> = Vec::new();
                let mut i = 0;
                while let Some(r @ Raw::List(l)) = args.get(i) {
                    if l.items.is_empty() {
                        i += 1;
                        continue;
                    }
                    params.push(self.build_param(r, &format!("{path}.params.{}", params.len())));
                    i += 1;
                }
                if !args.get(i).and_then(as_atom).is_some_and(|t| is_punct_tok(t, ":")) {
                    self.error(
                        raw.span.clone(),
                        "fn には ':' と戻り値の型、本体が必要です".to_string(),
                        None,
                        None,
                    );
                    return err_node("fn >構文が不正です", raw);
                }
                i += 1;
                if i >= args.len() {
                    self.error(raw.span.clone(), "fn には戻り値の型が必要です".to_string(), None, None);
                    return err_node("fn >戻り値の型がありません", raw);
                }
                let ret = self.build_type(&args[i], &format!("{path}.ret"));
                i += 1;
                if i >= args.len() {
                    self.error(raw.span.clone(), "fn には本体が必要です".to_string(), None, None);
                    return err_node("fn >本体がありません", raw);
                }
                let body = self.build_expr(&args[i], &format!("{path}.body"));
                Expr::Fn {
                    params,
                    ret: Box::new(ret),
                    body: Box::new(body),
                    trivia: raw.trivia.clone(),
                    span: raw.span.clone(),
                }
            }
            _ => err_node(&format!("未対応の特殊フォーム: {}", name), raw),
        }
    }

    fn build_pattern(&mut self, raw: &Raw<'a>, path: &str) -> Pat {
        match raw {
            Raw::Error(t) => Pat::Wildcard {
                trivia: t.trivia.clone(),
                span: t.span.clone(),
            },
            Raw::Atom(t) => match t.kind {
                TokenKind::Symbol => {
                    if &*t.text == "_" {
                        Pat::Wildcard {
                            trivia: t.trivia.clone(),
                            span: t.span.clone(),
                        }
                    } else if is_reserved(&t.text) {
                        self.error(
                            t.span.clone(),
                            format!("予約語 '{}' はパターンに使えません", t.text),
                            None,
                            None,
                        );
                        Pat::Wildcard {
                            trivia: t.trivia.clone(),
                            span: t.span.clone(),
                        }
                    } else {
                        Pat::Var {
                            name: t.text.clone(),
                            trivia: t.trivia.clone(),
                            span: t.span.clone(),
                        }
                    }
                }
                TokenKind::Number => {
                    let v = match &t.value {
                        Some(TokenValue::Num(v)) => *v,
                        _ => 0.0,
                    };
                    Pat::Lit {
                        value: LitValue::Num(v),
                        trivia: t.trivia.clone(),
                        span: t.span.clone(),
                    }
                }
                TokenKind::Str => {
                    let s = match &t.value {
                        Some(TokenValue::Str(s)) => Rc::from(s.as_str()),
                        _ => Rc::from(""),
                    };
                    Pat::Lit {
                        value: LitValue::Str(s),
                        trivia: t.trivia.clone(),
                        span: t.span.clone(),
                    }
                }
                TokenKind::Char => {
                    let c = match &t.value {
                        Some(TokenValue::Char(c)) => *c,
                        _ => 'a',
                    };
                    Pat::Lit {
                        value: LitValue::Char(c),
                        trivia: t.trivia.clone(),
                        span: t.span.clone(),
                    }
                }
                TokenKind::Bool => {
                    let b = match &t.value {
                        Some(TokenValue::Bool(b)) => *b,
                        _ => false,
                    };
                    Pat::Lit {
                        value: LitValue::Bool(b),
                        trivia: t.trivia.clone(),
                        span: t.span.clone(),
                    }
                }
                TokenKind::Nil => Pat::Lit {
                    value: LitValue::Nil,
                    trivia: t.trivia.clone(),
                    span: t.span.clone(),
                },
                _ => {
                    self.error(
                        t.span.clone(),
                        format!("トークン '{}' はパターンになれません", t.text),
                        None,
                        None,
                    );
                    Pat::Wildcard {
                        trivia: t.trivia.clone(),
                        span: t.span.clone(),
                    }
                }
            },
            Raw::List(l) => {
                if l.items.is_empty() {
                    self.error(l.span.clone(), "空のパターンです".to_string(), None, None);
                    return Pat::Wildcard {
                        trivia: l.trivia.clone(),
                        span: l.span.clone(),
                    };
                }
                let head = &l.items[0];
                let args = &l.items[1..];
                let head_tok = match as_atom(head) {
                    Some(t) if is_symbol_tok(t) => t,
                    _ => {
                        self.error(
                            l.span.clone(),
                            "パターンの先頭はシンボルでなければなりません".to_string(),
                            None,
                            None,
                        );
                        return Pat::Wildcard {
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        };
                    }
                };
                let name = head_tok.text.clone();
                match &*name {
                    "obj" => {
                        let mut fields: Vec<PatField> = Vec::new();
                        for (i, f) in args.iter().enumerate() {
                            let fl = match f {
                                Raw::List(l) if l.items.len() == 2 => l,
                                _ => {
                                    self.error(
                                        self.raw_span_of(f),
                                        "obj パターンのフィールドは (name pattern) >形です"
                                            .to_string(),
                                        None,
                                        None,
                                    );
                                    continue;
                                }
                            };
                            match as_atom(&fl.items[0]) {
                                Some(t) if is_symbol_tok(t) => {
                                    let pattern =
                                        self.build_pattern(&fl.items[1], &format!("{path}.fields.{i}"));
                                    fields.push(PatField {
                                        name: t.text.clone(),
                                        pattern: Box::new(pattern),
                                        trivia: fl.trivia.clone(),
                                        span: fl.span.clone(),
                                    });
                                }
                                _ => {
                                    self.error(
                                        self.raw_span_of(f),
                                        "obj パターンのフィールドは (name pattern) >形です"
                                            .to_string(),
                                        None,
                                        None,
                                    );
                                }
                            }
                        }
                        Pat::Obj {
                            fields,
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        }
                    }
                    "Array" => {
                        let elements = args
                            .iter()
                            .enumerate()
                            .map(|(i, a)| self.build_pattern(a, &format!("{path}.{i}")))
                            .collect();
                        Pat::Array {
                            elements,
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        }
                    }
                    "or" => {
                        if args.is_empty() {
                            self.error(
                                l.span.clone(),
                                "or パターンには1つ以上のパターンが必要です".to_string(),
                                None,
                                None,
                            );
                        }
                        let patterns = args
                            .iter()
                            .enumerate()
                            .map(|(i, a)| self.build_pattern(a, &format!("{path}.{i}")))
                            .collect();
                        Pat::Or {
                            patterns,
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        }
                    }
                    _ => {
                        self.error(
                            head_tok.span.clone(),
                            format!("不明なパターンです: '{}'", name),
                            None,
                            None,
                        );
                        Pat::Wildcard {
                            trivia: l.trivia.clone(),
                            span: l.span.clone(),
                        }
                    }
                }
            }
        }
    }
}

pub fn parse_expr(src: &str, file: &str) -> (Option<Expr>, Vec<Diagnostic>) {
    let lex = tokenize(src, file);
    let mut parser = Parser::new(&lex.tokens);
    let raws = parser.read_all();
    let expr = raws.first().map(|r| parser.build_expr(r, "(expr)"));
    let mut diags = lex.diagnostics.clone();
    diags.extend(parser.diagnostics);
    (expr, diags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    fn parse_src(src: &str) -> (Program, Vec<Diagnostic>) {
        let lex = tokenize(src, "test.lts");
        parse_program(&lex)
    }

    fn fixture_src(name: &str) -> String {
        let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
        std::fs::read_to_string(&path).expect("fixture readable")
    }

    fn parse_fixture(name: &str) -> (Program, Vec<Diagnostic>) {
        let src = fixture_src(name);
        let lex = tokenize(&src, name);
        parse_program(&lex)
    }

    fn as_defn<'a>(p: &'a Program, name: &str) -> &'a TopForm {
        p.forms
            .iter()
            .find(|f| matches!(f, TopForm::Defn { name: n, .. } if &**n == name))
            .expect("defn not found")
    }

    fn defn_body<'a>(p: &'a Program, name: &str) -> &'a Expr {
        match as_defn(p, name) {
            TopForm::Defn { body, .. } => body,
            _ => panic!("not a defn"),
        }
    }

    fn form_kind(f: &TopForm) -> &'static str {
        match f {
            TopForm::Import { .. } => "import",
            TopForm::Export { .. } => "export",
            TopForm::TypeDef { .. } => "typeDef",
            TopForm::EffectDef { .. } => "effectDef",
            TopForm::Defn { .. } => "defn",
            TopForm::Prop { .. } => "prop",
            TopForm::Example { .. } => "example",
            TopForm::Error { .. } => "error",
        }
    }

    #[test]
    fn money_lts_parses_clean() {
        let (p, diags) = parse_fixture("money.lts");
        assert!(diags.is_empty(), "{diags:?}");
        let kinds: Vec<&str> = p.forms.iter().map(form_kind).collect();
        assert_eq!(kinds, ["typeDef", "export", "defn", "defn", "example", "prop"]);

        let TopForm::TypeDef { name, ty, .. } = &p.forms[0] else {
            panic!("forms[0] is not typeDef")
        };
        assert_eq!(&**name, "Money");
        assert!(matches!(ty, TypeExpr::Obj { .. }));

        let TopForm::Defn {
            params, ret, body, ..
        } = as_defn(&p, "zero")
        else {
            panic!()
        };
        assert!(params.is_empty());
        assert!(matches!(ret, TypeExpr::Alias { name, .. } if &**name == "Money"));
        assert!(matches!(body.as_ref(), Expr::Obj { .. }));

        let TopForm::Defn {
            effectful,
            params,
            ret,
            body,
            ..
        } = as_defn(&p, "add-money")
        else {
            panic!()
        };
        assert!(!effectful);
        let names: Vec<&str> = params.iter().map(|p| &*p.name).collect();
        assert_eq!(names, ["a", "b"]);
        assert!(matches!(ret, TypeExpr::ResultT { .. }));
        assert!(matches!(body.as_ref(), Expr::If { .. }));
    }

    #[test]
    fn cart_lts_parses_clean() {
        let (p, diags) = parse_fixture("cart.lts");
        assert!(diags.is_empty(), "{diags:?}");
        let kinds: Vec<&str> = p.forms.iter().map(form_kind).collect();
        assert_eq!(
            kinds,
            [
                "import", "typeDef", "typeDef", "typeDef", "export", "defn", "defn", "defn",
                "example"
            ]
        );

        let TopForm::Import { path, names, .. } = &p.forms[0] else {
            panic!()
        };
        assert_eq!(&**path, "./money.lts");
        let names: Vec<&str> = names.iter().map(|n| &**n).collect();
        assert_eq!(names, ["Money", "add-money", "zero"]);

        let place = defn_body(&p, "place-order");
        let Expr::Do { binds, body, .. } = place else {
            panic!("place-order body is not do")
        };
        let bn: Vec<&str> = binds.iter().map(|b| &*b.name).collect();
        assert_eq!(bn, ["_", "t"]);
        assert!(matches!(body.as_ref(), Expr::App { name, .. } if &**name == "ok"));

        let check = defn_body(&p, "check-items");
        let Expr::If { else_, .. } = check else {
            panic!()
        };
        let Expr::Match { arms, .. } = else_.as_ref() else {
            panic!()
        };
        assert_eq!(arms.len(), 2);
        assert!(matches!(&arms[0].pattern, Pat::Lit { value: LitValue::Nil, .. }));
        assert!(matches!(&arms[1].pattern, Pat::Var { name, .. } if &**name == "i"));
    }

    #[test]
    fn broken_lts_recovers_per_form() {
        let (p, diags) = parse_fixture("broken.lts");
        assert_eq!(p.forms.len(), 2);
        assert!(!diags.is_empty());
        assert!(diags.iter().all(|d| d.kind == DiagKind::ParseError));

        let TopForm::Defn { body, .. } = as_defn(&p, "broken") else {
            panic!()
        };
        assert!(matches!(body.as_ref(), Expr::Error { .. }));

        let TopForm::Defn { body, span, .. } = as_defn(&p, "ok") else {
            panic!()
        };
        assert!(matches!(body.as_ref(), Expr::App { name, .. } if &**name == "+"));
        assert!(diags.iter().all(|d| d.span.end <= span.start));
    }

    #[test]
    fn appendix_b_fixtures_parse_clean() {
        for name in ["effects.lts", "stock.lts", "cycle-a.lts", "cycle-b.lts"] {
            let (p, diags) = parse_fixture(name);
            assert!(diags.is_empty(), "{name}: {diags:?}");
            assert!(!p.forms.is_empty(), "{name}");
        }
    }

    #[test]
    fn effects_lts_effect_def_and_effect_call() {
        let (p, diags) = parse_fixture("effects.lts");
        assert!(diags.is_empty(), "{diags:?}");
        let TopForm::EffectDef { name, .. } = &p.forms[0] else {
            panic!()
        };
        assert_eq!(&**name, "log");
        let Expr::Do { binds, .. } = defn_body(&p, "pure-run") else {
            panic!()
        };
        let Expr::App { name, effect, .. } = &*binds[0].expr else {
            panic!()
        };
        assert_eq!(&**name, "log!");
        assert!(*effect);
    }

    #[test]
    fn stock_lts_obj_pattern() {
        let (p, diags) = parse_fixture("stock.lts");
        assert!(diags.is_empty(), "{diags:?}");
        let Expr::Match { arms, .. } = defn_body(&p, "show-error") else {
            panic!()
        };
        let Pat::Obj { fields, .. } = &arms[0].pattern else {
            panic!("arm pattern is not obj")
        };
        assert!(
            matches!(&fields[0], PatField { name, pattern, .. } if &**name == "tag"
                && matches!(pattern.as_ref(), Pat::Lit { value: LitValue::Str(s), .. } if &**s == "PriceChanged"))
        );
        assert!(
            matches!(&fields[1], PatField { pattern, .. } if matches!(pattern.as_ref(), Pat::Var { name, .. } if &**name == "p"))
        );
    }

    #[test]
    fn reserved_words_in_expr_position_are_errors() {
        for w in RESERVED {
            let (_, diags) = parse_src(&format!("(defn f () : number {w})"));
            assert!(
                diags.iter().any(|d| d.kind == DiagKind::ParseError),
                "reserved word {w} should be a parse-error"
            );
        }
    }

    #[test]
    fn do_io_bare_exprs_become_underscore_binds() {
        // do-io (P1, §3.5): (do e1 e2 ...) の名前なし要素は `_` 束縛になる
        let (p, diags) = parse_src("(defn f () : number (do (g 1) [v (h 2)] (+ v 1)))");
        assert!(diags.is_empty(), "{diags:?}");
        let crate::ast::TopForm::Defn { body, .. } = &p.forms[0] else {
            panic!()
        };
        let crate::ast::Expr::Do { binds, body, .. } = body.as_ref() else {
            panic!("do expected")
        };
        assert_eq!(binds.len(), 2);
        assert_eq!(binds[0].name.as_ref(), "_");
        assert_eq!(binds[1].name.as_ref(), "v");
        assert!(matches!(body.as_ref(), crate::ast::Expr::App { .. }));
    }

    #[test]
    fn special_form_arity_errors() {
        for src in [
            "(if 1 2)",
            "(let 1)",
            "(cond)",
            "(match x)",
            "(do)",
            "(and 1)",
            "(-> x)",
        ] {
            let (_, diags) = parse_src(&format!("(defn f () : number {src})"));
            assert!(
                diags.iter().any(|d| d.kind == DiagKind::ParseError),
                "{src} should be a parse-error"
            );
        }
    }

    #[test]
    fn top_level_non_form_is_error() {
        let (_, diags) = parse_src("(let [a 1] a)");
        assert!(diags.iter().any(|d| d.kind == DiagKind::ParseError));
    }

    #[test]
    fn let_binds() {
        let (p, diags) = parse_src("(defn f () : number (let [a 1 b 2] (+ a b)))");
        assert!(diags.is_empty(), "{diags:?}");
        let Expr::Let { binds, body, .. } = defn_body(&p, "f") else {
            panic!()
        };
        let names: Vec<&str> = binds.iter().map(|b| &*b.name).collect();
        assert_eq!(names, ["a", "b"]);
        assert!(matches!(body.as_ref(), Expr::App { name, .. } if &**name == "+"));
    }

    #[test]
    fn cond_requires_else_arm_with_insert_fix() {
        let (_, diags) =
            parse_src("(defn f (x : number) : number (cond [true 1] [false 2]))");
        let d = diags
            .iter()
            .find(|d| d.kind == DiagKind::ParseError)
            .expect("parse-error expected");
        assert!(
            matches!(&d.fix, Some(Patch::Insert { template, .. }) if template.contains(":else"))
        );

        let (p, diags) =
            parse_src("(defn f (x : number) : number (cond [true 1] [:else 2]))");
        assert!(diags.is_empty(), "{diags:?}");
        let Expr::Cond { arms, else_, .. } = defn_body(&p, "f") else {
            panic!()
        };
        assert_eq!(arms.len(), 1);
        assert!(matches!(else_.as_ref(), Expr::Lit { value: LitValue::Num(v), .. } if *v == 2.0));
    }

    #[test]
    fn brackets_outside_special_positions_normalize_to_parens() {
        let (p, diags) = parse_src("(defn f () : number [x 1])");
        assert!(diags.is_empty(), "{diags:?}");
        assert!(matches!(defn_body(&p, "f"), Expr::App { name, .. } if &**name == "x"));
    }

    #[test]
    fn brace_is_parse_error() {
        let (_, diags) = parse_src("(defn f () : number { 1 })");
        assert!(diags
            .iter()
            .any(|d| d.kind == DiagKind::ParseError && d.message.contains('{')));
    }

    #[test]
    fn pipe_and_field_access() {
        let (p, diags) =
            parse_src("(defn f (o : (obj (a : number))) : number (-> o.a g h))");
        assert!(diags.is_empty(), "{diags:?}");
        let Expr::Pipe { exprs, .. } = defn_body(&p, "f") else {
            panic!()
        };
        assert_eq!(exprs.len(), 3);
        assert!(matches!(&exprs[0], Expr::FieldAccess { parts, .. }
            if parts.iter().map(|p| &**p).collect::<Vec<_>>() == ["o", "a"]));
    }

    #[test]
    fn defn_bang_sets_effect_flag() {
        let (p, diags) = parse_src("(defn! f () : nil (ok nil))");
        assert!(diags.is_empty(), "{diags:?}");
        let TopForm::Defn { effectful, .. } = as_defn(&p, "f") else {
            panic!()
        };
        assert!(*effectful);
    }

    #[test]
    fn fn_form_params_and_ret() {
        let (p, diags) =
            parse_src("(defn f () : number (fn (x : number) (y : number) : number (+ x y)))");
        assert!(diags.is_empty(), "{diags:?}");
        let Expr::Fn { params, ret, .. } = defn_body(&p, "f") else {
            panic!()
        };
        let names: Vec<&str> = params.iter().map(|p| &*p.name).collect();
        assert_eq!(names, ["x", "y"]);
        assert!(matches!(ret.as_ref(), TypeExpr::Prim { name: Prim::Number, .. }));
    }

    #[test]
    fn unclosed_paren_is_parse_error() {
        let (_, diags) = parse_src("(+ 1 2");
        assert!(diags.iter().any(|d| d.kind == DiagKind::ParseError));
    }

    #[test]
    fn option_type_normalizes_to_union_with_nil() {
        let (p, diags) = parse_src("(type X (Option number))");
        assert!(diags.is_empty(), "{diags:?}");
        let TopForm::TypeDef { ty, .. } = &p.forms[0] else {
            panic!()
        };
        let TypeExpr::Union { members, .. } = ty else {
            panic!("Option should normalize to union")
        };
        assert_eq!(members.len(), 2);
        assert!(matches!(&members[0], TypeExpr::Prim { name: Prim::Number, .. }));
        assert!(matches!(&members[1], TypeExpr::Prim { name: Prim::Nil, .. }));
    }

    #[test]
    fn tag_fields_and_effect_fn_type() {
        let (_, diags) =
            parse_src("(type E (union (obj (tag \"A\")) (obj (tag \"B\") (x : number))))");
        assert!(diags.is_empty(), "{diags:?}");
        let (p, diags) = parse_src("(effect log : (fn (s : string) : nil))");
        assert!(diags.is_empty(), "{diags:?}");
        let TopForm::EffectDef { ty, .. } = &p.forms[0] else {
            panic!()
        };
        assert!(matches!(ty, TypeExpr::FnT { .. }));
    }

    #[test]
    fn import_wildcard_is_forbidden() {
        let (_, diags) = parse_src("(import \"./x.lts\" (*))");
        assert!(diags.iter().any(|d| d.kind == DiagKind::ParseError));
    }

    #[test]
    fn trivia_preserved_on_nodes() {
        let (p, _) = parse_fixture("cart.lts");
        let place_error = p
            .forms
            .iter()
            .find(|f| matches!(f, TopForm::TypeDef { name, .. } if &**name == "PlaceError"))
            .expect("PlaceError typeDef");
        let TopForm::TypeDef { trivia, .. } = place_error else {
            panic!()
        };
        assert!(trivia.leading.iter().any(|c| c.text.contains("place-order")));

        let Expr::If { else_, .. } = defn_body(&p, "check-items") else {
            panic!()
        };
        let Expr::Match { arms, .. } = else_.as_ref() else {
            panic!()
        };
        assert_eq!(arms[0].trivia.leading.len(), 2);
        assert!(arms[0].trivia.leading[0].text.contains("Option CartItem"));

        let (p, _) = parse_fixture("money.lts");
        assert!(p.trailing.leading.iter().any(|c| c.text.contains("prop はP0では受理のみ")));

        let (p, diags) = parse_src(";; hello\n");
        assert!(p.forms.is_empty());
        assert_eq!(p.trailing.leading.len(), 1);
        assert_eq!(&*p.trailing.leading[0].text, ";; hello");
        assert!(diags.is_empty());
    }
}
