use crate::ast::{
    Expr, LitValue, Param, Pat, PatField, Prim, Program, TopForm, TypeExpr, TypeField,
};
use crate::diagnostic::Diagnostic;
use crate::lexer::tokenize;
use crate::parser::parse_program;
use crate::token::Trivia;

pub struct FormatResult {
    pub output: String,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn format_source(src: &str, file: &str) -> FormatResult {
    let lex = tokenize(src, file);
    let (program, diagnostics) = parse_program(&lex);
    if !diagnostics.is_empty() {
        return FormatResult {
            output: src.to_string(),
            diagnostics,
        };
    }
    FormatResult {
        output: format_program(&program),
        diagnostics: Vec::new(),
    }
}

pub fn format_program(p: &Program) -> String {
    let mut parts: Vec<String> = p.forms.iter().map(|f| print_top_form(f, "")).collect();
    for c in &p.trailing.leading {
        parts.push(c.text.to_string());
    }
    format!("{}\n", parts.join("\n"))
}

fn comments(trivia: &Trivia, ind: &str) -> String {
    if trivia.leading.is_empty() {
        return String::new();
    }
    let mut s = String::new();
    for c in &trivia.leading {
        s += &format!("{ind}{}\n", c.text);
    }
    s
}

/// char リテラルの印字（`\<文字>`）。エスケープは str と共通規則の逆写像（design §1.2）。
pub fn print_char_lit(c: char) -> String {
    let bs = 92u8 as char; // backslash
    let mut out = String::new();
    out.push(bs);
    match c {
        '"' => out.push('"'),
        '\u{5c}' => out.push(bs),
        '\u{a}' => out.push('n'),
        '\u{9}' => out.push('t'),
        '\u{d}' => out.push('r'),
        c if (c as u32) < 0x20 || c as u32 == 0x7f => {
            out.push('u');
            out += &format!("{:04x}", c as u32);
        }
        c => out.push(c),
    }
    out
}

pub fn print_string_lit(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            '\n' => out += "\\n",
            '\t' => out += "\\t",
            '\r' => out += "\\r",
            c if (c as u32) < 0x20 => {
                out += &format!("\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn print_top_form(f: &TopForm, ind: &str) -> String {
    match f {
        TopForm::Import {
            path,
            names,
            trivia,
            ..
        } => format!(
            "{}(import {} ({}))",
            comments(trivia, ind),
            print_string_lit(path),
            names.iter().map(|n| n.as_ref()).collect::<Vec<_>>().join(" ")
        ),
        TopForm::Export { names, trivia, .. } => format!(
            "{}(export {})",
            comments(trivia, ind),
            names.iter().map(|n| n.as_ref()).collect::<Vec<_>>().join(" ")
        ),
        TopForm::TypeDef {
            name,
            ty,
            trivia,
            ..
        } => format!("{}(type {name} {})", comments(trivia, ind), print_type(ty)),
        TopForm::EffectDef {
            name,
            ty,
            trivia,
            ..
        } => format!("{}(effect {name} : {})", comments(trivia, ind), print_type(ty)),
        TopForm::Defn {
            effectful,
            name,
            params,
            ret,
            body,
            trivia,
            ..
        } => {
            let params_s = if params.is_empty() {
                " ()".to_string()
            } else {
                format!(" {}", params.iter().map(print_param).collect::<Vec<_>>().join(" "))
            };
            let head = format!(
                "{}(defn{} {name}{params_s} : {}",
                comments(trivia, ind),
                if *effectful { "!" } else { "" },
                print_type(ret)
            );
            format!("{head}\n{ind}  {})", print_expr(body, &format!("{ind}  ")))
        }
        TopForm::Prop {
            name,
            params,
            body,
            trivia,
            ..
        } => {
            let quant = format!(
                "[{}]",
                params.iter().map(print_param).collect::<Vec<_>>().join(" ")
            );
            format!(
                "{}(prop {} (forall {} {}))",
                comments(trivia, ind),
                print_string_lit(name),
                quant,
                print_expr(body, &format!("{ind}  "))
            )
        }
        TopForm::Example {
            actual,
            expected,
            trivia,
            ..
        } => format!(
            "{}(example {}\n{}  => {})",
            comments(trivia, ind),
            print_expr(actual, &format!("{ind}  ")),
            ind,
            print_expr(expected, &format!("{ind}  "))
        ),
        TopForm::Error { trivia, .. } => comments(trivia, ind),
    }
}

fn print_expr(e: &Expr, ind: &str) -> String {
    let c = match e {
        Expr::Lit { trivia, .. }
        | Expr::Sym { trivia, .. }
        | Expr::FieldAccess { trivia, .. }
        | Expr::App { trivia, .. }
        | Expr::Let { trivia, .. }
        | Expr::Letrec { trivia, .. }
        | Expr::If { trivia, .. }
        | Expr::When { trivia, .. }
        | Expr::Cond { trivia, .. }
        | Expr::And { trivia, .. }
        | Expr::Or { trivia, .. }
        | Expr::Match { trivia, .. }
        | Expr::Do { trivia, .. }
        | Expr::Obj { trivia, .. }
        | Expr::Fn { trivia, .. }
        | Expr::Pipe { trivia, .. }
        | Expr::Error { trivia, .. } => comments(trivia, ind),
    };
    let body = match e {
        Expr::Lit { value, .. } => match value {
            LitValue::Str(s) => print_string_lit(s),
            LitValue::Char(c) => print_char_lit(*c),
            LitValue::Nil => "nil".to_string(),
            LitValue::Bool(b) => format!("{b}"),
            LitValue::Num(n) => format!("{n}"),
            LitValue::Dec(s) => format!("{s}d"),
        },
        Expr::Sym { name, .. } => name.to_string(),
        Expr::FieldAccess { parts, .. } => parts
            .iter()
            .map(|p| p.as_ref())
            .collect::<Vec<_>>()
            .join("."),
        Expr::App {
            name, args, ..
        } => join_list(name, &args.iter().map(|a| print_expr(a, ind)).collect::<Vec<_>>(), ind),
        Expr::Let { binds, body, .. } => {
            let binds_s: Vec<String> = binds
                .iter()
                .map(|b| format!("{} {}", b.name, print_expr(&b.expr, ind)))
                .collect();
            join_list("let", &[format!("[{}]", binds_s.join(" ")), print_expr(body, ind)], ind)
        }
        Expr::Letrec {
            name,
            lam,
            body,
            ..
        } => {
            let mut parts = vec![format!("[{name} {}]", print_expr(lam, ind))];
            parts.extend(body.iter().map(|e| print_expr(e, ind)));
            join_list("letrec", &parts, ind)
        }
        Expr::If {
            cond,
            then,
            else_,
            ..
        } => join_list(
            "if",
            &[print_expr(cond, ind), print_expr(then, ind), print_expr(else_, ind)],
            ind,
        ),
        Expr::When { cond, body, .. } => {
            join_list("when", &[print_expr(cond, ind), print_expr(body, ind)], ind)
        }
        Expr::And { left, right, .. } => {
            join_list("and", &[print_expr(left, ind), print_expr(right, ind)], ind)
        }
        Expr::Or { left, right, .. } => {
            join_list("or", &[print_expr(left, ind), print_expr(right, ind)], ind)
        }
        Expr::Cond {
            arms, else_, ..
        } => {
            let arms_s: Vec<String> = arms
                .iter()
                .map(|a| format!("[{ind}  {} {}]", print_expr(&a.cond, ind), print_expr(&a.body, ind)))
                .collect();
            format!(
                "(cond\n{}\n{ind}  [:else {}]\n{ind})",
                arms_s.join("\n"),
                print_expr(else_, ind)
            )
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            let arms_s: Vec<String> = arms
                .iter()
                .map(|a| {
                    format!(
                        "{}{}  [{} {}]",
                        comments(&a.trivia, &format!("{ind}  ")),
                        ind,
                        print_pattern(&a.pattern),
                        print_expr(&a.body, &format!("{ind}    "))
                    )
                })
                .collect();
            format!("(match {}\n{}\n{ind})", print_expr(scrutinee, ind), arms_s.join("\n"))
        }
        Expr::Do { binds, body, .. } => {
            let binds_s: Vec<String> = binds
                .iter()
                .map(|b| format!("{ind}  [{} {}]", b.name, print_expr(&b.expr, &format!("{ind}    "))))
                .collect();
            format!("(do\n{}\n{ind}  {}\n{ind})", binds_s.join("\n"), print_expr(body, &format!("{ind}  ")))
        }
        Expr::Obj { fields, .. } => join_list(
            "obj",
            &fields
                .iter()
                .map(|f| format!("({} {})", f.name, print_expr(&f.expr, ind)))
                .collect::<Vec<_>>(),
            ind,
        ),
        Expr::Fn {
            params,
            ret,
            body,
            ..
        } => {
            let params_s = if params.is_empty() {
                " ()".to_string()
            } else {
                format!(" {}", params.iter().map(print_param).collect::<Vec<_>>().join(" "))
            };
            join_list(
                &format!("fn{params_s} : {}", print_type(ret)),
                &[print_expr(body, ind)],
                ind,
            )
        }
        Expr::Pipe { exprs, .. } => {
            join_list("->", &exprs.iter().map(|x| print_expr(x, ind)).collect::<Vec<_>>(), ind)
        }
        Expr::Error { message, .. } => format!("({message})"),
    };
    format!("{c}{body}")
}

fn join_list(head: &str, args: &[String], ind: &str) -> String {
    if args.iter().all(|a| !a.contains('\n')) {
        return format!("({head} {})", args.join(" "));
    }
    format!(
        "({head}\n{}\n{ind})",
        args.iter().map(|a| format!("{ind}  {a}")).collect::<Vec<_>>().join("\n")
    )
}

fn print_param(p: &Param) -> String {
    format!("({} : {})", p.name, print_type(&p.ty))
}

fn print_type(t: &TypeExpr) -> String {
    match t {
        TypeExpr::Prim { name, .. } => match name {
            Prim::Number => "number",
            Prim::Dec => "decimal",
            Prim::Str => "string",
            Prim::Bool => "bool",
            Prim::Char => "char",
            Prim::Builder => "builder",
            Prim::Bytes => "bytes",
            Prim::Nil => "nil",
        }
        .to_string(),
        TypeExpr::Alias { name, .. } => name.to_string(),
        TypeExpr::Array { element, .. } => format!("(Array {})", print_type(element)),
        TypeExpr::Union { members, .. } => format!(
            "(union {})",
            members.iter().map(print_type).collect::<Vec<_>>().join(" ")
        ),
        TypeExpr::Obj { fields, .. } => format!(
            "(obj {})",
            fields
                .iter()
                .map(|f| match f {
                    TypeField::Field { name, ty, .. } => format!("({name} : {})", print_type(ty)),
                    TypeField::Tag { name, value, .. } => format!("({name} {})", print_string_lit(value)),
                })
                .collect::<Vec<_>>()
                .join(" ")
        ),
        TypeExpr::Map { key, value, .. } => format!("(map {} {})", print_type(key), print_type(value)),
        TypeExpr::Set { key, .. } => format!("(set {})", print_type(key)),
        TypeExpr::Ref { inner, .. } => format!("(ref {})", print_type(inner)),
        TypeExpr::ResultT { ok, err, .. } => {
            format!("(Result {} {})", print_type(ok), print_type(err))
        }
        TypeExpr::FnT { params, ret, .. } => {
            let params_s = if params.is_empty() {
                " ()".to_string()
            } else {
                format!(" {}", params.iter().map(print_param).collect::<Vec<_>>().join(" "))
            };
            format!("(fn{params_s} : {})", print_type(ret))
        }
        TypeExpr::Error { .. } => "_".to_string(),
    }
}

fn print_pattern(p: &Pat) -> String {
    match p {
        Pat::Wildcard { .. } => "_".to_string(),
        Pat::Var { name, .. } => name.to_string(),
        Pat::Lit { value, .. } => match value {
            LitValue::Str(s) => print_string_lit(s),
            LitValue::Char(c) => print_char_lit(*c),
            LitValue::Nil => "nil".to_string(),
            LitValue::Bool(b) => format!("{b}"),
            LitValue::Num(n) => format!("{n}"),
            LitValue::Dec(s) => format!("{s}d"),
        },
        Pat::Obj { fields, .. } => format!(
            "(obj {})",
            fields
                .iter()
                .map(|f: &PatField| format!("({} {})", f.name, print_pattern(&f.pattern)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Pat::Array { elements, .. } => format!(
            "(Array {})",
            elements.iter().map(print_pattern).collect::<Vec<_>>().join(" ")
        ),
        Pat::Or { patterns, .. } => format!(
            "(or {})",
            patterns.iter().map(print_pattern).collect::<Vec<_>>().join(" ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as Json;

    fn fixture_src(name: &str) -> String {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&path).expect("fixture readable")
    }

    fn strip_spans(v: Json) -> Json {
        match v {
            Json::Object(m) => Json::Object(
                m.into_iter()
                    .filter(|(k, _)| k != "span")
                    .map(|(k, v)| (k, strip_spans(v)))
                    .collect(),
            ),
            Json::Array(a) => Json::Array(a.into_iter().map(strip_spans).collect()),
            other => other,
        }
    }

    fn ast_json(src: &str, name: &str) -> Json {
        let lex = tokenize(src, name);
        let (program, diags) = parse_program(&lex);
        assert!(diags.is_empty(), "{diags:?}");
        strip_spans(serde_json::to_value(&program).expect("ser"))
    }

    #[test]
    fn fmt_is_idempotent_on_fixtures() {
        for name in ["money.lts", "cart.lts", "stock.lts", "effects.lts", "cycle-a.lts", "fact.lts", "map-edge.lts", "char-edge.lts", "letrec-ref-builder.lts", "hostfx.lts", "bytes-edge.lts", "props-edge.lts"] {
            let src = fixture_src(name);
            let once = format_source(&src, name);
            assert!(once.diagnostics.is_empty(), "{name}: {:?}", once.diagnostics);
            let twice = format_source(&once.output, name);
            assert!(twice.diagnostics.is_empty(), "{name}: {:?}", twice.diagnostics);
            assert_eq!(once.output, twice.output, "{name}: fmt(fmt(x)) === fmt(x)");
        }
    }

    #[test]
    fn fmt_preserves_ast_on_fixtures() {
        for name in ["money.lts", "cart.lts", "stock.lts", "effects.lts", "map-edge.lts", "char-edge.lts", "letrec-ref-builder.lts", "hostfx.lts", "bytes-edge.lts", "props-edge.lts"] {
            let src = fixture_src(name);
            let once = format_source(&src, name).output;
            assert_eq!(ast_json(&src, name), ast_json(&once, name), "{name}");
        }
    }

    #[test]
    fn fmt_letrec_and_ref_roundtrip() {
        let src = "(defn f (n : number) : number\n  (letrec [g (fn (k : number) : number k)] (g n)))\n\
                   (defn! h (host : (obj)) : (ref number) (ref! 0))\n";
        let once = format_source(src, "t.lts").output;
        assert!(once.contains("(letrec [g (fn (k : number) : number k)]"), "{once}");
        assert!(once.contains("(ref! 0)"), "{once}");
        assert!(once.contains("(ref number)"), "{once}");
        let twice = format_source(&once, "t.lts").output;
        assert_eq!(once, twice, "fmt は冪等");
        assert_eq!(ast_json(src, "t.lts"), ast_json(&once, "t.lts"));
    }

    #[test]
    fn fmt_char_literal_roundtrip() {
        let src = "(defn f (c : char) : number (char-code \\a))\n(example (char-code \\u0041) => 65)\n";
        let once = format_source(src, "t.lts").output;
        assert!(once.contains("\\a"), "{once}");
        assert!(once.contains("(c : char)"), "{once}");
        let twice = format_source(&once, "t.lts").output;
        assert_eq!(once, twice, "fmt は冪等");
        // エスケープが正しく往復すること（AST 保存）
        assert_eq!(ast_json(src, "t.lts"), ast_json(&once, "t.lts"));
    }

    #[test]
    fn fmt_map_set_type_syntax() {
        let out = format_source(
            "(defn f (m : (map string number)) (s : (set number)) : number 1)",
            "t.lts",
        )
        .output;
        assert!(out.contains("(m : (map string number))"), "{out}");
        assert!(out.contains("(s : (set number))"), "{out}");
        let twice = format_source(&out, "t.lts").output;
        assert_eq!(out, twice, "fmt は冪等");
    }

    #[test]
    fn fmt_keeps_comments_in_canonical_position() {
        let src = fixture_src("cart.lts");
        let out = format_source(&src, "cart.lts").output;
        let idx_comment = out.find(";; place-order");
        let idx_type = out.find("(type PlaceError");
        assert!(idx_comment.is_some_and(|i| i < idx_type.unwrap_or(usize::MAX)), "{out}");
        assert!(out.contains(";; (Option CartItem) = (union CartItem nil)"), "{out}");
    }

    #[test]
    fn number_literals_are_normalized() {
        let out = format_source("(defn f () : number 1.50)\n(defn g () : number 1E5)", "t.lts").output;
        assert!(out.contains("(defn f () : number\n  1.5)"), "{out}");
        assert!(out.contains("100000"), "{out}");
    }

    #[test]
    fn string_escapes_and_multiline_styles() {
        let out = format_source("(defn f () : string \"a\\nb\\t\")", "t.lts").output;
        assert_eq!(out, "(defn f () : string\n  \"a\\nb\\t\")\n");
        let m = format_source("(defn f (x : number) : number (match x [0 1] [_ 2]))", "t.lts").output;
        assert!(m.contains("(match x\n    [0 1]\n    [_ 2]"), "{m}");
    }

    #[test]
    fn parse_error_returns_original_source() {
        let src = "(defn broken (x : number) : number\n  (if (< x 0) (- 0 x) x\n";
        let r = format_source(src, "broken.lts");
        assert!(!r.diagnostics.is_empty());
        assert_eq!(r.output, src);
    }
}
