#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use lists::backends;
use lists::diagnostic::{DiagKind, Diagnostic, Severity};
use lists::fmt::format_source;
use lists::repl;
use lists::span::Span;
use std::rc::Rc;

#[derive(Parser)]
#[command(name = "lists", version, about = "λ LisTS P0 compiler (Rust edition)")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Compile {
        files: Vec<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
        /// 実行/差分テスト対象のバックエンド（デフォルト: js）
        #[arg(long, default_value = "js")]
        backend: String,
        /// prop 評価のシード（codegen 出力に埋め込む。既定: ドキュメント化された定数）
        #[arg(long)]
        seed: Option<u64>,
    },
    Fmt {
        files: Vec<PathBuf>,
    },
    Repl {
        /// 注入する host（none=注入なし、mock=モック host）
        #[arg(long, default_value = "none")]
        host: String,
        /// prop 評価のシード（既定: ドキュメント化された定数。同じシードで同じ反例）
        #[arg(long)]
        seed: Option<u64>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Commands::Compile {
            files,
            out,
            backend,
            seed,
        } => ExitCode::from(compile(&files, out.as_deref(), &backend, seed) as u8),
        Commands::Fmt { files } => ExitCode::from(fmt_cmd(&files) as u8),
        Commands::Repl { host, seed } => {
            match repl::run_with_options(&host, seed) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("repl error: {e}");
                    ExitCode::from(2)
                }
            }
        }
    }
}

fn read_diag(file: &str, message: String) -> Diagnostic {
    Diagnostic {
        kind: DiagKind::ImportNotFound,
        severity: Severity::Error,
        span: Span {
            file: Rc::from(file),
            start: 0,
            end: 0,
            line: 1,
            col: 1,
        },
        message,
        data: None,
        fix: None,
    }
}

fn compile(files: &[PathBuf], out_dir: Option<&Path>, backend_name: &str, seed: Option<u64>) -> i32 {
    let Some(backend) = backends::lookup(backend_name) else {
        let d = read_diag(
            backend_name,
            format!(
                "unknown backend '{backend_name}' (available: {:?})",
                backends::names()
            ),
        );
        println!(
            "{}",
            serde_json::to_string(&vec![d]).unwrap_or_else(|_| "[]".to_string())
        );
        return 2;
    };
    let sources: Vec<backends::SourceFile> = files
        .iter()
        .map(|f| backends::SourceFile {
            path: f.clone(),
            name: f
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| f.to_string_lossy().into_owned()),
        })
        .collect();
    let out_root = match out_dir {
        Some(d) => {
            let _ = std::fs::create_dir_all(d);
            d.to_path_buf()
        }
        None => files
            .first()
            .and_then(|f| f.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from(".")),
    };
    match backend.compile(&sources, &out_root, seed.unwrap_or(lists::rng::DEFAULT_SEED)) {
        Ok(_artifacts) => {
            println!("[]");
            0
        }
        Err(diags) => {
            println!(
                "{}",
                serde_json::to_string(&diags).unwrap_or_else(|_| "[]".to_string())
            );
            1
        }
    }
}

fn fmt_cmd(files: &[PathBuf]) -> i32 {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    for file in files {
        let file_s = file.to_string_lossy().into_owned();
        let src = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                diagnostics.push(read_diag(&file_s, format!("ファイルを読めません: '{file_s}': {e}")));
                continue;
            }
        };
        let r = format_source(&src, &file_s);
        let had_err = !r.diagnostics.is_empty();
        diagnostics.extend(r.diagnostics);
        if !had_err {
            let _ = std::fs::write(file, &r.output);
        }
    }
    println!(
        "{}",
        serde_json::to_string(&diagnostics).unwrap_or_else(|_| "[]".to_string())
    );
    if diagnostics.is_empty() {
        0
    } else {
        1
    }
}
