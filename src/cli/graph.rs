use clap::Args;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::{Result, WrightError, WrightResultExt};

const WRIGHT_GRAPH_AFTER_HELP: &str = "\
Examples:
  wright graph
  wright graph --web
  wright graph --web --port 0
  wright graph --web --no-open

By default a terminal summary lists every discovered plan with its dependency
edges (grouped by domain) and installed-state marker, followed by totals.
With --web a read-only local HTTP server (127.0.0.1 only) serves an
interactive graph UI; the plan index and database are re-read on every
request, so the view always reflects current state.";

#[derive(Args)]
#[command(
    long_about = "Show the global plan relationship graph, either as a terminal summary or as an interactive web UI (--web).",
    after_help = WRIGHT_GRAPH_AFTER_HELP
)]
pub struct GraphArgs {
    /// Serve an interactive web UI instead of terminal output
    #[arg(long)]
    pub web: bool,
    /// Port for --web (0 = random)
    #[arg(long, default_value = "8642", requires = "web")]
    pub port: u16,
    /// Do not open the browser automatically
    #[arg(long, requires = "web")]
    pub no_open: bool,
}

#[cfg(with_handlers)]
pub async fn run(args: GraphArgs, ctx: &Context<'_>) -> Result<()> {
    if args.web {
        // Bind here (not inside serve) so the effective address is known
        // before the browser is opened — with --port 0 the OS picks it.
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, args.port))
            .await
            .context(format!("failed to bind 127.0.0.1:{}", args.port))?;
        let addr = listener
            .local_addr()
            .map_err(|e| WrightError::context("failed to read bound address", e))?;
        if !args.no_open {
            // Best-effort: a missing xdg-open (headless system) is not an error.
            let _ = std::process::Command::new("xdg-open")
                .arg(format!("http://{}/", addr))
                .spawn();
        }
        let ledger_dir = crate::ledger::dir(ctx.config, Some(&ctx.db_path));
        return crate::graph::server::serve(
            listener,
            ctx.config.clone(),
            ctx.db_path.clone(),
            ledger_dir,
        )
        .await;
    }

    let db = ctx.open_db().await?;
    let doc = crate::graph::build_graph(ctx.config, &db).await?;
    crate::graph::render_terminal(&doc);
    Ok(())
}
