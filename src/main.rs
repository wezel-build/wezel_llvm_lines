use anyhow::{Context, Result, bail};
use forager_sdk::{Forager, ForagerPluginOutput};
use schemars::JsonSchema;
use serde::Deserialize;

type LlvmRow = (String, u64, u64);
type LlvmLinesOutput = (u64, u64, Vec<LlvmRow>);

const CARGO_LLVM_LINES_VERSION: &str = "0.4.45";

#[derive(Deserialize, JsonSchema)]
struct LlvmLinesInputs {
    /// Package name (required for workspaces).
    package: Option<String>,
}

struct LlvmLines;

impl Forager for LlvmLines {
    const NAME: &'static str = "llvm-lines";
    const DESCRIPTION: &'static str = "Counts LLVM IR lines via cargo-llvm-lines";
    const OUTCOMES_DOC: &'static str = "\
**`llvm-lines`** — count of LLVM IR lines (or monomorphisation copies) for a \
function (u64). Filter tags:\n\
- `summary_unit = \"lines\"` or `\"copies\"` — crate-wide totals (one row each).\n\
- `function = \"<symbol>\"` + `unit = \"lines\"`|`\"copies\"` — per-function rows.";
    type Inputs = LlvmLinesInputs;

    fn run(inputs: LlvmLinesInputs) -> Result<Vec<ForagerPluginOutput>> {
        ensure_cargo_llvm_lines_installed()?;

        let mut cmd = std::process::Command::new("cargo");
        cmd.arg("llvm-lines");
        if let Some(pkg) = &inputs.package {
            cmd.args(["-p", pkg]);
        }
        let output = cmd.output().context("failed to run cargo llvm-lines")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("cargo llvm-lines failed: {stderr}");
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let (total_lines, total_copies, functions) = parse_llvm_lines_output(&stdout)?;

        let mut outcomes = Vec::with_capacity(2 + functions.len() * 2);
        outcomes.push(m(total_lines, &[("summary_unit", "lines")]));
        outcomes.push(m(total_copies, &[("summary_unit", "copies")]));
        for (fn_name, lines, copies) in &functions {
            outcomes.push(m(*lines, &[("function", fn_name), ("unit", "lines")]));
            outcomes.push(m(*copies, &[("function", fn_name), ("unit", "copies")]));
        }
        Ok(outcomes)
    }
}

fn main() {
    if let Err(error) = forager_sdk::run::<LlvmLines>() {
        eprintln!("{}: {error:#}", env!("CARGO_BIN_NAME"));
        std::process::exit(1);
    }
}

fn ensure_cargo_llvm_lines_installed() -> Result<()> {
    let probe = std::process::Command::new("cargo")
        .args(["llvm-lines", "--help"])
        .output();
    if let Ok(out) = probe
        && out.status.success()
    {
        return Ok(());
    }

    eprintln!(
        "[wezel_llvm_lines] cargo-llvm-lines not found; installing v{CARGO_LLVM_LINES_VERSION}..."
    );
    let status = std::process::Command::new("cargo")
        .args([
            "install",
            "--locked",
            "--quiet",
            "--version",
            CARGO_LLVM_LINES_VERSION,
            "cargo-llvm-lines",
        ])
        .status()
        .context("failed to spawn `cargo install cargo-llvm-lines`")?;
    if !status.success() {
        bail!(
            "`cargo install --locked --version {CARGO_LLVM_LINES_VERSION} cargo-llvm-lines` failed (exit {})",
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

fn m(value: u64, tags: &[(&str, &str)]) -> ForagerPluginOutput {
    ForagerPluginOutput {
        name: "llvm-lines".to_string(),
        value: serde_json::json!(value),
        tags: tags
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

/// Parse `cargo llvm-lines` output.
///
/// Format:
/// ```text
///   Lines                 Copies               Function name
///   -----                 ------               -------------
///   361539                12556                (TOTAL)
///     6639 (1.8%,  1.8%)     22 (0.2%,  0.2%)  core::ops::function::FnOnce::call_once
/// ```
fn parse_llvm_lines_output(s: &str) -> Result<LlvmLinesOutput> {
    let mut lines = s.lines();

    loop {
        match lines.next() {
            None => bail!("unexpected end of cargo llvm-lines output: no separator line"),
            Some(line) if line.trim().starts_with("-----") => break,
            Some(_) => continue,
        }
    }

    let total_line = lines.next().context("no TOTAL line after separator")?;
    let mut total_parts = total_line.split_whitespace();
    let total_lines: u64 = total_parts
        .next()
        .and_then(|s| s.parse().ok())
        .context("could not parse total line count")?;
    let total_copies: u64 = total_parts
        .next()
        .and_then(|s| s.parse().ok())
        .context("could not parse total copies count")?;

    let mut functions = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let fn_lines: u64 = match trimmed
            .split_whitespace()
            .next()
            .and_then(|s| s.parse().ok())
        {
            Some(n) => n,
            None => continue,
        };
        let fn_copies: u64 = match trimmed.find(')').and_then(|pos| {
            trimmed[pos + 1..]
                .split_whitespace()
                .find_map(|t| t.parse().ok())
        }) {
            Some(n) => n,
            None => continue,
        };
        let name = match trimmed.rfind(')') {
            Some(pos) => trimmed[pos + 1..].trim(),
            None => continue,
        };
        if !name.is_empty() {
            functions.push((name.to_string(), fn_lines, fn_copies));
        }
    }

    Ok((total_lines, total_copies, functions))
}
