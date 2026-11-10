use std::borrow::Cow;
use std::rc::Rc;

use crate::diagnostic::{DiagKind, Diagnostic, Severity};
use crate::span::Span;
use crate::token::{Comment, Punct, Token, TokenKind, TokenValue, Trivia};

pub struct LexResult {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
    pub trailing: Trivia,
}

fn is_symbol_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_symbol_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '?'
}

fn is_digit(c: char) -> bool {
    c.is_ascii_digit()
}

fn is_hex(c: char) -> bool {
    c.is_ascii_hexdigit()
}

struct Lexer<'a> {
    src: &'a str,
    file: Rc<str>,
    pos: usize,
    line: usize,
    col: usize,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
    pending: Vec<Comment>,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str, file: &str) -> Self {
        Lexer {
            src,
            file: Rc::from(file),
            pos: 0,
            line: 1,
            col: 1,
            tokens: Vec::new(),
            diagnostics: Vec::new(),
            pending: Vec::new(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek_second(&self) -> Option<char> {
        let mut it = self.src[self.pos..].chars();
        it.next();
        it.next()
    }

    fn starts_with(&self, s: &str) -> bool {
        self.src[self.pos..].starts_with(s)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn mark(&self) -> (usize, usize, usize) {
        (self.pos, self.line, self.col)
    }

    fn span(&self, start: usize, line: usize, col: usize, end: usize) -> Span {
        Span {
            file: self.file.clone(),
            start,
            end,
            line,
            col,
        }
    }

    fn emit(
        &mut self,
        kind: TokenKind,
        start: usize,
        line: usize,
        col: usize,
        end: usize,
        value: Option<TokenValue>,
    ) {
        let text: Rc<str> = Rc::from(&self.src[start..end]);
        self.tokens.push(Token {
            kind,
            text,
            value,
            trivia: Trivia {
                leading: std::mem::take(&mut self.pending),
            },
            span: self.span(start, line, col, end),
        });
    }

    fn emit_error(&mut self, start: usize, line: usize, col: usize, end: usize, message: String) {
        self.diagnostics.push(Diagnostic {
            kind: DiagKind::ParseError,
            severity: Severity::Error,
            span: self.span(start, line, col, end),
            message,
            data: None,
            fix: None,
        });
        self.emit(TokenKind::Error, start, line, col, end, None);
    }

    fn lex_one(&mut self, kind: TokenKind) {
        let (start, line, col) = self.mark();
        self.bump();
        let end = self.pos;
        self.emit(kind, start, line, col, end, None);
    }

    fn lex_comment(&mut self) {
        let (start, line, col) = self.mark();
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            self.bump();
        }
        let end = self.pos;
        let text: Rc<str> = Rc::from(&self.src[start..end]);
        self.pending.push(Comment {
            text,
            span: self.span(start, line, col, end),
        });
    }

    fn lex_punct(&mut self) -> bool {
        let (start, line, col) = self.mark();
        let two = [
            ("<=", Punct::Le),
            (">=", Punct::Ge),
            ("=>", Punct::FatArrow),
            ("->", Punct::Arrow),
        ];
        for (s, p) in two {
            if self.starts_with(s) {
                self.bump();
                self.bump();
                let end = self.pos;
                self.emit(
                    TokenKind::Punct,
                    start,
                    line,
                    col,
                    end,
                    Some(TokenValue::Punct(p)),
                );
                return true;
            }
        }
        let one = match self.peek() {
            Some(':') => Some(Punct::Colon),
            Some('+') => Some(Punct::Plus),
            Some('-') => Some(Punct::Minus),
            Some('*') => Some(Punct::Star),
            Some('/') => Some(Punct::Slash),
            Some('<') => Some(Punct::Lt),
            Some('>') => Some(Punct::Gt),
            Some('=') => Some(Punct::Eq),
            _ => None,
        };
        if let Some(p) = one {
            self.bump();
            let end = self.pos;
            self.emit(
                TokenKind::Punct,
                start,
                line,
                col,
                end,
                Some(TokenValue::Punct(p)),
            );
            return true;
        }
        false
    }

    fn lex_number(&mut self) {
        let (start, line, col) = self.mark();
        if self.peek() == Some('-') {
            self.bump();
        }
        while self.peek().is_some_and(is_digit) {
            self.bump();
        }
        if self.peek() == Some('.') && self.peek_second().is_some_and(is_digit) {
            self.bump();
            while self.peek().is_some_and(is_digit) {
                self.bump();
            }
        }
        if matches!(self.peek(), Some('e') | Some('E')) {
            let bytes = self.src.as_bytes();
            let mut k = self.pos + 1;
            if k < bytes.len() && (bytes[k] == b'+' || bytes[k] == b'-') {
                k += 1;
            }
            if k < bytes.len() && bytes[k].is_ascii_digit() {
                self.bump();
                if matches!(self.peek(), Some('+') | Some('-')) {
                    self.bump();
                }
                while self.peek().is_some_and(is_digit) {
                    self.bump();
                }
            }
        }
        // decimal 接尾辞: 1.50d（直後がシンボル文字・'!' でなければ d を消費）
        if self.peek() == Some('d')
            && !self
                .peek_second()
                .is_some_and(|c| is_symbol_char(c) || c == '!')
        {
            self.bump();
            let end = self.pos;
            let text = &self.src[start..end - 1];
            self.emit(TokenKind::Dec, start, line, col, end, Some(TokenValue::Dec(Rc::from(text))));
            return;
        }
        let end = self.pos;
        let text = &self.src[start..end];
        match text.parse::<f64>() {
            Ok(v) => self.emit(TokenKind::Number, start, line, col, end, Some(TokenValue::Num(v))),
            Err(_) => self.emit_error(start, line, col, end, "不正な数値です".to_string()),
        }
    }

    fn lex_keyword(&mut self) {
        let (start, line, col) = self.mark();
        self.bump();
        while self.peek().is_some_and(is_symbol_char) {
            self.bump();
        }
        let end = self.pos;
        let name: Rc<str> = Rc::from(&self.src[start + 1..end]);
        self.emit(
            TokenKind::Keyword,
            start,
            line,
            col,
            end,
            Some(TokenValue::Keyword(name)),
        );
    }

    fn lex_string(&mut self) {
        let (start, line, col) = self.mark();
        self.bump();
        let mut value = String::new();
        loop {
            match self.peek() {
                None | Some('\n') => {
                    let end = self.pos;
                    self.emit_error(start, line, col, end, "文字列リテラルが閉じていません".to_string());
                    return;
                }
                Some('"') => {
                    self.bump();
                    let end = self.pos;
                    self.emit(
                        TokenKind::Str,
                        start,
                        line,
                        col,
                        end,
                        Some(TokenValue::Str(value)),
                    );
                    return;
                }
                Some('\\') => {
                    self.bump();
                    match self.peek() {
                        None | Some('\n') => {
                            let end = self.pos;
                            self.emit_error(
                                start,
                                line,
                                col,
                                end,
                                "文字列リテラルが閉じていません".to_string(),
                            );
                            return;
                        }
                        Some('u') => {
                            let hex: String = self.src[self.pos + 1..].chars().take(4).collect();
                            if hex.len() == 4 && hex.chars().all(is_hex) {
                                self.bump();
                                for _ in 0..4 {
                                    self.bump();
                                }
                                match u32::from_str_radix(&hex, 16)
                                    .ok()
                                    .and_then(char::from_u32)
                                {
                                    Some(c) => value.push(c),
                                    None => {
                                        let end = self.pos;
                                        self.emit_error(
                                            start,
                                            line,
                                            col,
                                            end,
                                            format!("無効なエスケープシーケンスです: \\u{hex}"),
                                        );
                                        return;
                                    }
                                }
                            } else {
                                self.bump();
                                let end = self.pos;
                                self.emit_error(
                                    start,
                                    line,
                                    col,
                                    end,
                                    "\\u エスケープには4桁の16進数が必要です".to_string(),
                                );
                                return;
                            }
                        }
                        Some(esc) => {
                            self.bump();
                            match esc {
                                '"' => value.push('"'),
                                '\\' => value.push('\\'),
                                'n' => value.push('\n'),
                                't' => value.push('\t'),
                                'r' => value.push('\r'),
                                other => {
                                    let end = self.pos;
                                    self.emit_error(
                                        start,
                                        line,
                                        col,
                                        end,
                                        format!("無効なエスケープシーケンスです: \\{other}"),
                                    );
                                    return;
                                }
                            }
                        }
                    }
                }
                Some(c) => {
                    value.push(c);
                    self.bump();
                }
            }
        }
    }

    /// char リテラル `\<文字>`（design/interp-capability.md §1.2）。
    /// エスケープ規則は str リテラルと共通（\" \\ \n \t \r \uXXXX）。
    /// `\` の次がエスケープならその文字、さもなくば次の1文字（改行・EOF は不可）。
    fn lex_char(&mut self) {
        let (start, line, col) = self.mark();
        self.bump(); // '\'
        let c = match self.peek() {
            None | Some('\n') => {
                let end = self.pos;
                self.emit_error(start, line, col, end, "char リテラルが閉じていません".to_string());
                return;
            }
            Some(c) => c,
        };
        // str リテラルと同じエスケープ規則を1文字に適用する（§1.2「共通化」）。
        let value = match c {
            'u' => {
                let hex: String = self.src[self.pos + 1..].chars().take(4).collect();
                if hex.len() == 4 && hex.chars().all(is_hex) {
                    self.bump();
                    for _ in 0..4 {
                        self.bump();
                    }
                    match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        Some(ch) => ch,
                        None => {
                            let end = self.pos;
                            self.emit_error(
                                start,
                                line,
                                col,
                                end,
                                format!("無効なエスケープシーケンスです: \\u{hex}（サロゲートは存在しません）"),
                            );
                            return;
                        }
                    }
                } else {
                    self.bump();
                    let end = self.pos;
                    self.emit_error(start, line, col, end, "\\u エスケープには4桁の16進数が必要です".to_string());
                    return;
                }
            }
            '"' | '\\' | 'n' | 't' | 'r' => {
                self.bump();
                match c {
                    '"' => '"',
                    '\\' => '\\',
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    _ => unreachable!(),
                }
            }
            other => {
                self.bump();
                other
            }
        };
        let end = self.pos;
        self.emit(
            TokenKind::Char,
            start,
            line,
            col,
            end,
            Some(TokenValue::Char(value)),
        );
    }

    fn scan_name(&mut self) -> (usize, usize) {
        let start = self.pos;
        // 名前中で許す演算子由来の接尾辞（design/interp-capability.md §3.1 の char<=? / char=? 等）。
        // 単独の '<' '>' '=' は従来通り演算子（Punct）として扱う。
        const OP_SUFFIXES: &[&str] = &["<=?", ">=?", "=?", "<?", ">?"];
        while let Some(c) = self.peek() {
            if is_symbol_char(c) {
                self.bump();
            } else if c == '>'
                && self.src[..self.pos].ends_with('-')
                && self.peek_second().is_some_and(is_symbol_char)
            {
                // 名前中の "->" はシンボルの一部（design §1.1 の obj->map / set->array 等）。
                // 単独の '->' は list 先頭のパイプなので is_symbol_start('-') == false のため
                // 従来通り Punct::Arrow になる。
                self.bump();
            } else if let Some(s) = OP_SUFFIXES
                .iter()
                .find(|s| self.src[self.pos..].starts_with(**s))
            {
                for _ in 0..s.chars().count() {
                    self.bump();
                }
            } else {
                break;
            }
        }
        (start, self.pos)
    }

    fn lex_symbol(&mut self) {
        let (start, line, col) = self.mark();
        let (ns, ne) = self.scan_name();
        let first: Rc<str> = Rc::from(&self.src[ns..ne]);
        let mut segments: Vec<Rc<str>> = vec![first.clone()];
        let mut dotted = false;
        while self.peek() == Some('.') && self.peek_second().is_some_and(is_symbol_start) {
            dotted = true;
            self.bump();
            let (s, e) = self.scan_name();
            segments.push(Rc::from(&self.src[s..e]));
        }
        if self.peek() == Some('!') {
            if dotted {
                self.bump();
                let end = self.pos;
                self.emit_error(
                    start,
                    line,
                    col,
                    end,
                    "ドットシンボルの末尾に '!' は使用できません".to_string(),
                );
                return;
            }
            self.bump();
            if self.peek() == Some('.') {
                self.bump();
                let end = self.pos;
                self.emit_error(
                    start,
                    line,
                    col,
                    end,
                    "'!' を含むシンボルをドットで連結することはできません".to_string(),
                );
                return;
            }
            let end = self.pos;
            self.emit(
                TokenKind::Symbol,
                start,
                line,
                col,
                end,
                Some(TokenValue::Symbol {
                    name: first,
                    effect: true,
                }),
            );
            return;
        }
        let end = self.pos;
        if !dotted {
            if &*first == "nil" {
                self.emit(TokenKind::Nil, start, line, col, end, None);
                return;
            }
            if &*first == "true" || &*first == "false" {
                let b = &*first == "true";
                self.emit(TokenKind::Bool, start, line, col, end, Some(TokenValue::Bool(b)));
                return;
            }
        }
        if dotted {
            self.emit(
                TokenKind::DottedSymbol,
                start,
                line,
                col,
                end,
                Some(TokenValue::Dotted(segments)),
            );
        } else {
            self.emit(
                TokenKind::Symbol,
                start,
                line,
                col,
                end,
                Some(TokenValue::Symbol {
                    name: first,
                    effect: false,
                }),
            );
        }
    }

    fn run(&mut self) {
        while let Some(c) = self.peek() {
            if c == ' ' || c == '\t' || c == '\n' || c == '\r' {
                self.bump();
            } else if c == ';' {
                self.lex_comment();
            } else if c == '(' {
                self.lex_one(TokenKind::OpenParen);
            } else if c == ')' {
                self.lex_one(TokenKind::CloseParen);
            } else if c == '[' {
                self.lex_one(TokenKind::OpenBracket);
            } else if c == ']' {
                self.lex_one(TokenKind::CloseBracket);
            } else if c == '{' {
                self.lex_one(TokenKind::OpenBrace);
            } else if c == '}' {
                self.lex_one(TokenKind::CloseBrace);
            } else if c == '"' {
                self.lex_string();
            } else if c == '\\' {
                self.lex_char();
            } else if is_digit(c) || (c == '-' && self.peek_second().is_some_and(is_digit)) {
                self.lex_number();
            } else if c == ':' && self.peek_second().is_some_and(is_symbol_start) {
                self.lex_keyword();
            } else if self.lex_punct() {
            } else if is_symbol_start(c) {
                self.lex_symbol();
            } else {
                let (start, line, col) = self.mark();
                if let Some(ch) = self.bump() {
                    let end = self.pos;
                    self.emit_error(start, line, col, end, format!("不正な文字です: '{ch}'"));
                }
            }
        }
    }
}

pub fn normalize_source(src: &str) -> Cow<'_, str> {
    let s = src.strip_prefix('\u{feff}').unwrap_or(src);
    if s.contains("\r\n") {
        Cow::Owned(s.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(s)
    }
}

pub fn tokenize(src: &str, file: &str) -> LexResult {
    let text = normalize_source(src);
    let mut lx = Lexer::new(&text, file);
    lx.run();
    LexResult {
        tokens: lx.tokens,
        diagnostics: lx.diagnostics,
        trailing: Trivia { leading: lx.pending },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::span_of;

    fn lex(src: &str) -> LexResult {
        tokenize(src, "test.lts")
    }

    fn kinds(src: &str) -> Vec<TokenKind> {
        lex(src).tokens.iter().map(|t| t.kind).collect()
    }

    #[test]
    fn parens_and_operands() {
        let r = lex("(+ 1 2)");
        assert!(r.diagnostics.is_empty());
        assert_eq!(
            kinds("(+ 1 2)"),
            vec![
                TokenKind::OpenParen,
                TokenKind::Punct,
                TokenKind::Number,
                TokenKind::Number,
                TokenKind::CloseParen
            ]
        );
        let texts: Vec<&str> = r.tokens.iter().map(|t| &*t.text).collect();
        assert_eq!(texts, vec!["(", "+", "1", "2", ")"]);
        assert_eq!(
            r.tokens[1].value,
            Some(TokenValue::Punct(Punct::Plus))
        );
        assert_eq!(r.tokens[2].value, Some(TokenValue::Num(1.0)));
        assert_eq!(r.tokens[3].value, Some(TokenValue::Num(2.0)));
    }

    #[test]
    fn annotation_colon_is_punct() {
        let r = lex("(a : Money)");
        assert!(r.diagnostics.is_empty());
        assert_eq!(
            kinds("(a : Money)"),
            vec![
                TokenKind::OpenParen,
                TokenKind::Symbol,
                TokenKind::Punct,
                TokenKind::Symbol,
                TokenKind::CloseParen
            ]
        );
        assert_eq!(
            r.tokens[2].value,
            Some(TokenValue::Punct(Punct::Colon))
        );
        assert_eq!(
            r.tokens[1].value,
            Some(TokenValue::Symbol {
                name: Rc::from("a"),
                effect: false
            })
        );
        assert_eq!(
            r.tokens[3].value,
            Some(TokenValue::Symbol {
                name: Rc::from("Money"),
                effect: false
            })
        );
    }

    #[test]
    fn symbol_kebab_and_question() {
        for src in ["add-money", "contains?"] {
            let r = lex(src);
            assert!(r.diagnostics.is_empty());
            assert_eq!(r.tokens.len(), 1);
            assert_eq!(r.tokens[0].kind, TokenKind::Symbol);
            assert_eq!(
                r.tokens[0].value,
                Some(TokenValue::Symbol {
                    name: Rc::from(src),
                    effect: false
                })
            );
            assert_eq!(&*r.tokens[0].text, src);
        }
    }

    #[test]
    fn symbol_effect_suffix() {
        let r = lex("db!");
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(r.tokens[0].kind, TokenKind::Symbol);
        assert_eq!(&*r.tokens[0].text, "db!");
        assert_eq!(
            r.tokens[0].value,
            Some(TokenValue::Symbol {
                name: Rc::from("db"),
                effect: true
            })
        );
    }

    #[test]
    fn decimal_suffix_d_makes_dec_token() {
        let r = lex("1.50d 2 1d xd 1.5");
        let kinds: Vec<TokenKind> = r.tokens.iter().map(|t| t.kind).collect();
        assert_eq!(kinds[0], TokenKind::Dec);
        assert_eq!(r.tokens[0].value, Some(TokenValue::Dec(Rc::from("1.50"))));
        assert_eq!(kinds[2], TokenKind::Dec, "1d も decimal");
        assert_eq!(r.tokens[4].value, Some(TokenValue::Num(1.5)), "1.5 は number のまま");
        // xd はシンボルのまま
        let r2 = lex("xd");
        assert_eq!(r2.tokens[0].kind, TokenKind::Symbol);
    }

    #[test]
    fn negative_number_vs_minus_punct() {
        let r = lex("-5");
        assert!(r.diagnostics.is_empty());
        assert_eq!(kinds("-5"), vec![TokenKind::Number]);
        assert_eq!(r.tokens[0].value, Some(TokenValue::Num(-5.0)));
        assert_eq!(&*r.tokens[0].text, "-5");

        let r = lex("- 5");
        assert!(r.diagnostics.is_empty());
        assert_eq!(
            kinds("- 5"),
            vec![TokenKind::Punct, TokenKind::Number]
        );
        assert_eq!(
            r.tokens[0].value,
            Some(TokenValue::Punct(Punct::Minus))
        );
        assert_eq!(r.tokens[1].value, Some(TokenValue::Num(5.0)));
    }

    #[test]
    fn number_forms() {
        let r = lex("1.50");
        assert_eq!(r.tokens[0].kind, TokenKind::Number);
        assert_eq!(&*r.tokens[0].text, "1.50");
        assert_eq!(r.tokens[0].value, Some(TokenValue::Num(1.5)));

        let r = lex("1E5");
        assert_eq!(&*r.tokens[0].text, "1E5");
        assert_eq!(r.tokens[0].value, Some(TokenValue::Num(100000.0)));

        let r = lex("2.5e-3 -0.5E+2");
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.tokens.len(), 2);
        assert_eq!(r.tokens[0].value, Some(TokenValue::Num(0.0025)));
        assert_eq!(r.tokens[1].value, Some(TokenValue::Num(-50.0)));
        assert_eq!(&*r.tokens[1].text, "-0.5E+2");
    }

    #[test]
    fn number_edge_cases() {
        let r = lex("1.");
        assert_eq!(kinds("1."), vec![TokenKind::Number, TokenKind::Error]);
        assert_eq!(r.tokens[0].value, Some(TokenValue::Num(1.0)));
        assert_eq!(r.diagnostics.len(), 1);

        let r = lex("1e");
        assert_eq!(kinds("1e"), vec![TokenKind::Number, TokenKind::Symbol]);
        assert!(r.diagnostics.is_empty());

        let r = lex("+5");
        assert_eq!(
            kinds("+5"),
            vec![TokenKind::Punct, TokenKind::Number]
        );
        assert_eq!(r.tokens[0].value, Some(TokenValue::Punct(Punct::Plus)));
        assert!(r.diagnostics.is_empty());
    }

    #[test]
    fn dotted_symbol() {
        let r = lex("x.y.z");
        assert!(r.diagnostics.is_empty());
        assert_eq!(kinds("x.y.z"), vec![TokenKind::DottedSymbol]);
        assert_eq!(
            r.tokens[0].value,
            Some(TokenValue::Dotted(vec![
                Rc::from("x"),
                Rc::from("y"),
                Rc::from("z")
            ]))
        );
        assert_eq!(&*r.tokens[0].text, "x.y.z");
    }

    #[test]
    fn dotted_symbol_with_bang_is_error() {
        let r = lex("o.f!");
        assert_eq!(kinds("o.f!"), vec![TokenKind::Error]);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].kind, DiagKind::ParseError);
        assert_eq!(r.diagnostics[0].severity, Severity::Error);

        let r = lex("f!.a");
        assert_eq!(r.tokens[0].kind, TokenKind::Error);
        assert_eq!(r.diagnostics.len(), 1);
        assert!(r.diagnostics[0].message.contains("ドットで連結"));

        assert_eq!(kinds("a."), vec![TokenKind::Symbol, TokenKind::Error]);
    }

    #[test]
    fn keyword_token() {
        let r = lex(":else");
        assert!(r.diagnostics.is_empty());
        assert_eq!(kinds(":else"), vec![TokenKind::Keyword]);
        assert_eq!(&*r.tokens[0].text, ":else");
        assert_eq!(
            r.tokens[0].value,
            Some(TokenValue::Keyword(Rc::from("else")))
        );

        let r = lex(": 5");
        assert_eq!(kinds(": 5"), vec![TokenKind::Punct, TokenKind::Number]);
        assert_eq!(r.tokens[0].value, Some(TokenValue::Punct(Punct::Colon)));

        assert_eq!(kinds(":5"), vec![TokenKind::Punct, TokenKind::Number]);
    }

    #[test]
    fn string_escapes() {
        let r = lex("\"a\\n\"");
        assert!(r.diagnostics.is_empty());
        assert_eq!(kinds("\"a\\n\""), vec![TokenKind::Str]);
        assert_eq!(&*r.tokens[0].text, "\"a\\n\"");
        assert_eq!(
            r.tokens[0].value,
            Some(TokenValue::Str("a\n".to_string()))
        );

        let r = lex("\"\\t\\\"\\\\\\r\"");
        assert_eq!(r.tokens[0].value, Some(TokenValue::Str("\t\"\\\r".to_string())));

        let r = lex("\"\\u0041\"");
        assert_eq!(r.tokens[0].value, Some(TokenValue::Str("A".to_string())));
    }

    #[test]
    fn string_errors() {
        let r = lex("\"abc");
        assert_eq!(kinds("\"abc"), vec![TokenKind::Error]);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].kind, DiagKind::ParseError);

        let r = lex("\"a\nb\"");
        assert_eq!(r.tokens[0].kind, TokenKind::Error);
        assert_eq!(r.diagnostics[0].kind, DiagKind::ParseError);

        let r = lex("\"a\\x\"");
        assert_eq!(r.tokens[0].kind, TokenKind::Error);
        assert_eq!(r.diagnostics[0].kind, DiagKind::ParseError);
        assert!(r.diagnostics[0].message.contains("無効なエスケープシーケンス"));

        let r = lex("\"\\u12g4\"");
        assert_eq!(r.tokens[0].kind, TokenKind::Error);
        assert_eq!(r.diagnostics[0].kind, DiagKind::ParseError);
        assert!(r.diagnostics[0].message.contains("4桁の16進数"));
    }

    #[test]
    fn nil_and_bool() {
        assert_eq!(kinds("nil"), vec![TokenKind::Nil]);
        assert_eq!(kinds("true false"), vec![TokenKind::Bool, TokenKind::Bool]);
        let r = lex("true false");
        assert_eq!(r.tokens[0].value, Some(TokenValue::Bool(true)));
        assert_eq!(r.tokens[1].value, Some(TokenValue::Bool(false)));

        let r = lex("nilx nil.x");
        assert_eq!(
            kinds("nilx nil.x"),
            vec![TokenKind::Symbol, TokenKind::DottedSymbol]
        );
        assert_eq!(
            r.tokens[0].value,
            Some(TokenValue::Symbol {
                name: Rc::from("nilx"),
                effect: false
            })
        );
    }

    #[test]
    fn unclosed_paren_is_not_lexer_error() {
        let r = lex("(+ 1 2");
        assert!(r.diagnostics.is_empty());
        assert_eq!(
            kinds("(+ 1 2"),
            vec![
                TokenKind::OpenParen,
                TokenKind::Punct,
                TokenKind::Number,
                TokenKind::Number
            ]
        );
    }

    #[test]
    fn all_delimiters_and_puncts() {
        let r = lex("()[]{}");
        assert_eq!(
            r.tokens.iter().map(|t| t.kind).collect::<Vec<_>>(),
            vec![
                TokenKind::OpenParen,
                TokenKind::CloseParen,
                TokenKind::OpenBracket,
                TokenKind::CloseBracket,
                TokenKind::OpenBrace,
                TokenKind::CloseBrace
            ]
        );

        let src = ": + - * / < > = <= >= -> =>";
        let r = lex(src);
        assert!(r.diagnostics.is_empty());
        let expected = [
            Punct::Colon,
            Punct::Plus,
            Punct::Minus,
            Punct::Star,
            Punct::Slash,
            Punct::Lt,
            Punct::Gt,
            Punct::Eq,
            Punct::Le,
            Punct::Ge,
            Punct::Arrow,
            Punct::FatArrow,
        ];
        assert_eq!(r.tokens.len(), expected.len());
        for (t, p) in r.tokens.iter().zip(expected.iter()) {
            assert_eq!(t.kind, TokenKind::Punct);
            assert_eq!(t.value, Some(TokenValue::Punct(*p)));
            assert_eq!(&*t.text, p.as_str());
        }
    }

    #[test]
    fn longest_match_puncts() {
        let r = lex("<=>");
        assert_eq!(
            r.tokens.iter().map(|t| t.value.clone()).collect::<Vec<_>>(),
            vec![
                Some(TokenValue::Punct(Punct::Le)),
                Some(TokenValue::Punct(Punct::Gt))
            ]
        );
        assert!(r.diagnostics.is_empty());

        let r = lex("-> =>");
        assert_eq!(
            r.tokens.iter().map(|t| t.value.clone()).collect::<Vec<_>>(),
            vec![
                Some(TokenValue::Punct(Punct::Arrow)),
                Some(TokenValue::Punct(Punct::FatArrow))
            ]
        );
        assert_eq!(&*r.tokens[0].text, "->");
        assert_eq!(&*r.tokens[1].text, "=>");
    }

    #[test]
    fn char_literals() {
        // design/interp-capability.md §1.2: \<文字>。エスケープは str と共通。
        let r = lex("\\a \\n \\u0041 \\😀");
        let vals: Vec<Option<TokenValue>> = r.tokens.iter().map(|t| t.value.clone()).collect();
        assert_eq!(
            vals,
            vec![
                Some(TokenValue::Char('a')),
                Some(TokenValue::Char('\n')),
                Some(TokenValue::Char('A')),
                Some(TokenValue::Char('😀')),
            ]
        );
        assert!(r.diagnostics.is_empty());
        // サロゲートは char になれない
        let r = lex("\\uD800");
        assert!(!r.diagnostics.is_empty());
        // 演算子接尾辞を含む名前（§3.1: char<=? / char=?）
        let r = lex("(char<=? a b) (char=? a b) (<= a b) (= a b)");
        let texts: Vec<&str> = r.tokens.iter().map(|t| &*t.text).collect();
        assert_eq!(
            texts,
            ["(", "char<=?", "a", "b", ")", "(", "char=?", "a", "b", ")", "(", "<=", "a", "b", ")", "(", "=", "a", "b", ")"]
        );
        assert!(r.diagnostics.is_empty());
    }

    #[test]
    fn arrow_inside_symbol_is_part_of_name() {
        // design/interp-capability.md §1.1: obj->map / set->array は単一シンボル
        let r = lex("(obj->map o) (set->array s) (-> a b)");
        let texts: Vec<&str> = r.tokens.iter().map(|t| &*t.text).collect();
        assert_eq!(
            texts,
            ["(", "obj->map", "o", ")", "(", "set->array", "s", ")", "(", "->", "a", "b", ")"]
        );
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.tokens[9].value, Some(TokenValue::Punct(Punct::Arrow)));
    }

    #[test]
    fn whitespace_only() {
        let r = lex(" \t\n\r ");
        assert!(r.tokens.is_empty());
        assert!(r.diagnostics.is_empty());
    }

    #[test]
    fn comment_attaches_to_next_token() {
        let r = lex(";; c\n(defn f)");
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.tokens[0].kind, TokenKind::OpenParen);
        assert_eq!(r.tokens[0].trivia.leading.len(), 1);
        assert_eq!(&*r.tokens[0].trivia.leading[0].text, ";; c");
        let cs = &r.tokens[0].trivia.leading[0].span;
        assert_eq!((cs.start, cs.end, cs.line, cs.col), (0, 4, 1, 1));
        assert_eq!(r.tokens[1].kind, TokenKind::Symbol);
        assert_eq!(&*r.tokens[1].text, "defn");
        assert!(r.tokens[1].trivia.leading.is_empty());

        let r = lex("(;; c\ndefn f)");
        assert!(r.tokens[0].trivia.leading.is_empty());
        assert_eq!(r.tokens[1].kind, TokenKind::Symbol);
        assert_eq!(&*r.tokens[1].text, "defn");
        assert_eq!(r.tokens[1].trivia.leading.len(), 1);
        assert_eq!(&*r.tokens[1].trivia.leading[0].text, ";; c");
    }

    #[test]
    fn multiple_comments_accumulate_and_trailing_returned() {
        let r = lex("; a\n; b\nx ;; end");
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(
            r.tokens[0]
                .trivia
                .leading
                .iter()
                .map(|c| &*c.text)
                .collect::<Vec<_>>(),
            vec!["; a", "; b"]
        );
        assert_eq!(r.trailing.leading.len(), 1);
        assert_eq!(&*r.trailing.leading[0].text, ";; end");

        let r = lex(";; only\n");
        assert!(r.tokens.is_empty());
        assert_eq!(r.trailing.leading.len(), 1);
    }

    #[test]
    fn error_token_carries_leading_comment_and_lexing_continues() {
        let r = lex("; c\n@ x");
        assert_eq!(kinds("; c\n@ x"), vec![TokenKind::Error, TokenKind::Symbol]);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.tokens[0].kind, TokenKind::Error);
        assert_eq!(r.tokens[0].trivia.leading.len(), 1);
        assert!(r.diagnostics[0].message.contains("不正な文字"));
    }

    #[test]
    fn invalid_multibyte_char_is_one_error_token() {
        let r = lex("λ");
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(r.tokens[0].kind, TokenKind::Error);
        assert_eq!(r.tokens[0].span.start, 0);
        assert_eq!(r.tokens[0].span.end, 2);
        assert_eq!(r.diagnostics.len(), 1);
    }

    #[test]
    fn spans_are_byte_positions_with_char_based_col() {
        let r = lex("(+ 1 2)");
        let spans: Vec<(usize, usize, usize, usize)> = r
            .tokens
            .iter()
            .map(|t| (t.span.start, t.span.end, t.span.line, t.span.col))
            .collect();
        assert_eq!(
            spans,
            vec![(0, 1, 1, 1), (1, 2, 1, 2), (3, 4, 1, 4), (5, 6, 1, 6), (6, 7, 1, 7)]
        );

        let r = lex("a é");
        assert_eq!(r.tokens[0].kind, TokenKind::Symbol);
        assert_eq!(r.tokens[1].kind, TokenKind::Error);
        assert_eq!(r.tokens[1].span.start, 2);
        assert_eq!(r.tokens[1].span.end, 4);
        assert_eq!(r.tokens[1].span.line, 1);
        assert_eq!(r.tokens[1].span.col, 3);
    }

    #[test]
    fn multiline_spans() {
        let r = lex("(\n  x)");
        assert_eq!(r.tokens[0].span.line, 1);
        assert_eq!(r.tokens[0].span.col, 1);
        assert_eq!((r.tokens[1].span.start, r.tokens[1].span.line, r.tokens[1].span.col), (4, 2, 3));
        assert_eq!((r.tokens[2].span.start, r.tokens[2].span.line, r.tokens[2].span.col), (5, 2, 4));
    }

    #[test]
    fn crlf_and_bom() {
        let r = lex("(\r\n)");
        assert!(r.diagnostics.is_empty());
        assert_eq!((r.tokens[1].span.start, r.tokens[1].span.line, r.tokens[1].span.col), (2, 2, 1));

        let r = lex("\u{feff}(+ 1 2)");
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.tokens.len(), 5);
        assert_eq!(r.tokens[0].span.start, 0);
        assert_eq!((r.tokens[0].span.line, r.tokens[0].span.col), (1, 1));
    }

    #[test]
    fn token_spans_match_span_of() {
        let src = "(defn f (x : number) : number ; cmt\n  (+ x 1))\n;; tail";
        let r = lex(src);
        assert!(r.diagnostics.is_empty());
        for t in &r.tokens {
            let s = &t.span;
            assert_eq!(
                *s,
                span_of(Rc::from("test.lts"), src, s.start, s.end)
            );
        }
        for c in &r.trailing.leading {
            let s = &c.span;
            assert_eq!(*s, span_of(Rc::from("test.lts"), src, s.start, s.end));
        }
    }
}
