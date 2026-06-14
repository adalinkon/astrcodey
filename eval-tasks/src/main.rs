use std::{fs, path::PathBuf, process::ExitCode};

use astrcode_eval::{EvalConfig, default_cases_dir, run_eval};
use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, ValueEnum)]
enum EvalOutputFormat {
    Json,
    Markdown,
    Md,
}

#[derive(Debug, Parser)]
#[command(
    name = "astrcode-eval",
    version,
    about = "Run AstrCode evaluation cases"
)]
struct Cli {
    /// eval case directory path
    #[arg(long, default_value_os_t = default_cases_dir())]
    cases: PathBuf,
    /// report output path (defaults to stdout)
    #[arg(long)]
    output: Option<PathBuf>,
    /// output format
    #[arg(long, default_value = "json")]
    format: EvalOutputFormat,
    /// max concurrent cases
    #[arg(long, default_value_t = 4)]
    concurrency: usize,
    /// filter by tags
    #[arg(long, value_delimiter = ',')]
    tags: Option<Vec<String>>,
    /// keep temporary work directories
    #[arg(long)]
    keep_workdir: bool,
    /// isolated storage root for eval sessions
    #[arg(long)]
    storage: Option<PathBuf>,
    /// connect to an already-running server
    #[arg(long)]
    server_addr: Option<String>,
    /// auth token for the running server
    #[arg(long)]
    auth_token: Option<String>,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let config = EvalConfig {
        cases_dir: cli.cases,
        concurrency: cli.concurrency,
        tags_filter: cli.tags,
        keep_workdir: cli.keep_workdir,
        server_addr: cli.server_addr,
        auth_token: cli.auth_token,
        storage_root: cli.storage,
    };

    match run_eval(config).await {
        Ok(report) => {
            let rendered = match cli.format {
                EvalOutputFormat::Json => report.to_json(),
                EvalOutputFormat::Markdown | EvalOutputFormat::Md => report.to_markdown(),
            };
            if let Some(path) = cli.output {
                if let Err(err) = fs::write(&path, rendered) {
                    eprintln!("failed to write report to {}: {err}", path.display());
                    return ExitCode::from(1);
                }
            } else {
                println!("{rendered}");
            }
            if report.all_passed() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        },
        Err(err) => {
            eprintln!("eval failed: {err}");
            ExitCode::from(1)
        },
    }
}
