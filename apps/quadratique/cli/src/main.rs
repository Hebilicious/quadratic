mod agents_line;
mod background;
mod caller;
mod commands;
mod init;
mod mcp;
mod process;
mod route;
mod shipped;
mod upgrade;

use std::sync::Arc;

use acid_cat_core::Kind;
use acid_cat_core::cli::{CliOptions, CliOutput, OutputForm, exit, run_cli_process};
use quadratique_server::app::{CATALOG_JSON, Context, application};
use quadratique_server::domain::Binding;
use quadratique_server::server::TARGET;

fn main() {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("quadratique: no async runtime: {error}");
            std::process::exit(exit::INTERNAL);
        }
    };
    let code = runtime.block_on(run(std::env::args().skip(1).collect()));
    drop(runtime);
    std::process::exit(code);
}

async fn run(args: Vec<String>) -> i32 {
    let app = match application() {
        Ok(app) => app,
        Err(problems) => return fail(Kind::Internal, problems.join("; ")),
    };
    let mut options = CliOptions::new("quadratique", env!("CARGO_PKG_VERSION"))
        .output(CliOutput::new().switch(["--json"], OutputForm::Json));
    options.commands = commands::all();
    let catalog = serde_json::from_str(CATALOG_JSON).unwrap_or_default();
    let words = route::Words::of(&catalog, TARGET);
    if let Some(message) = words.human_words_on_several(&args) {
        eprintln!("{message}");
        return exit::INVALID_INPUT;
    }
    if words.call_the_server(&args) {
        match remote(words.mcp_tool(&args)).await {
            Ok(client) => options = options.remote(client.http()),
            Err(code) => return code,
        }
    }
    let renamed = words.renamed(&args);
    let code = run_cli_process(app, Arc::new(Context::remote()), options, args).await;
    // After an answer or a refusal, which both go to standard output; any other failure keeps standard error as its
    // one JSON line.
    if renamed && matches!(code, exit::VALUE | exit::FAILURE) {
        eprintln!("{}", route::RENAMED_POINTER);
    }
    code
}

pub(crate) async fn remote(mcp_tool: route::McpTool) -> Result<caller::Client, i32> {
    let caller = caller::detect(Binding::Cli)
        .map_err(|error| fail(Kind::InvalidInput, error.to_string()))?;
    let home = commands::home().map_err(|error| {
        fail(
            commands::home_refusal_kind(&error),
            error.for_a_command(&mcp_tool),
        )
    })?;
    let endpoint = background::ensure(&home)
        .await
        .map_err(|error| fail(Kind::Unavailable, error.for_a_command(&mcp_tool)))?;
    caller::client(&caller, &endpoint).map_err(|message| fail(Kind::Internal, message))
}

fn fail(kind: Kind, message: String) -> i32 {
    commands::report_to(&mut |line| eprintln!("{line}"), kind, message)
}
