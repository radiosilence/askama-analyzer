//! `askama-analyzer check`: the language server's template errors, printed for a terminal or
//! CI. With `--rust-analyzer`, rust-analyzer checks the generated code as well, as it does in
//! the editor.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use tokio::sync::Notify;

use crate::analysis::Analysis;
use crate::generator::Generator;
use crate::ra::Config;
use crate::text::Encoding;
use crate::vfs::Vfs;
use crate::workspace::Workspace;

/// How long rust-analyzer may take to load the workspace. A cold start builds the workspace's
/// build scripts and proc macros, which takes minutes on a large one.
const LOAD: Duration = Duration::from_secs(15 * 60);
/// rust-analyzer publishes each file's diagnostics as it gets to it; this much quiet means it
/// has published them all.
const QUIET: Duration = Duration::from_secs(5);

pub async fn run(
    root: &Path,
    rust_analyzer: Option<PathBuf>,
    annotate: bool,
) -> anyhow::Result<ExitCode> {
    let root = root
        .canonicalize()
        .with_context(|| format!("finding {}", root.display()))?;
    let started = Instant::now();
    let workspace = analyse(&root, rust_analyzer, Encoding::Utf8).await?;
    let elapsed = started.elapsed();

    let problems = workspace.problems();
    let mut count = 0;
    for (path, list) in &problems {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let shown = path.strip_prefix(&root).unwrap_or(path).display();
        for problem in list {
            let (line, column) = line_column(&text, problem.range.start);
            if annotate {
                let shown = shown.to_string();
                let annotation = annotation(&shown, line, None, Some(column), &problem.message);
                println!("{annotation}");
            } else {
                println!("{shown}:{line}:{column}: {}", problem.message);
            }
            count += 1;
        }
    }
    for (name, failure) in workspace.analysis.failures() {
        let message = format!("{name}: askama-analyzer could not generate this item: {failure}");
        if annotate {
            println!("::error::{}", escape(&message, false));
        } else {
            println!("{message}");
        }
        count += 1;
    }
    eprintln!(
        "{} template items in {} packages, checked in {elapsed:.1?}; {count} problems",
        workspace.analysis.item_count(),
        workspace.analysis.packages.len()
    );
    Ok(if count == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Loads the Cargo workspace at `root` and generates every template item. Given a
/// rust-analyzer, also starts it, sends it the overlays, and waits until it has loaded the
/// workspace and published its diagnostics.
pub async fn analyse(
    root: &Path,
    rust_analyzer: Option<PathBuf>,
    encoding: Encoding,
) -> anyhow::Result<Workspace> {
    let analysis = Analysis::load(root, Generator::spawn()?, Vfs::default()).await?;
    let mut workspace = Workspace::new(analysis, encoding);
    let Some(path) = rust_analyzer else {
        return Ok(workspace);
    };
    let config = Config {
        path: Some(path),
        ..Config::default()
    };
    let wake = Arc::new(Notify::new());
    if let Some(message) = workspace.start(&config, &wake).await {
        bail!(message);
    }
    workspace.sync();
    let Some(session) = workspace.session() else {
        bail!("rust-analyzer stopped while starting");
    };
    if !session.ready(LOAD).await {
        bail!("rust-analyzer did not finish loading within {LOAD:?}");
    }
    while tokio::time::timeout(QUIET, wake.notified()).await.is_ok() {}
    workspace.pull_stale().await;
    if let Some(trouble) = workspace.session().and_then(|session| session.trouble()) {
        eprintln!("rust-analyzer could not load everything: {trouble}");
    }
    Ok(workspace)
}

/// One-based line and character column, as editors and compilers print them.
fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let before = text.get(..offset).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(0, |line| line.chars().count())
        + 1;
    (line, column)
}

/// A GitHub Actions workflow command marking an error in `file` at `line` (from 1), through
/// `end_line` when given, at `column` when given.
pub(crate) fn annotation(
    file: &str,
    line: usize,
    end_line: Option<usize>,
    column: Option<usize>,
    message: &str,
) -> String {
    let mut properties = format!("file={},line={line}", escape(file, true));
    if let Some(end_line) = end_line {
        properties.push_str(&format!(",endLine={end_line}"));
    }
    if let Some(column) = column {
        properties.push_str(&format!(",col={column}"));
    }
    format!("::error {properties}::{}", escape(message, false))
}

/// `text` escaped as GitHub Actions requires in a workflow command's message, or in a property
/// value, where `:` and `,` are separators too.
pub(crate) fn escape(text: &str, property: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '%' => out.push_str("%25"),
            '\r' => out.push_str("%0D"),
            '\n' => out.push_str("%0A"),
            ':' if property => out.push_str("%3A"),
            ',' if property => out.push_str("%2C"),
            c => out.push(c),
        }
    }
    out
}
