use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use serde_json::{json, Value as Json};

use crate::check::{check_file, create_context, ModuleCache};
use crate::desugar::{desugar, desugar_expr, CoreDef};
use crate::eval::{global_env, load_defs, load_file, run_examples, Interp};
use crate::host::{host_type, inject_host, HostMode, MockHost};
use crate::lexer::tokenize;
use crate::parser::{parse_expr, parse_program};
use crate::types::{normalize_type, print_type, AliasEnv, Type};
use crate::value::{print_value, Env};

pub struct Repl {
    defs: Vec<CoreDef>,
    context_file: String,
    env: Rc<Env>,
    cache: ModuleCache,
    host_mode: HostMode,
    mock: Option<MockHost>,
    seed: u64,
    aliases: AliasEnv,
}

impl Default for Repl {
    fn default() -> Self {
        Self::new()
    }
}

impl Repl {
    pub fn new() -> Self {
        Self::with_host_mode(HostMode::None)
    }

    pub fn with_host_mode(host_mode: HostMode) -> Self {
        Self::with_options(host_mode, crate::rng::DEFAULT_SEED)
    }

    pub fn with_options(host_mode: HostMode, seed: u64) -> Self {
        Repl {
            defs: Vec::new(),
            context_file: "repl.lts".to_string(),
            env: global_env(),
            cache: ModuleCache::default(),
            host_mode,
            mock: None,
            seed,
            aliases: AliasEnv::new(),
        }
    }

    /// 読み込んだ effect 宣言から host を再構築して注入する（host DI）。
    /// mock モードでは `host` 全局にモック host を束縛し、検査には host の obj 型を登録する。
    fn refresh_host(&mut self, effects: &BTreeMap<Rc<str>, Type>, aliases: &AliasEnv) {
        if self.host_mode != HostMode::Mock {
            return;
        }
        self.cache
            .host_bindings
            .insert(Rc::from("host"), host_type(effects));
        let mock = MockHost::from_effects(effects, aliases);
        inject_host(&self.env, "host", &mock.obj());
        self.mock = Some(mock);
    }

    /// prop op（P1/M22 実評価）: 指定した prop を seed 付きで評価する。
    /// リクエストに "seed" (u64) があればそれを使い、なければ REPL のシード（CLI --seed、既定は rng::DEFAULT_SEED）。
    fn prop(&mut self, id: Json, name: &str, seed: Option<u64>) -> Json {
        let seed = seed.unwrap_or(self.seed);
        let Some(CoreDef::Prop {
            name: pname,
            params,
            body,
            ..
        }) = self
            .defs
            .iter()
            .find(|d| matches!(d, CoreDef::Prop { name: n, .. } if n.as_ref() == name))
        else {
            return json!({
                "id": id, "ok": false,
                "errors": [{ "kind": "unbound-symbol", "message": format!("prop が見つかりません: '{name}'") }]
            });
        };
        let outcome = crate::prop::run_prop(
            pname,
            params,
            body,
            &self.env,
            &self.aliases,
            seed,
            crate::prop::DEFAULT_MAX_TRIALS,
        );
        crate::prop::prop_result_json(id, pname, seed, params, body, outcome)
    }

    fn host_log(&self, id: Json) -> Json {
        match &self.mock {
            Some(mock) => json!({
                "id": id,
                "ok": true,
                "host": "mock",
                "calls": mock
                    .calls()
                    .iter()
                    .map(|c| json!({
                        "effect": c.effect,
                        "args": c.args.iter().map(print_value).collect::<Vec<_>>()
                    }))
                    .collect::<Vec<_>>()
            }),
            None => json!({
                "id": id,
                "ok": true,
                "host": self.host_mode.name(),
                "calls": []
            }),
        }
    }

    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let req: Json = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return Some(
                    serde_json::to_string(&json!({
                        "ok": false,
                        "errors": [{ "kind": "parse-error", "message": format!("invalid JSON line: {e}") }]
                    }))
                    .unwrap_or_else(|_| "{}".to_string()),
                );
            }
        };
        self.handle(&req)
            .map(|r| serde_json::to_string(&r).unwrap_or_else(|_| "{}".to_string()))
    }

    fn handle(&mut self, req: &Json) -> Option<Json> {
        let id = req["id"].clone();
        match req["op"].as_str() {
            Some("quit") => None,
            Some("eval") => Some(self.eval(id, req["src"].as_str().unwrap_or(""))),
            Some("test") => Some(self.test(id)),
            Some("prop") => Some(self.prop(id, req["name"].as_str().unwrap_or(""), req["seed"].as_u64())),
            Some("reload") => Some(self.reload(id, req["file"].as_str().unwrap_or(""))),
            Some("host-log") => Some(self.host_log(id)),
            Some("type-of") => Some(self.type_of(id, req["src"].as_str().unwrap_or(""))),
            other => Some(json!({
                "id": id,
                "ok": false,
                "errors": [{ "kind": "parse-error", "message": format!("unknown op: {}", other.unwrap_or("")) }]
            })),
        }
    }

    fn eval(&mut self, id: Json, src: &str) -> Json {
        let lex = tokenize(src, "repl.lts");
        let (program, lex_diags) = parse_program(&lex);
        if !lex_diags.is_empty() {
            return json!({ "id": id, "ok": false, "errors": lex_diags });
        }
        let (ds_defs, ds_diags) = desugar(&program);
        if !ds_diags.is_empty() {
            return json!({ "id": id, "ok": false, "errors": ds_diags });
        }
        let mut all = self.defs.clone();
        all.extend(ds_defs.iter().cloned());
        let (result, _checker) = create_context(&all, &self.context_file, &mut self.cache);
        if !result.diagnostics.is_empty() {
            return json!({ "id": id, "ok": false, "errors": result.diagnostics });
        }
        self.refresh_host(&result.effects, &result.type_aliases);
        self.aliases = result.type_aliases.clone();
        self.defs = all;
        load_defs(&ds_defs, &self.env);
        if let Some(CoreDef::Defn { name, .. }) = ds_defs.iter().rev().find(|d| matches!(d, CoreDef::Defn { .. })) {
            let type_str = match result.value_types.get(name) {
                Some(t) => print_type(
                    &normalize_type(t, &result.type_aliases),
                    &result.type_aliases,
                ),
                None => "?".to_string(),
            };
            return json!({ "id": id, "ok": true, "defined": name.as_ref(), "type": type_str });
        }
        json!({ "id": id, "ok": true })
    }

    fn test(&mut self, id: Json) -> Json {
        let all = self.defs.clone();
        load_defs(&all, &self.env);
        let mut interp = Interp::new();
        // design §4: trace! の記録をモック host の host-log と共有する
        if let Some(mock) = &self.mock {
            interp.set_host_log(mock.log_handle());
        }
        match run_examples(&mut interp, &all, &self.env) {
            Ok(results) => {
                if results.iter().all(|r| r.pass) {
                    json!({
                        "id": id,
                        "ok": true,
                        "results": results.iter().map(|r| json!({
                            "example": format!("example {}", r.index),
                            "pass": r.pass
                        })).collect::<Vec<_>>()
                    })
                } else {
                    json!({
                        "id": id,
                        "ok": false,
                        "failures": results.iter().filter(|r| !r.pass).map(|r| json!({
                            "example": format!("example {}", r.index),
                            "actual": print_value(&r.actual),
                            "expected": print_value(&r.expected)
                        })).collect::<Vec<_>>()
                    })
                }
            }
            Err(e) => json!({
                "id": id,
                "ok": false,
                "errors": [{ "kind": "internal-error", "message": e.to_string() }]
            }),
        }
    }

    fn reload(&mut self, id: Json, file: &str) -> Json {
        let abs = if std::path::Path::new(file).is_absolute() {
            file.to_string()
        } else {
            match std::env::current_dir() {
                Ok(cwd) => cwd.join(file).to_string_lossy().into_owned(),
                Err(_) => file.to_string(),
            }
        };
        let checked = check_file(&abs, &mut self.cache);
        if !checked.diagnostics.is_empty() {
            return json!({ "id": id, "ok": false, "errors": checked.diagnostics });
        }
        let mut seen = BTreeSet::new();
        match load_file(&abs, &self.env, &mut seen) {
            Ok(defs) => {
                self.defs.extend(defs);
                self.context_file = abs;
                self.refresh_host(&checked.effects, &checked.type_aliases);
                self.aliases = checked.type_aliases.clone();
                json!({ "id": id, "ok": true })
            }
            Err(e) => json!({
                "id": id,
                "ok": false,
                "errors": [{ "kind": "internal-error", "message": e.to_string() }]
            }),
        }
    }

    fn type_of(&mut self, id: Json, src: &str) -> Json {
        let (expr, diags) = parse_expr(src, "repl.lts");
        if !diags.is_empty() {
            return json!({ "id": id, "ok": false, "errors": diags });
        }
        let Some(expr) = expr else {
            return json!({
                "id": id,
                "ok": false,
                "errors": [{ "kind": "parse-error", "message": "式としてパースできません" }]
            });
        };
        let core = desugar_expr(&expr);
        let (result, mut checker) = create_context(&self.defs, &self.context_file, &mut self.cache);
        if !result.diagnostics.is_empty() {
            return json!({ "id": id, "ok": false, "errors": result.diagnostics });
        }
        let t = checker.infer(&core, "(repl)");
        json!({
            "id": id,
            "ok": true,
            "type": print_type(&normalize_type(&t, &result.type_aliases), &result.type_aliases)
        })
    }
}

pub fn run() -> std::io::Result<()> {
    run_with_options("none", None)
}

/// `--host none|mock` と `--seed <u64>` 付きで REPL を起動する。
pub fn run_with_options(host_arg: &str, seed: Option<u64>) -> std::io::Result<()> {
    run_with_host_seed(host_arg, seed)
}

/// 後方互換: `--host` のみ。
pub fn run_with_host(host_arg: &str) -> std::io::Result<()> {
    run_with_host_seed(host_arg, None)
}

fn run_with_host_seed(host_arg: &str, seed: Option<u64>) -> std::io::Result<()> {
    use std::io::{BufRead, Write};
    let mode = HostMode::parse(host_arg).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unknown --host '{host_arg}' (available: none, mock)"),
        )
    })?;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut repl = Repl::with_options(mode, seed.unwrap_or(crate::rng::DEFAULT_SEED));
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match repl.handle_line(trimmed) {
            Some(s) => writeln!(out, "{s}")?,
            None => break,
        }
    }
    Ok(())
}
