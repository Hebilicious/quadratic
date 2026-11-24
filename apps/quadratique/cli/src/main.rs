mod background;
mod caller;
mod commands;
mod init;
mod mcp;
mod route;
mod shipped;

use std::sync::Arc;

use acid_cat_core::Kind;
use acid_cat_core::cli::{CliOptions, CliOutput, OutputForm, run_cli_process};
use acid_cat_core::http::HttpClient;
use quadratique_server::app::{CATALOG_JSON, Context, application};
use quadratique_server::domain::Binding;
use quadratique_server::server::TARGET;

fn main() {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("quadratique: no async runtime: {error}");
            std::process::exit(acid_cat_core::cli::exit::INTERNAL);
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
    if words.call_the_server(&args) {
        match remote(words.mcp_tool(&args)).await {
            Ok(client) => options = options.remote(client),
            Err(code) => return code,
        }
    }
    run_cli_process(app, Arc::new(Context::remote()), options, args).await
}

pub(crate) async fn remote(mcp_tool: route::McpTool) -> Result<HttpClient, i32> {
    let caller = caller::detect(Binding::Cli)
        .map_err(|error| fail(Kind::InvalidInput, error.to_string()))?;
    let home = commands::home().map_err(|message| fail(Kind::Internal, message))?;
    let runtime = background::ensure(&home)
        .await
        .map_err(|error| fail(Kind::Unavailable, error.for_a_command(&mcp_tool)))?;
    caller::client(&caller, &runtime.url).map_err(|message| fail(Kind::Internal, message))
}

fn fail(kind: Kind, message: String) -> i32 {
    commands::report_to(&mut |line| eprintln!("{line}"), kind, message)
}
