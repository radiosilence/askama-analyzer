//! `askama-analyzer`: a language server for askama templates, a `check` command that runs
//! the same analysis from a terminal, `fmt`, the template formatter, and `completions`.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the Language Server Protocol on stdin and stdout. The default.
    Lsp,
    /// Print every template error in the Cargo workspace at PATH, exiting 1 if there are any.
    Check {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Also have this rust-analyzer check the generated code, as the editor does.
        #[arg(long, value_name = "PATH")]
        rust_analyzer: Option<PathBuf>,
        /// Print each problem as a GitHub Actions annotation.
        #[arg(long)]
        annotate: bool,
    },
    /// Format templates in place: the files given, or every file under a `templates`
    /// directory in the directories given. Expressions inside tags are laid out as rustfmt
    /// lays out Rust, and HTML templates are re-indented by nesting depth, by the indentation
    /// their `.editorconfig` sets (2 spaces when it sets none).
    Fmt {
        /// List the templates that are not formatted, and exit 1 if there are any, instead of
        /// writing them.
        #[arg(long)]
        check: bool,
        /// With `--check`, print each unformatted run of lines as a GitHub Actions
        /// annotation holding the formatted text, instead of listing the templates.
        #[arg(long, requires = "check")]
        annotate: bool,
        /// The width lines are kept to.
        #[arg(long, default_value_t = 100)]
        max_width: usize,
        /// Leave the indentation outside tags as written in HTML templates.
        #[arg(long)]
        no_indent_html: bool,
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,
    },
    /// Print the completion script for SHELL.
    Completions { shell: clap_complete::Shell },
}

fn main() -> ExitCode {
    // Logs go to stderr: stdout carries the protocol.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("askama_analyzer=info,warn")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let cli = Cli::parse();
    if let Some(Command::Completions { shell }) = cli.command {
        clap_complete::generate(
            shell,
            &mut Cli::command(),
            "askama-analyzer",
            &mut std::io::stdout(),
        );
        return ExitCode::SUCCESS;
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("askama-analyzer: cannot start the async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    let result = match cli.command.unwrap_or(Command::Lsp) {
        Command::Lsp => runtime.block_on(askama_analyzer::server::run()),
        Command::Check {
            path,
            rust_analyzer,
            annotate,
        } => runtime.block_on(askama_analyzer::check::run(&path, rust_analyzer, annotate)),
        Command::Fmt {
            check,
            annotate,
            max_width,
            no_indent_html,
            paths,
        } => askama_analyzer::format::run(check, annotate, &paths, max_width, !no_indent_html),
        Command::Completions { .. } => unreachable!("handled before the runtime starts"),
    };
    match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("askama-analyzer: {err:#}");
            ExitCode::FAILURE
        }
    }
}
