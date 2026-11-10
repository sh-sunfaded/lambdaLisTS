use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use serde_json::json;

use crate::ast::print_pattern;
use crate::desugar::{desugar, CoreDef};
use crate::diagnostic::{ast_path, DiagKind, Diagnostic, Patch, Severity};
use crate::effects::{check_effects, find_effect_calls};
use crate::exhaustive::check_exhaustive;
use crate::ir::{Core, Pat, PatKind};
use crate::lexer::tokenize;
use crate::parser::parse_program;
use crate::resolver::{
    check_import_names, file_exists, import_not_found, module_cycle, resolve_import_path,
    ModuleGraph, ModuleInfo,
};
use crate::span::Span;
use crate::stdlib::stdlib;
use crate::types::{
    find_unresolved_alias, normalize_type, normalize_union, print_type, print_type_plain,
    AliasEnv, Scheme, Type,
};
use crate::unify::{apply_subst, prune, unify, Subst};
use crate::value::Value;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModuleResult {
    pub diagnostics: Vec<Diagnostic>,
    pub value_types: BTreeMap<Rc<str>, Type>,
    pub type_aliases: AliasEnv,
    pub effects: BTreeMap<Rc<str>, Type>,
    pub exports: Vec<Rc<str>>,
    /// defn 名 -> 純粋かどうか（defn! は false）。import 先モジュールの分も含めて伝播させる（§7.2 eff）。
    pub defn_purity: BTreeMap<Rc<str>, bool>,
}

#[derive(Default)]
pub struct ModuleCache {
    pub modules: BTreeMap<String, ModuleResult>,
    pub graph: ModuleGraph,
    /// 注入された host の束縛（名前 → host の obj 型）。REPL の `--host` が設定する。
    /// ここにある名前は値名前空間の既知の全局として検査に通る（host DI）。
    pub host_bindings: BTreeMap<Rc<str>, Type>,
}

pub fn new_module_cache() -> ModuleCache {
    ModuleCache::default()
}

pub fn check_program(defs: &[CoreDef], file: &str, cache: &mut ModuleCache) -> ModuleResult {
    let mut checker = Checker::new(file, cache);
    checker.run(defs)
}

pub fn create_context<'a>(
    defs: &[CoreDef],
    file: &'a str,
    cache: &'a mut ModuleCache,
) -> (ModuleResult, Checker<'a>) {
    let mut checker = Checker::new(file, cache);
    let result = checker.run(defs);
    (result, checker)
}

pub fn check_source(src: &str, file: &str, cache: &mut ModuleCache) -> ModuleResult {
    let lex = tokenize(src, file);
    let (program, mut diags) = parse_program(&lex);
    let (defs, _desugar_diags) = desugar(&program);
    let mut res = check_program(&defs, file, cache);
    let type_names: BTreeSet<Rc<str>> = res.type_aliases.keys().cloned().collect();
    let mut name_diags = crate::names::check(&program, &type_names);
    diags.append(&mut res.diagnostics);
    diags.append(&mut name_diags);
    res.diagnostics = diags;
    res
}

pub fn check_file(path: &str, cache: &mut ModuleCache) -> ModuleResult {
    match std::fs::read_to_string(path) {
        Ok(src) => check_source(&src, path, cache),
        Err(_) => ModuleResult {
            diagnostics: vec![import_not_found(
                Span {
                    file: Rc::from(path),
                    start: 0,
                    end: 0,
                    line: 1,
                    col: 1,
                },
                path,
                "(program)",
            )],
            ..Default::default()
        },
    }
}

pub fn type_of_all(result: &ModuleResult) -> BTreeMap<String, String> {
    result
        .value_types
        .iter()
        .map(|(name, t)| {
            (
                name.to_string(),
                print_type(&normalize_type(t, &result.type_aliases), &result.type_aliases),
            )
        })
        .collect()
}

const VARIADIC: &[(&str, &str)] = &[("str-concat", "string"), ("array", "fresh")];

/// fix テンプレート用: 型中の map/set キーをすべて string に置き換える。
fn map_key_to_string(t: &Type) -> Type {
    match t {
        Type::Map(_, v) => Type::Map(Box::new(Type::Str), Box::new(map_key_to_string(v))),
        Type::Set(_) => Type::Set(Box::new(Type::Str)),
        Type::Array(el) => Type::Array(Box::new(map_key_to_string(el))),
        Type::Union(ms) => Type::Union(ms.iter().map(map_key_to_string).collect()),
        Type::Obj(fields) => Type::Obj(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), map_key_to_string(v)))
                .collect(),
        ),
        Type::ResultT(ok, err) => Type::ResultT(
            Box::new(map_key_to_string(ok)),
            Box::new(map_key_to_string(err)),
        ),
        Type::FnT(params, ret) => Type::FnT(
            params.iter().map(map_key_to_string).collect(),
            Box::new(map_key_to_string(ret)),
        ),
        other => other.clone(),
    }
}

type MatchArms<'c> = Vec<(String, BTreeMap<Rc<str>, Type>, &'c Core)>;

pub struct Checker<'a> {
    file: &'a str,
    cache: &'a mut ModuleCache,
    diagnostics: Vec<Diagnostic>,
    subst: Subst,
    aliases: AliasEnv,
    values: BTreeMap<Rc<str>, Type>,
    effects: BTreeMap<Rc<str>, Type>,
    scope: Vec<BTreeMap<Rc<str>, Type>>,
    next_var_id: usize,
    stdlib: BTreeMap<Rc<str>, Scheme>,
    predef: BTreeSet<Rc<str>>,
    defn_purity: BTreeMap<Rc<str>, bool>,
    in_impure_defn: bool,
}

impl<'a> Checker<'a> {
    pub fn new(file: &'a str, cache: &'a mut ModuleCache) -> Self {
        let host_bindings = cache.host_bindings.clone();
        let mut checker = Checker {
            file,
            cache,
            diagnostics: Vec::new(),
            subst: Vec::new(),
            aliases: BTreeMap::new(),
            values: BTreeMap::new(),
            effects: BTreeMap::new(),
            scope: Vec::new(),
            next_var_id: 1,
            stdlib: stdlib(),
            predef: BTreeSet::new(),
            defn_purity: BTreeMap::new(),
            in_impure_defn: false,
        };
        // 注入された host は既知の全局として扱う（host DI）
        for (n, t) in host_bindings {
            checker.values.insert(n.clone(), t.clone());
            checker.predef.insert(n);
        }
        checker
    }

    pub fn run(&mut self, defs: &[CoreDef]) -> ModuleResult {
        let mut exports: Vec<Rc<str>> = Vec::new();
        for d in defs {
            if let CoreDef::Type {
                name, ty, span, path, ..
            } = d
            {
                if self.aliases.contains_key(name) {
                    self.diag(
                        DiagKind::DuplicateDefinition,
                        span.clone(),
                        format!("型 '{name}' は既に定義されています"),
                        Some(json!({ "path": path })),
                        None,
                        Severity::Error,
                    );
                    continue;
                }
                self.aliases.insert(name.clone(), ty.clone());
            }
        }
        // 型エイリアス定義中の map/set キー型も検証する（§1.1 キー制約）。
        for d in defs {
            if let CoreDef::Type {
                ty, span, path, ..
            } = d
            {
                let n = normalize_type(ty, &self.aliases);
                self.validate_container_keys(&n, span, path);
            }
        }
        // Load imports first so defn signatures can reference imported names and types.
        for d in defs {
            if let CoreDef::Import {
                path,
                names,
                span,
            } = d
            {
                self.load_import(path, names, span);
            }
        }
        // Register defn signatures up front so mutually recursive defns in the same file can reference each other.
        for d in defs {
            if let CoreDef::Defn {
                name,
                params,
                ret,
                span,
                path,
                pure,
                ..
            } = d
            {
                self.defn_purity.insert(name.clone(), *pure);
                if self.values.contains_key(name)
                    || self.effects.contains_key(name)
                    || !self.predef.insert(name.clone())
                {
                    self.diag(
                        DiagKind::DuplicateDefinition,
                        span.clone(),
                        format!("値名前空間の '{name}' は既に定義されています"),
                        Some(json!({ "path": path })),
                        None,
                        Severity::Error,
                    );
                    continue;
                }
                let param_types: Vec<Type> = params
                    .iter()
                    .enumerate()
                    .map(|(i, p)| self.resolve(&p.ty, span, &ast_path(path, &["params", &i.to_string()])))
                    .collect();
                let ret_t = self.resolve(ret, span, &ast_path(path, &["ret"]));
                self.values.insert(
                    name.clone(),
                    Type::FnT(param_types, Box::new(ret_t)),
                );
            }
        }
        for d in defs {
            match d {
                CoreDef::Import { .. } => {}
                CoreDef::Export { names, .. } => exports.extend(names.iter().cloned()),
                CoreDef::Type { .. } => {}
                CoreDef::Effect {
                    name,
                    ty,
                    span,
                    path,
                } => {
                    if self.effects.contains_key(name) || self.values.contains_key(name) {
                        self.diag(
                            DiagKind::DuplicateDefinition,
                            span.clone(),
                            format!("値名前空間の '{name}' は既に定義されています"),
                            Some(json!({ "path": path })),
                            None,
                            Severity::Error,
                        );
                        continue;
                    }
                    let t = self.resolve(ty, span, &ast_path(path, &["type"]));
                    self.effects.insert(name.clone(), t);
                }
                CoreDef::Defn { .. } => self.check_defn(d),
                CoreDef::Example {
                    actual,
                    expected,
                    span,
                    path,
                } => {
                    let a = self.infer(actual, &ast_path(path, &["args", "0"]));
                    let e = self.infer(expected, &ast_path(path, &["args", "2"]));
                    if unify(&a, &e, &mut self.subst, &self.aliases).is_none() {
                        self.type_mismatch(span, path, &e, &a);
                    }
                }
                CoreDef::Prop {
                    params,
                    body,
                    span,
                    path,
                    ..
                } => {
                    let ptypes: Vec<Type> = params
                        .iter()
                        .enumerate()
                        .map(|(i, p)| {
                            self.resolve(&p.ty, span, &ast_path(path, &["params", &i.to_string()]))
                        })
                        .collect();
                    // P1 (M22): prop は実評価されるので forall パラメータは生成可能な型のみ
                    for (p, t) in params.iter().zip(ptypes.iter()) {
                        if !crate::prop::is_generatable(t, &self.aliases) {
                            self.diag(
                                DiagKind::TypeMismatch,
                                span.clone(),
                                format!(
                                    "prop の forall パラメータ '{n}' の型 {t:?} は生成できない型です（関数型・型変数は不可）",
                                    n = p.name
                                ),
                                Some(json!({ "path": path })),
                                None,
                                Severity::Error,
                            );
                        }
                    }
                    let frame: BTreeMap<Rc<str>, Type> = params
                        .iter()
                        .zip(ptypes.iter())
                        .map(|(p, t)| (p.name.clone(), t.clone()))
                        .collect();
                    self.scope.push(frame);
                    self.check(body, &Type::Bool, &ast_path(path, &["body"]));
                    self.scope.pop();
                }
                CoreDef::Error {
                    message,
                    span,
                    path,
                } => {
                    self.diag(
                        DiagKind::ParseError,
                        span.clone(),
                        message.clone(),
                        Some(json!({ "path": path })),
                        None,
                        Severity::Error,
                    );
                }
            }
        }
        self.diagnostics.extend(check_effects(defs, &self.defn_purity));
        ModuleResult {
            diagnostics: self.diagnostics.clone(),
            value_types: self.values.clone(),
            type_aliases: self.aliases.clone(),
            effects: self.effects.clone(),
            exports,
            defn_purity: self.defn_purity.clone(),
        }
    }

    fn load_import(&mut self, import_path: &str, names: &[Rc<str>], span: &Span) {
        let Some(target) = resolve_import_path(self.file, import_path) else {
            return;
        };
        if !file_exists(&target) {
            self.diagnostics
                .push(import_not_found(span.clone(), import_path, "(import)"));
            return;
        }
        if self.cache.graph.is_on_stack(&target) {
            self.diagnostics
                .push(module_cycle(span.clone(), import_path, "(import)"));
            return;
        }
        let cached = self.cache.modules.get(&target).cloned();
        let mod_result = match cached {
            Some(m) => m,
            None => {
                self.cache.graph.enter(&target);
                let r = match std::fs::read_to_string(&target) {
                    Ok(src) => check_source(&src, &target, self.cache),
                    Err(_) => ModuleResult {
                        diagnostics: vec![import_not_found(span.clone(), import_path, "(import)")],
                        ..Default::default()
                    },
                };
                self.cache.graph.leave();
                self.cache.modules.insert(target.clone(), r.clone());
                r
            }
        };
        self.diagnostics.extend(mod_result.diagnostics.iter().cloned());
        let info = ModuleInfo {
            exports: mod_result.exports.iter().map(|s| s.to_string()).collect(),
            value_names: mod_result.value_types.keys().map(|k| k.to_string()).collect(),
            type_names: mod_result.type_aliases.keys().map(|k| k.to_string()).collect(),
            effect_names: mod_result.effects.keys().map(|k| k.to_string()).collect(),
        };
        self.diagnostics
            .extend(check_import_names(&info, names, span, import_path, "(import)"));
        for name in names {
            if let Some(t) = mod_result.value_types.get(name) {
                self.values.insert(name.clone(), t.clone());
                continue;
            }
            if let Some(t) = mod_result.type_aliases.get(name) {
                self.aliases.insert(name.clone(), t.clone());
                continue;
            }
            if let Some(t) = mod_result.effects.get(name) {
                self.effects.insert(name.clone(), t.clone());
            }
        }
        for (n, p) in &mod_result.defn_purity {
            self.defn_purity.insert(n.clone(), *p);
        }
    }

    fn check_defn(&mut self, d: &CoreDef) {
        let CoreDef::Defn {
            name,
            params,
            ret,
            body,
            span,
            path,
            pure,
            ..
        } = d
        else {
            return;
        };
        if !self.predef.contains(name)
            && (self.values.contains_key(name) || self.effects.contains_key(name))
        {
            self.diag(
                DiagKind::DuplicateDefinition,
                span.clone(),
                format!("値名前空間の '{name}' は既に定義されています"),
                Some(json!({ "path": path })),
                None,
                Severity::Error,
            );
            return;
        }
        let (param_types, ret_t) = match self.values.get(name) {
            Some(Type::FnT(ps, r)) if self.predef.contains(name) => (ps.to_vec(), (**r).clone()),
            _ => {
                let param_types: Vec<Type> = params
                    .iter()
                    .enumerate()
                    .map(|(i, p)| self.resolve(&p.ty, span, &ast_path(path, &["params", &i.to_string()])))
                    .collect();
                let ret_t = self.resolve(ret, span, &ast_path(path, &["ret"]));
                self.values.insert(
                    name.clone(),
                    Type::FnT(param_types.clone(), Box::new(ret_t.clone())),
                );
                (param_types, ret_t)
            }
        };
        let frame: BTreeMap<Rc<str>, Type> = params
            .iter()
            .zip(param_types.iter())
            .map(|(p, t)| (p.name.clone(), t.clone()))
            .collect();
        self.scope.push(frame);
        let saved_impure = self.in_impure_defn;
        self.in_impure_defn = !pure;
        self.check(body, &ret_t, &ast_path(path, &["body"]));
        self.scope.pop();
        self.in_impure_defn = saved_impure;
        if !pure {
            self.check_host_handlers(&param_types, body, path);
        }
    }

    /// P1 (§12.6): defn! の第1引数が host。その型が obj なら、本体の各エフェクト呼び出しに
    /// 対応するハンドラフィールドが宣言シグネチャと一致するか検査する。
    fn check_host_handlers(&mut self, param_types: &[Type], body: &Core, path: &str) {
        let Some(host_t) = param_types.first() else {
            return;
        };
        let host_t = prune(host_t, &self.subst);
        let Type::Obj(fields) = &host_t else {
            return;
        };
        let mut sites = Vec::new();
        find_effect_calls(body, &mut sites);
        for site in &sites {
            // 組み込みエフェクト（ref 系・now/elapsed/trace）は host ハンドラ不要
            // （design §1.5 は言語内完結、§4 は言語側で宣言済みの host 能力）
            if crate::effects::is_builtin_effect(&site.effect) {
                continue;
            }
            let Some(sig) = self.effects.get(&site.effect).cloned() else {
                continue;
            };
            match fields.get(&site.effect) {
                Some(ft) => {
                    let ft = prune(ft, &self.subst);
                    if unify(&ft, &sig, &mut self.subst, &self.aliases).is_none() {
                        self.type_mismatch(&site.span, &ast_path(path, &["body"]), &sig, &ft);
                    }
                }
                None => {
                    self.diag(
                        DiagKind::TypeMismatch,
                        site.span.clone(),
                        format!(
                            "host の型にエフェクト '{e}!' のハンドラがありません",
                            e = site.effect
                        ),
                        Some(json!({ "path": path, "effect": site.effect })),
                        None,
                        Severity::Error,
                    );
                }
            }
        }
    }

    fn fresh(&mut self) -> Type {
        let id = self.next_var_id;
        self.next_var_id += 1;
        Type::Var(id)
    }

    fn instantiate(&mut self, scheme: &Scheme) -> Type {
        fn rename(t: &Type, map: &BTreeMap<usize, Type>) -> Type {
            match t {
                Type::Var(id) => map.get(id).cloned().unwrap_or_else(|| t.clone()),
                Type::Array(el) => Type::Array(Box::new(rename(el, map))),
                Type::Map(k, v) => Type::Map(Box::new(rename(k, map)), Box::new(rename(v, map))),
                Type::Set(k) => Type::Set(Box::new(rename(k, map))),
                Type::Ref(inner) => Type::Ref(Box::new(rename(inner, map))),
                Type::Union(ms) => Type::Union(ms.iter().map(|m| rename(m, map)).collect()),
                Type::Obj(fields) => Type::Obj(
                    fields
                        .iter()
                        .map(|(k, v)| (k.clone(), rename(v, map)))
                        .collect(),
                ),
                Type::ResultT(ok, err) => Type::ResultT(
                    Box::new(rename(ok, map)),
                    Box::new(rename(err, map)),
                ),
                Type::FnT(params, ret) => Type::FnT(
                    params.iter().map(|p| rename(p, map)).collect(),
                    Box::new(rename(ret, map)),
                ),
                _ => t.clone(),
            }
        }
        let mut map: BTreeMap<usize, Type> = BTreeMap::new();
        for id in &scheme.vars {
            let f = self.fresh();
            map.insert(*id, f);
        }
        rename(&scheme.body, &map)
    }

    fn lookup_value(&mut self, name: &str) -> Option<Type> {
        for frame in self.scope.iter().rev() {
            if let Some(t) = frame.get(name) {
                return Some(t.clone());
            }
        }
        if let Some(t) = self.values.get(name) {
            return Some(t.clone());
        }
        let scheme = self.stdlib.get(name).cloned();
        if let Some(scheme) = scheme {
            return Some(self.instantiate(&scheme));
        }
        None
    }

    fn resolve(&mut self, t: &Type, span: &Span, path: &str) -> Type {
        let n = normalize_type(t, &self.aliases);
        if let Some(unresolved) = find_unresolved_alias(&n) {
            self.diag(
                DiagKind::TypeSyntaxError,
                span.clone(),
                format!("未定義の型です: '{unresolved}'"),
                Some(json!({ "path": path })),
                None,
                Severity::Error,
            );
        }
        self.validate_container_keys(&n, span, path);
        n
    }

    /// design/interp-capability.md §1.1: map/set のキーは str/sym/int のみ。
    /// 現 SPEC には sym/int が無いため string/number（リテラル含む）のみ許可（REPORT.md に記録）。
    /// 違反は type-mismatch + fix テンプレート（キーを string に置換した型）を付ける（SPEC §9.3）。
    fn validate_container_keys(&mut self, t: &Type, span: &Span, path: &str) {
        match t {
            Type::Map(k, v) => {
                self.check_map_key(k, span, path, t);
                self.validate_container_keys(k, span, path);
                self.validate_container_keys(v, span, path);
            }
            Type::Set(k) => {
                self.check_map_key(k, span, path, t);
                self.validate_container_keys(k, span, path);
            }
            Type::Ref(inner) => self.validate_container_keys(inner, span, path),
            Type::Array(el) => self.validate_container_keys(el, span, path),
            Type::Union(ms) => {
                for m in ms {
                    self.validate_container_keys(m, span, path);
                }
            }
            Type::Obj(fields) => {
                for v in fields.values() {
                    self.validate_container_keys(v, span, path);
                }
            }
            Type::ResultT(ok, err) => {
                self.validate_container_keys(ok, span, path);
                self.validate_container_keys(err, span, path);
            }
            Type::FnT(params, ret) => {
                for p in params {
                    self.validate_container_keys(p, span, path);
                }
                self.validate_container_keys(ret, span, path);
            }
            _ => {}
        }
    }

    fn check_map_key(&mut self, key: &Type, span: &Span, path: &str, whole: &Type) {
        let k = normalize_type(key, &self.aliases);
        let ok = matches!(k, Type::Str | Type::Number | Type::Literal(_) | Type::Var(_));
        if ok {
            return;
        }
        let fixed = map_key_to_string(whole);
        let exp = print_type(&fixed, &self.aliases);
        let act = print_type(key, &self.aliases);
        self.diag(
            DiagKind::TypeMismatch,
            span.clone(),
            format!("map/set のキー型は string か number のみです: 期待 {exp}、実際 {act}"),
            Some(json!({ "path": path, "expected": exp, "actual": act })),
            Some(Patch::Replace {
                path: path.to_string(),
                template: print_type_plain(&fixed),
            }),
            Severity::Error,
        );
    }

    fn diag(
        &mut self,
        kind: DiagKind,
        span: Span,
        message: String,
        data: Option<serde_json::Value>,
        fix: Option<Patch>,
        severity: Severity,
    ) {
        self.diagnostics.push(Diagnostic {
            kind,
            severity,
            span,
            message,
            data,
            fix,
        });
    }

    fn type_mismatch(&mut self, span: &Span, path: &str, expected: &Type, actual: &Type) {
        let exp = print_type(expected, &self.aliases);
        let act = print_type(&apply_subst(actual, &self.subst), &self.aliases);
        self.diag(
            DiagKind::TypeMismatch,
            span.clone(),
            format!("型が一致しません: 期待 {exp}、実際 {act}"),
            Some(json!({ "path": path, "expected": exp, "actual": act })),
            Some(Patch::Replace {
                path: path.to_string(),
                template: print_type_plain(expected),
            }),
            Severity::Error,
        );
    }

    fn lit_type(value: &Value) -> Type {
        match value {
            Value::Nil => Type::Nil,
            Value::Num(_) => Type::Number,
            Value::Dec(_) => Type::Dec,
            Value::Str(_) => Type::Str,
            Value::Bool(_) => Type::Bool,
            Value::Char(_) => Type::Char,
            _ => Type::Nil,
        }
    }

    pub fn infer(&mut self, e: &Core, path: &str) -> Type {
        match e {
            Core::Lit { value, .. } => Self::lit_type(value),
            Core::Var { name, span } => {
                if name.as_ref() == "_" {
                    self.diag(
                        DiagKind::UnboundSymbol,
                        span.clone(),
                        "'_' に束縛された値は参照できません".to_string(),
                        Some(json!({ "path": path })),
                        None,
                        Severity::Error,
                    );
                    return Type::Nil;
                }
                match self.lookup_value(name) {
                    Some(t) => t,
                    None => {
                        self.diag(
                            DiagKind::UnboundSymbol,
                            span.clone(),
                            format!("未定義のシンボルです: '{name}'"),
                            Some(json!({ "path": path })),
                            None,
                            Severity::Error,
                        );
                        Type::Nil
                    }
                }
            }
            Core::Lam {
                params,
                ret,
                body,
                span,
                ..
            } => {
                let ptypes: Vec<Type> = params
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        self.resolve(&p.ty, span, &ast_path(path, &["params", &i.to_string()]))
                    })
                    .collect();
                let ret_t = self.resolve(ret, span, &ast_path(path, &["ret"]));
                let frame: BTreeMap<Rc<str>, Type> = params
                    .iter()
                    .zip(ptypes.iter())
                    .map(|(p, t)| (p.name.clone(), t.clone()))
                    .collect();
                self.scope.push(frame);
                self.check(body, &ret_t, &ast_path(path, &["body"]));
                self.scope.pop();
                Type::FnT(ptypes, Box::new(ret_t))
            }
            Core::App { f, args, span, .. } => {
                if let Core::Var { name, .. } = f.as_ref() {
                    // decimal（M23）: + - * / < <= > >= は「両引数が decimal」のとき decimal/bool を返す。
                    // 両 number なら従来どおり。混在は type-mismatch（暗黙変換なし）。
                    if matches!(
                        name.as_ref(),
                        "+" | "-" | "*" | "/" | "<" | "<=" | ">" | ">="
                    ) && args.len() == 2
                    {
                        let t1 = prune(&self.infer(&args[0], &ast_path(path, &["args", "0"])), &self.subst);
                        let t2 = prune(&self.infer(&args[1], &ast_path(path, &["args", "1"])), &self.subst);
                        match (&t1, &t2) {
                            (Type::Dec, Type::Dec) => {
                                return if matches!(name.as_ref(), "<" | "<=" | ">" | ">=") {
                                    Type::Bool
                                } else {
                                    Type::Dec
                                };
                            }
                            (Type::Number, Type::Number) => {}
                            _ => {
                                self.type_mismatch(span, path, &t1, &t2);
                                return Type::Nil;
                            }
                        }
                    }
                    if let Some((_, kind)) = VARIADIC.iter().find(|(n, _)| *n == name.as_ref()) {
                        let param = if *kind == "string" {
                            Type::Str
                        } else {
                            self.fresh()
                        };
                        for (i, arg) in args.iter().enumerate() {
                            self.check(arg, &param, &ast_path(path, &["args", &i.to_string()]));
                        }
                        return if name.as_ref() == "array" {
                            Type::Array(Box::new(apply_subst(&param, &self.subst)))
                        } else {
                            Type::Str
                        };
                    }
                    // design/interp-capability.md §1.1: obj->map は obj から map への単方向変換。
                    // 値型は obj フィールド値型の union として組み立てる（多相では表現できないため特殊処理）。
                    if name.as_ref() == "obj->map" && args.len() == 1 {
                        let at = prune(
                            &self.infer(&args[0], &ast_path(path, &["args", "0"])),
                            &self.subst,
                        );
                        let Type::Obj(fields) = &at else {
                            let want = Type::Obj(BTreeMap::new());
                            self.type_mismatch(span, path, &want, &at);
                            return Type::Nil;
                        };
                        let val_t = if fields.is_empty() {
                            Type::Nil
                        } else {
                            Type::Union(normalize_union(
                                fields.values().cloned().collect(),
                                &self.aliases,
                            ))
                        };
                        return Type::Map(Box::new(Type::Str), Box::new(val_t));
                    }
                }
                let ft = prune(&self.infer(f, &ast_path(path, &["fn"])), &self.subst);
                let Type::FnT(ft_params, ft_ret) = &ft else {
                    let want = Type::FnT(
                        args.iter().map(|_| self.fresh()).collect(),
                        Box::new(self.fresh()),
                    );
                    self.type_mismatch(span, path, &want, &ft);
                    return Type::Nil;
                };
                if ft_params.len() != args.len() {
                    self.diag(
                        DiagKind::ArityMismatch,
                        span.clone(),
                        format!(
                            "引数の数が一致しません: 期待 {}、実際 {}",
                            ft_params.len(),
                            args.len()
                        ),
                        Some(json!({
                            "path": path,
                            "expected": ft_params.len(),
                            "actual": args.len()
                        })),
                        Some(Patch::Replace {
                            path: path.to_string(),
                            template: print_type_plain(&ft),
                        }),
                        Severity::Error,
                    );
                    return Type::Nil;
                }
                for (i, arg) in args.iter().enumerate() {
                    self.check(arg, &ft_params[i], &ast_path(path, &["args", &i.to_string()]));
                }
                apply_subst(ft_ret, &self.subst)
            }
            Core::Let {
                name, rhs, body, ..
            } => {
                let t = self.infer(rhs, &ast_path(path, &["rhs"]));
                self.scope.push(BTreeMap::from([(name.clone(), t)]));
                let r = self.infer(body, &ast_path(path, &["body"]));
                self.scope.pop();
                r
            }
            Core::Letrec {
                name,
                lam,
                body,
                ..
            } => {
                // design §2.1: 束縛対象は Lam のみ。違反は type-mismatch + fix（§9.3）。
                if !matches!(lam.as_ref(), Core::Lam { .. }) {
                    let lt = self.infer(lam, &ast_path(path, &["lam"]));
                    let lam_path = ast_path(path, &["lam"]);
                    let want = Type::FnT(vec![], Box::new(self.fresh()));
                    let act = print_type(&apply_subst(&lt, &self.subst), &self.aliases);
                    let exp = print_type(&want, &self.aliases);
                    self.diag(
                        DiagKind::TypeMismatch,
                        lam.span().clone(),
                        format!("letrec の束縛対象は fn (Lam) のみです: 期待 {exp}、実際 {act}"),
                        Some(json!({ "path": lam_path, "expected": exp, "actual": act })),
                        Some(Patch::Replace {
                            path: lam_path,
                            template: format!("(fn () : nil {})", crate::prop::print_core(lam)),
                        }),
                        Severity::Error,
                    );
                    return Type::Nil;
                }
                // Lam は注釈から関数型が分かるので、本体検査前に束縛名へその型を入れる
                //（自己参照 (go ...) が fn 型として検査できるようにするため）。
                let lt = if let Core::Lam { params, ret, span, .. } = lam.as_ref() {
                    let ptypes: Vec<Type> = params
                        .iter()
                        .enumerate()
                        .map(|(i, p)| self.resolve(&p.ty, span, &ast_path(path, &["lam", "params", &i.to_string()])))
                        .collect();
                    let ret_t = self.resolve(ret, span, &ast_path(path, &["lam", "ret"]));
                    Type::FnT(ptypes, Box::new(ret_t))
                } else {
                    self.fresh()
                };
                self.scope.push(BTreeMap::from([(name.clone(), lt)]));
                let _ = self.infer(lam, &ast_path(path, &["lam"]));
                let r = self.infer(body, &ast_path(path, &["body"]));
                self.scope.pop();
                r
            }
            Core::If {
                cond,
                then,
                else_,
                span,
            } => {
                self.check(cond, &Type::Bool, &ast_path(path, &["cond"]));
                let t1 = self.infer(then, &ast_path(path, &["then"]));
                let t2 = self.infer(else_, &ast_path(path, &["else"]));
                if unify(&t1, &t2, &mut self.subst, &self.aliases).is_none() {
                    self.type_mismatch(span, path, &t1, &t2);
                }
                apply_subst(&t1, &self.subst)
            }
            Core::Field { obj, name, span } => {
                let ot = prune(&self.infer(obj, &ast_path(path, &["obj"])), &self.subst);
                self.field_type(&ot, name, span, path)
            }
            Core::Obj { fields, span, .. } => {
                let mut map: BTreeMap<Rc<str>, Type> = BTreeMap::new();
                for (i, (name, expr)) in fields.iter().enumerate() {
                    if map.contains_key(name) {
                        self.diag(
                            DiagKind::TypeSyntaxError,
                            span.clone(),
                            format!("obj >フィールド '{name}' が重複しています"),
                            Some(json!({ "path": ast_path(path, &["fields", &i.to_string()]) })),
                            None,
                            Severity::Error,
                        );
                        continue;
                    }
                    let t = self.infer(expr, &ast_path(path, &["fields", &i.to_string()]));
                    map.insert(name.clone(), t);
                }
                Type::Obj(map)
            }
            Core::Do { .. } => {
                let res = Type::ResultT(Box::new(self.fresh()), Box::new(self.fresh()));
                self.do_check(e, &res, path);
                apply_subst(&res, &self.subst)
            }
            Core::Match { scrutinee, span, .. } => {
                let st = self.infer(scrutinee, &ast_path(path, &["scrutinee"]));
                let arms_info = self.collect_match_arms(e, &st, path);
                let mut result: Option<Type> = None;
                for (arm_path, binds, body) in arms_info {
                    self.scope.push(binds);
                    let bt = self.infer(body, &arm_path);
                    self.scope.pop();
                    match &result {
                        None => result = Some(bt),
                        Some(r) => {
                            if unify(r, &bt, &mut self.subst, &self.aliases).is_none() {
                                self.type_mismatch(span, path, r, &bt);
                            }
                        }
                    }
                }
                result.unwrap_or(Type::Nil)
            }
            Core::EffectCall {
                effect,
                args,
                span,
                ..
            } => {
                // design §1.5: ref 系は組み込みエフェクト（effect 宣言不要）
                let sig = if let Some(scheme) = crate::effects::builtin_effect_scheme(effect) {
                    self.instantiate(&scheme)
                } else if let Some(s) = self.effects.get(effect) {
                    s.clone()
                } else {
                    self.diag(
                        DiagKind::UnknownEffect,
                        span.clone(),
                        format!("未宣言のエフェクトです: '{effect}!'"),
                        Some(json!({ "path": path })),
                        None,
                        Severity::Error,
                    );
                    return Type::Nil;
                };
                let f = prune(&sig, &self.subst);
                let Type::FnT(f_params, f_ret) = &f else {
                    let want = Type::FnT(
                        args.iter().map(|_| self.fresh()).collect(),
                        Box::new(self.fresh()),
                    );
                    self.type_mismatch(span, path, &want, &f);
                    return Type::Nil;
                };
                if f_params.len() != args.len() {
                    self.diag(
                        DiagKind::ArityMismatch,
                        span.clone(),
                        format!(
                            "エフェクト '{effect}!' >引数の数が一致しません: 期待 {}、実際 {}",
                            f_params.len(),
                            args.len()
                        ),
                        Some(json!({
                            "path": path,
                            "expected": f_params.len(),
                            "actual": args.len()
                        })),
                        None,
                        Severity::Error,
                    );
                    return Type::Nil;
                }
                for (i, arg) in args.iter().enumerate() {
                    self.check(arg, &f_params[i], &ast_path(path, &["args", &i.to_string()]));
                }
                apply_subst(f_ret, &self.subst)
            }
            Core::Error { message, span } => {
                self.diag(
                    DiagKind::ParseError,
                    span.clone(),
                    message.clone(),
                    Some(json!({ "path": path })),
                    None,
                    Severity::Error,
                );
                Type::Nil
            }
        }
    }

    fn check(&mut self, e: &Core, expected: &Type, path: &str) {
        let exp = prune(expected, &self.subst);
        match e {
            Core::Lam {
                params,
                ret,
                body,
                span,
                ..
            } => {
                let ptypes: Vec<Type> = params
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        self.resolve(&p.ty, span, &ast_path(path, &["params", &i.to_string()]))
                    })
                    .collect();
                if let Type::FnT(exp_params, exp_ret) = &exp {
                    if exp_params.len() == params.len() {
                        for (i, pt) in ptypes.iter().enumerate() {
                            if unify(pt, &exp_params[i], &mut self.subst, &self.aliases).is_none() {
                                self.type_mismatch(
                                    span,
                                    &ast_path(path, &["params", &i.to_string()]),
                                    &exp_params[i],
                                    pt,
                                );
                            }
                        }
                        let frame: BTreeMap<Rc<str>, Type> = params
                            .iter()
                            .zip(exp_params.iter())
                            .map(|(p, t)| (p.name.clone(), t.clone()))
                            .collect();
                        self.scope.push(frame);
                        self.check(body, exp_ret, &ast_path(path, &["body"]));
                        self.scope.pop();
                        return;
                    }
                }
                let _ = ret;
                let t = self.infer(e, path);
                if unify(&t, expected, &mut self.subst, &self.aliases).is_none() {
                    self.type_mismatch(span, path, expected, &t);
                }
            }
            Core::If { cond, then, else_, .. } => {
                self.check(cond, &Type::Bool, &ast_path(path, &["cond"]));
                self.check(then, expected, &ast_path(path, &["then"]));
                self.check(else_, expected, &ast_path(path, &["else"]));
            }
            Core::Let { name, rhs, body, .. } => {
                let t = self.infer(rhs, &ast_path(path, &["rhs"]));
                self.scope.push(BTreeMap::from([(name.clone(), t)]));
                self.check(body, expected, &ast_path(path, &["body"]));
                self.scope.pop();
            }
            Core::Do { .. } => self.do_check(e, expected, path),
            Core::Match { scrutinee, .. } => {
                let st = self.infer(scrutinee, &ast_path(path, &["scrutinee"]));
                let arms_info = self.collect_match_arms(e, &st, path);
                for (arm_path, binds, body) in arms_info {
                    self.scope.push(binds);
                    self.check(body, expected, &arm_path);
                    self.scope.pop();
                }
            }
            _ => {
                let t = self.infer(e, path);
                if unify(&t, expected, &mut self.subst, &self.aliases).is_none() {
                    self.type_mismatch(e.span(), path, expected, &t);
                }
            }
        }
    }

    fn do_check(&mut self, e: &Core, expected: &Type, path: &str) {
        let Core::Do {
            binds,
            body,
            span,
            ..
        } = e
        else {
            return;
        };
        let mut exp = prune(expected, &self.subst);
        if matches!(exp, Type::Alias(_)) {
            exp = prune(&normalize_type(&exp, &self.aliases), &self.subst);
        }
        if let Type::Var(id) = &exp {
            let id = *id;
            let r = Type::ResultT(Box::new(self.fresh()), Box::new(self.fresh()));
            if self.subst.len() <= id {
                self.subst.resize(id + 1, None);
            }
            self.subst[id] = Some(r);
            exp = prune(expected, &self.subst);
        }
        let Type::ResultT(_, err_t) = &exp else {
            let want = Type::ResultT(Box::new(self.fresh()), Box::new(self.fresh()));
            self.type_mismatch(span, path, &want, expected);
            return;
        };
        let err_t = (**err_t).clone();
        self.scope.push(BTreeMap::new());
        for (i, b) in binds.iter().enumerate() {
            let bind_path = ast_path(path, &["binds", &i.to_string()]);
            let ti = prune(&self.infer(&b.expr, &bind_path), &self.subst);
            if matches!(b.expr, Core::EffectCall { .. }) && self.in_impure_defn {
                // §7.4 rule 5: defn! 内ではエフェクト呼び出しの do>bind は Result でなくてもよい
                if b.name.as_ref() != "_" {
                    let bound = match &ti {
                        Type::ResultT(ok, _) => apply_subst(ok, &self.subst),
                        _ => apply_subst(&ti, &self.subst),
                    };
                    if let Some(frame) = self.scope.last_mut() {
                        frame.insert(b.name.clone(), bound);
                    }
                }
                continue;
            }
            if !matches!(ti, Type::ResultT(_, _)) {
                let want = Type::ResultT(Box::new(self.fresh()), Box::new(err_t.clone()));
                self.type_mismatch(span, &bind_path, &want, &ti);
                continue;
            }
            let Type::ResultT(ok_t, _) = &ti else {
                continue;
            };
            let ok_t = (**ok_t).clone();
            let want = Type::ResultT(Box::new(ok_t.clone()), Box::new(err_t.clone()));
            if unify(&ti, &want, &mut self.subst, &self.aliases).is_none() {
                self.type_mismatch(span, &bind_path, &want, &ti);
            }
            if b.name.as_ref() != "_" {
                let bound = apply_subst(&ok_t, &self.subst);
                if let Some(frame) = self.scope.last_mut() {
                    frame.insert(b.name.clone(), bound);
                }
            }
        }
        self.check(body, &exp, &ast_path(path, &["body"]));
        self.scope.pop();
    }

    fn collect_match_arms<'c>(
        &mut self,
        e: &'c Core,
        st: &Type,
        path: &str,
    ) -> MatchArms<'c> {
        let Core::Match { arms, span, .. } = e else {
            return Vec::new();
        };
        let s = prune(st, &self.subst);
        let patterns: Vec<Pat> = arms.iter().map(|a| a.pattern.clone()).collect();
        let ex = check_exhaustive(&s, &patterns, &mut self.subst, &self.aliases);
        if !ex.missing.is_empty() {
            let missing_str: Vec<String> = ex.missing.iter().map(print_pattern).collect();
            self.diag(
                DiagKind::NonExhaustive,
                span.clone(),
                format!(
                    "match が網羅的ではありません。不足パターン: {}",
                    missing_str.join(", ")
                ),
                Some(json!({ "path": path, "missing": missing_str })),
                Some(Patch::Insert {
                    path: ast_path(path, &["arms"]),
                    template: ex
                        .missing
                        .iter()
                        .map(|m| format!("[{} \"...\"]", print_pattern(m)))
                        .collect::<Vec<_>>()
                        .join(" "),
                }),
                Severity::Error,
            );
        }
        for i in &ex.redundant {
            self.diag(
                DiagKind::RedundantArm,
                arms[*i].body.span().clone(),
                format!("アーム {i} は到達不能です（冗長アーム）"),
                Some(json!({
                    "path": ast_path(path, &["arms", &i.to_string()]),
                    "arm": i
                })),
                Some(Patch::Delete {
                    path: ast_path(path, &["arms", &i.to_string()]),
                }),
                Severity::Warning,
            );
        }
        let mut remaining: Option<Vec<Type>> = match &s {
            Type::Union(m) => Some(m.clone()),
            _ => None,
        };
        let mut out = Vec::new();
        for (i, arm) in arms.iter().enumerate() {
            let arm_path = ast_path(path, &["arms", &i.to_string()]);
            let mut target = s.clone();
            if let Some(rem) = &mut remaining {
                if matches!(arm.pattern.kind, PatKind::Var(_) | PatKind::Wildcard) {
                    target = Type::Union(normalize_union(rem.clone(), &self.aliases));
                    rem.clear();
                } else {
                    let hit = rem.iter().position(|m| {
                        self.match_pattern(&arm.pattern, m, true, span, &arm_path)
                            .is_some()
                    });
                    match hit {
                        Some(h) => {
                            target = rem.remove(h);
                        }
                        None => {
                            rem.clear();
                        }
                    }
                }
            }
            let binds = self
                .match_pattern(&arm.pattern, &target, false, span, &arm_path)
                .unwrap_or_default();
            out.push((arm_path, binds, &arm.body));
        }
        out
    }

    fn match_pattern(
        &mut self,
        p: &crate::ir::Pat,
        scrut: &Type,
        quiet: bool,
        span: &Span,
        path: &str,
    ) -> Option<BTreeMap<Rc<str>, Type>> {
        let s = prune(scrut, &self.subst);
        match &p.kind {
            PatKind::Wildcard => Some(BTreeMap::new()),
            PatKind::Var(name) => Some(BTreeMap::from([(name.clone(), s)])),
            PatKind::Lit(value) => {
                let pt = match value {
                    Value::Nil => Type::Nil,
                    Value::Str(sv) => Type::Literal(sv.clone()),
                    Value::Num(_) => Type::Number,
                    Value::Dec(_) => Type::Dec,
                    Value::Bool(_) => Type::Bool,
                    Value::Char(_) => Type::Char,
                    _ => Type::Nil,
                };
                if unify(&pt, &s, &mut self.subst, &self.aliases).is_none() {
                    if !quiet {
                        self.type_mismatch(span, path, &pt, &s);
                    }
                    return None;
                }
                Some(BTreeMap::new())
            }
            PatKind::Obj(fields) => {
                let Type::Obj(sf) = &s else {
                    let want = Type::Obj(
                        fields
                            .iter()
                            .map(|(n, _)| (n.clone(), self.fresh()))
                            .collect(),
                    );
                    if !quiet {
                        self.type_mismatch(span, path, &want, &s);
                    }
                    return None;
                };
                let mut binds = BTreeMap::new();
                for (name, sub) in fields {
                    let Some(ft) = sf.get(name) else {
                        let want = Type::Obj(BTreeMap::from([(name.clone(), self.fresh())]));
                        if !quiet {
                            self.type_mismatch(span, path, &want, &s);
                        }
                        return None;
                    };
                    let sub_path = ast_path(path, &[name.as_ref()]);
                    let sub_binds = self.match_pattern(sub, ft, quiet, span, &sub_path)?;
                    binds.extend(sub_binds);
                }
                Some(binds)
            }
            PatKind::Array(elements) => {
                let Type::Array(el) = &s else {
                    let want = Type::Array(Box::new(self.fresh()));
                    if !quiet {
                        self.type_mismatch(span, path, &want, &s);
                    }
                    return None;
                };
                let mut binds = BTreeMap::new();
                for el_pat in elements {
                    let sub_binds = self.match_pattern(el_pat, el, quiet, span, path)?;
                    binds.extend(sub_binds);
                }
                Some(binds)
            }
            PatKind::Or(patterns) => {
                for sub in patterns {
                    if let Some(b) = self.match_pattern(sub, &s, true, span, path) {
                        return Some(b);
                    }
                }
                if !quiet {
                    self.type_mismatch(span, path, &s, &s);
                }
                None
            }
        }
    }

    fn field_type(&mut self, ot: &Type, name: &str, span: &Span, path: &str) -> Type {
        if let Type::Union(members) = ot {
            let mut types: Vec<Type> = Vec::new();
            for m in members {
                if let Type::Obj(fields) = m {
                    if let Some(ft) = fields.get(name) {
                        types.push(ft.clone());
                        continue;
                    }
                }
                self.diag(
                    DiagKind::TypeMismatch,
                    span.clone(),
                    format!("フィールド '{name}' が {} にはありません", print_type(m, &self.aliases)),
                    Some(json!({ "path": path })),
                    None,
                    Severity::Error,
                );
            }
            if types.is_empty() {
                return Type::Nil;
            }
            return if types.len() == 1 {
                types[0].clone()
            } else {
                Type::Union(normalize_union(types, &self.aliases))
            };
        }
        if let Type::Obj(fields) = ot {
            if let Some(ft) = fields.get(name) {
                return ft.clone();
            }
        }
        let want = Type::Obj(BTreeMap::from([(Rc::from(name), self.fresh())]));
        self.type_mismatch(span, path, &want, ot);
        Type::Nil
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_src(src: &str) -> ModuleResult {
        let mut cache = ModuleCache::default();
        check_source(src, "test.lts", &mut cache)
    }

    fn fixture_path(name: &str) -> String {
        format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
    }

    #[test]
    fn simple_defn_checks_clean() {
        let r = check_src("(defn f (x : number) : number (+ x 1))");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let t = type_of_all(&r);
        assert_eq!(t["f"], "(fn (number) : number)");
    }

    #[test]
    fn failure_ok_string_against_number() {
        let r = check_src("(defn f () : number (ok \"a\"))");
        assert_eq!(r.diagnostics.len(), 1, "{:?}", r.diagnostics);
        let d = &r.diagnostics[0];
        assert_eq!(d.kind, DiagKind::TypeMismatch);
        let data = d.data.as_ref().expect("data");
        assert_eq!(data["expected"], "number");
        assert_eq!(data["path"], "(defn f).body");
        let actual = data["actual"].as_str().expect("actual");
        assert!(actual.contains("(Result string"), "{actual}");
        assert!(matches!(&d.fix, Some(Patch::Replace { path, template }) if path == "(defn f).body" && template == "number"));
    }

    #[test]
    fn unbound_symbol() {
        let r = check_src("(defn f () : number g)");
        assert!(r
            .diagnostics
            .iter()
            .any(|d| d.kind == DiagKind::UnboundSymbol && d.message.contains("'g'")));
    }

    #[test]
    fn duplicate_definition_in_value_namespace() {
        let r = check_src("(defn f () : number 1)\n(defn f () : number 2)");
        assert!(r
            .diagnostics
            .iter()
            .any(|d| d.kind == DiagKind::DuplicateDefinition));
    }

    #[test]
    fn type_and_value_namespaces_are_separate() {
        let r = check_src(
            "(type Money (obj (amount : number)))\n(defn money () : Money (obj (amount 1)))",
        );
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    }

    #[test]
    fn arity_mismatch() {
        let r = check_src("(defn f () : number (+ 1))");
        let d = r
            .diagnostics
            .iter()
            .find(|d| d.kind == DiagKind::ArityMismatch)
            .expect("arity-mismatch");
        let data = d.data.as_ref().expect("data");
        assert_eq!(data["expected"], 2);
        assert_eq!(data["actual"], 1);
    }

    #[test]
    fn type_syntax_error_unknown_alias() {
        let r = check_src("(defn f () : Foo 1)");
        assert!(r
            .diagnostics
            .iter()
            .any(|d| d.kind == DiagKind::TypeSyntaxError && d.message.contains("Foo")));
    }

    #[test]
    fn do_is_check_mode_only() {
        let r = check_src("(defn f () : number (do [_ (ok 1)] (ok 2)))");
        assert!(r
            .diagnostics
            .iter()
            .any(|d| d.kind == DiagKind::TypeMismatch));
    }

    #[test]
    fn do_bind_error_type_unifies_against_expected_e() {
        let r = check_src("(defn f () : (Result number string) (do [_ (err 1)] (ok 2)))");
        assert!(r
            .diagnostics
            .iter()
            .any(|d| d.kind == DiagKind::TypeMismatch));
    }

    #[test]
    fn appendix_a_fixtures_check_clean() {
        for name in ["money.lts", "cart.lts"] {
            let mut cache = ModuleCache::default();
            let r = check_file(&fixture_path(name), &mut cache);
            assert!(r.diagnostics.is_empty(), "{name}: {:?}", r.diagnostics);
        }
    }

    #[test]
    fn place_order_do_unifies_place_error() {
        let mut cache = ModuleCache::default();
        let r = check_file(&fixture_path("cart.lts"), &mut cache);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let t = type_of_all(&r);
        let place = t["place-order"].clone();
        assert!(place.contains("(union (obj (id : string) (tag \"NegativeQty\")) (obj (tag \"EmptyCart\")) string)"), "{place}");
    }
}
