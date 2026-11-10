use std::collections::BTreeMap;
use std::rc::Rc;

use crate::types::{Scheme, Type};

fn v(id: usize) -> Type {
    Type::Var(id)
}

fn fn_t(params: Vec<Type>, ret: Type) -> Type {
    Type::FnT(params, Box::new(ret))
}

fn arr(el: Type) -> Type {
    Type::Array(Box::new(el))
}

fn res(ok: Type, err: Type) -> Type {
    Type::ResultT(Box::new(ok), Box::new(err))
}

fn map_t(k: Type, v: Type) -> Type {
    Type::Map(Box::new(k), Box::new(v))
}

fn set_t(k: Type) -> Type {
    Type::Set(Box::new(k))
}

/// map-get の失敗は (err (obj (tag "not-found")))（design §1.1/§6 の判断 kind）
fn not_found_err() -> Type {
    Type::Obj(BTreeMap::from([(
        Rc::from("tag"),
        Type::Literal(Rc::from("not-found")),
    )]))
}

fn opt(t: Type, nil_t: &Type) -> Type {
    Type::Union(vec![t, nil_t.clone()])
}

fn entry(name: &str, vars: Vec<usize>, body: Type) -> (Rc<str>, Scheme) {
    (Rc::from(name), Scheme { vars, body })
}

pub fn stdlib() -> BTreeMap<Rc<str>, Scheme> {
    let num = Type::Number;
    let dec = Type::Dec;
    let str_t = Type::Str;
    let bool_t = Type::Bool;
    let char_t = Type::Char;
    let nil_t = Type::Nil;

    fn tag_obj(tag: &str) -> Type {
        Type::Obj(BTreeMap::from([(
            Rc::from("tag"),
            Type::Literal(Rc::from(tag)),
        )]))
    }
    BTreeMap::from([
        entry("+", vec![], fn_t(vec![num.clone(), num.clone()], num.clone())),
        entry("-", vec![], fn_t(vec![num.clone(), num.clone()], num.clone())),
        entry("*", vec![], fn_t(vec![num.clone(), num.clone()], num.clone())),
        entry("/", vec![], fn_t(vec![num.clone(), num.clone()], num.clone())),
        entry("<", vec![], fn_t(vec![num.clone(), num.clone()], bool_t.clone())),
        entry(">", vec![], fn_t(vec![num.clone(), num.clone()], bool_t.clone())),
        entry("<=", vec![], fn_t(vec![num.clone(), num.clone()], bool_t.clone())),
        entry(">=", vec![], fn_t(vec![num.clone(), num.clone()], bool_t.clone())),
        entry("=", vec![1], fn_t(vec![v(1), v(1)], bool_t.clone())),
        entry("mod", vec![], fn_t(vec![num.clone(), num.clone()], num.clone())),
        entry("abs", vec![], fn_t(vec![num.clone()], num.clone())),
        entry("min", vec![], fn_t(vec![num.clone(), num.clone()], num.clone())),
        entry("max", vec![], fn_t(vec![num.clone(), num.clone()], num.clone())),
        entry("not", vec![], fn_t(vec![bool_t.clone()], bool_t.clone())),
        entry("str", vec![1], fn_t(vec![v(1)], str_t.clone())),
        entry("to-dec", vec![], fn_t(vec![num.clone()], dec.clone())),
        entry("to-num", vec![], fn_t(vec![dec.clone()], num.clone())),
        entry("str-concat", vec![], fn_t(vec![str_t.clone(), str_t.clone()], str_t.clone())),
        entry("str-slice", vec![], fn_t(vec![str_t.clone(), num.clone(), num.clone()], str_t.clone())),
        entry("len", vec![], fn_t(vec![str_t.clone()], num.clone())),
        entry("contains?", vec![], fn_t(vec![str_t.clone(), str_t.clone()], bool_t.clone())),
        entry("split", vec![], fn_t(vec![str_t.clone(), str_t.clone()], arr(str_t.clone()))),
        entry("array", vec![1], fn_t(vec![v(1), v(1)], arr(v(1)))),
        entry("nth", vec![1], fn_t(vec![arr(v(1)), num.clone()], opt(v(1), &nil_t))),
        entry("count", vec![1], fn_t(vec![arr(v(1))], num.clone())),
        entry("slice", vec![1], fn_t(vec![arr(v(1)), num.clone(), num.clone()], arr(v(1)))),
        entry("push", vec![1], fn_t(vec![arr(v(1)), v(1)], arr(v(1)))),
        entry("concat", vec![1], fn_t(vec![arr(v(1)), arr(v(1))], arr(v(1)))),
        entry(
            "map",
            vec![1, 2],
            fn_t(vec![arr(v(1)), fn_t(vec![v(1)], v(2))], arr(v(2))),
        ),
        entry(
            "filter",
            vec![1],
            fn_t(vec![arr(v(1)), fn_t(vec![v(1)], bool_t.clone())], arr(v(1))),
        ),
        entry(
            "fold-left",
            vec![1, 2],
            fn_t(
                vec![arr(v(1)), v(2), fn_t(vec![v(2), v(1)], v(2))],
                v(2),
            ),
        ),
        entry(
            "find",
            vec![1],
            fn_t(vec![arr(v(1)), fn_t(vec![v(1)], bool_t.clone())], opt(v(1), &nil_t)),
        ),
        entry(
            "any?",
            vec![1],
            fn_t(vec![arr(v(1)), fn_t(vec![v(1)], bool_t.clone())], bool_t.clone()),
        ),
        entry(
            "all?",
            vec![1],
            fn_t(vec![arr(v(1)), fn_t(vec![v(1)], bool_t.clone())], bool_t.clone()),
        ),
        entry("ok", vec![1, 2], fn_t(vec![v(1)], res(v(1), v(2)))),
        entry("err", vec![1, 2], fn_t(vec![v(2)], res(v(1), v(2)))),
        entry(
            "bind",
            vec![1, 2, 3],
            fn_t(
                vec![res(v(1), v(2)), fn_t(vec![v(1)], res(v(3), v(2)))],
                res(v(3), v(2)),
            ),
        ),
        entry(
            "map-result",
            vec![1, 2, 3],
            fn_t(vec![res(v(1), v(2)), fn_t(vec![v(1)], v(3))], res(v(3), v(2))),
        ),
        entry(
            "traverse",
            vec![1, 2, 3],
            fn_t(
                vec![arr(v(1)), fn_t(vec![v(1)], res(v(3), v(2)))],
                res(arr(v(3)), v(2)),
            ),
        ),
        // design/interp-capability.md §1.1: map / set（純粋関数。キーは string/number のみ）
        entry("map-empty", vec![1, 2], fn_t(vec![], map_t(v(1), v(2)))),
        entry(
            "map-get",
            vec![1, 2],
            fn_t(vec![map_t(v(1), v(2)), v(1)], res(v(2), not_found_err())),
        ),
        entry(
            "map-set",
            vec![1, 2],
            fn_t(vec![map_t(v(1), v(2)), v(1), v(2)], map_t(v(1), v(2))),
        ),
        entry(
            "map-del",
            vec![1, 2],
            fn_t(vec![map_t(v(1), v(2)), v(1)], map_t(v(1), v(2))),
        ),
        entry(
            "map-keys",
            vec![1, 2],
            fn_t(vec![map_t(v(1), v(2))], arr(v(1))),
        ),
        entry(
            "map-has?",
            vec![1, 2],
            fn_t(vec![map_t(v(1), v(2)), v(1)], bool_t.clone()),
        ),
        entry(
            "map-size",
            vec![1, 2],
            // design は int だが現 SPEC は number のみ（i64 は P5）。REPORT.md に記録。
            fn_t(vec![map_t(v(1), v(2))], num.clone()),
        ),
        entry("set-empty", vec![1], fn_t(vec![], set_t(v(1)))),
        entry(
            "set-add",
            vec![1],
            fn_t(vec![set_t(v(1)), v(1)], set_t(v(1))),
        ),
        entry(
            "set-has?",
            vec![1],
            fn_t(vec![set_t(v(1)), v(1)], bool_t.clone()),
        ),
        entry(
            "set->array",
            vec![1],
            fn_t(vec![set_t(v(1))], arr(v(1))),
        ),
        // obj -> map は単方向変換。値型は obj フィールド型の union（check.rs で特殊処理）。
        entry("obj->map", vec![1, 2], fn_t(vec![v(1)], map_t(str_t.clone(), v(2)))),
        // design/interp-capability.md §1.2/§3.1: char と文字列プリミティブ（C2）。
        // 位置は Unicode スカラー値（char）単位。int は現 SPEC では number（REPORT.md 記録）。
        entry("char-at", vec![], fn_t(vec![str_t.clone(), num.clone()], res(char_t.clone(), tag_obj("out-of-range")))),
        entry("char-code", vec![], fn_t(vec![char_t.clone()], num.clone())),
        entry("code->char", vec![], fn_t(vec![num.clone()], res(char_t.clone(), tag_obj("invalid-code")))),
        entry(
            "str-take-while",
            vec![],
            fn_t(
                vec![str_t.clone(), num.clone(), fn_t(vec![char_t.clone()], bool_t.clone())],
                Type::Obj(BTreeMap::from([
                    (Rc::from("acc"), str_t.clone()),
                    (Rc::from("pos"), num.clone()),
                ])),
            ),
        ),
        entry(
            "str-skip-while",
            vec![],
            fn_t(
                vec![str_t.clone(), num.clone(), fn_t(vec![char_t.clone()], bool_t.clone())],
                num.clone(),
            ),
        ),
        entry(
            "str-split-at",
            vec![],
            fn_t(
                vec![str_t.clone(), num.clone()],
                res(
                    Type::Obj(BTreeMap::from([
                        (Rc::from("left"), str_t.clone()),
                        (Rc::from("right"), str_t.clone()),
                    ])),
                    tag_obj("out-of-range"),
                ),
            ),
        ),
        entry("str-contains?", vec![], fn_t(vec![str_t.clone(), str_t.clone()], bool_t.clone())),
        entry("char<=?", vec![], fn_t(vec![char_t.clone(), char_t.clone()], bool_t.clone())),
        entry("char=?", vec![], fn_t(vec![char_t.clone(), char_t.clone()], bool_t.clone())),
        entry("char-alphabetic?", vec![], fn_t(vec![char_t.clone()], bool_t.clone())),
        entry("char-numeric?", vec![], fn_t(vec![char_t.clone()], bool_t.clone())),
        entry("char-whitespace?", vec![], fn_t(vec![char_t.clone()], bool_t.clone())),
        // design/interp-capability.md §3.2: builder（純粋・不変値。str→string）
        entry("builder-empty", vec![], fn_t(vec![], Type::Builder)),
        entry(
            "builder-push",
            vec![],
            fn_t(vec![Type::Builder, str_t.clone()], Type::Builder),
        ),
        entry(
            "builder->str",
            vec![],
            fn_t(vec![Type::Builder], str_t.clone()),
        ),
        entry(
            "builder-join",
            vec![],
            fn_t(vec![arr(str_t.clone()), str_t.clone()], str_t.clone()),
        ),
        // design/interp-capability.md §1.4: bytes（不変バイト列。int→number）
        entry("bytes-get", vec![], fn_t(vec![Type::Bytes, num.clone()], opt(num.clone(), &nil_t))),
        entry("bytes-len", vec![], fn_t(vec![Type::Bytes], num.clone())),
        entry(
            "bytes-slice",
            vec![],
            fn_t(vec![Type::Bytes, num.clone(), num.clone()], Type::Bytes),
        ),
        entry("str->bytes", vec![], fn_t(vec![str_t.clone()], Type::Bytes)),
        entry(
            "bytes->str",
            vec![],
            fn_t(vec![Type::Bytes], res(str_t.clone(), tag_obj("invalid-utf8"))),
        ),
        entry(
            "str->codepoints",
            vec![],
            fn_t(vec![str_t.clone()], arr(num.clone())),
        ),
        entry(
            "codepoints->str",
            vec![],
            fn_t(vec![arr(num.clone())], res(str_t.clone(), tag_obj("invalid-code"))),
        ),
    ])
}
