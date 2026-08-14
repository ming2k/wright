//! Read-only local HTTP server for `wright graph --web`.
//!
//! Serves the embedded single-page UI plus a `/api/graph` JSON endpoint on
//! 127.0.0.1. The graph document is rebuilt on every request — plan files
//! and the installed-state database are re-read each time, so a running
//! server always reflects current state. There are no mutating routes.

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

use crate::config::GlobalConfig;
use crate::error::{Result, WrightResultExt};

const INDEX_HTML: &str = include_str!("assets/index.html");
const APP_JS: &str = include_str!("assets/app.js");
const STYLE_CSS: &str = include_str!("assets/style.css");

/// Read-only inputs shared by all request handlers. The database is opened
/// per request rather than held open: the server may outlive deployments
/// that replace the file, and a long-lived pool would also hold the
/// database lock.
struct Shared {
    config: GlobalConfig,
    db_path: PathBuf,
    ledger_dir: PathBuf,
}

type Body = Full<Bytes>;

/// Serve the graph UI on an already-bound listener (bound by the caller so
/// it learns the effective address first, e.g. for `--port 0`). Runs until
/// Ctrl-C.
pub async fn serve(
    listener: TcpListener,
    config: GlobalConfig,
    db_path: PathBuf,
    ledger_dir: PathBuf,
) -> Result<()> {
    let addr = listener
        .local_addr()
        .context("failed to read bound address")?;
    crate::outln!("Serving plan graph at http://{}/ (Ctrl-C to stop)", addr);
    let shared = Arc::new(Shared {
        config,
        db_path,
        ledger_dir,
    });
    run(listener, shared, tokio::signal::ctrl_c()).await
}

/// Accept loop: one spawned task per connection, `shutdown` ends the loop.
async fn run(
    listener: TcpListener,
    shared: Arc<Shared>,
    shutdown: impl Future<Output = std::io::Result<()>>,
) -> Result<()> {
    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(pair) => pair,
                    Err(e) => {
                        tracing::warn!("graph server: accept failed: {}", e);
                        continue;
                    }
                };
                let shared = Arc::clone(&shared);
                tokio::spawn(async move {
                    let service = service_fn(move |req| {
                        let shared = Arc::clone(&shared);
                        async move { respond(req, shared).await }
                    });
                    if let Err(e) = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await
                    {
                        tracing::debug!("graph server: connection from {} ended: {}", peer, e);
                    }
                });
            }
        }
    }
    Ok(())
}

async fn respond(
    req: Request<Incoming>,
    shared: Arc<Shared>,
) -> std::result::Result<Response<Body>, Infallible> {
    Ok(route(&req, &shared).await)
}

async fn route(req: &Request<Incoming>, shared: &Shared) -> Response<Body> {
    if req.method() != Method::GET {
        return text_response(StatusCode::NOT_FOUND, "not found");
    }
    match req.uri().path() {
        "/" => asset_response("text/html; charset=utf-8", INDEX_HTML),
        "/app.js" => asset_response("text/javascript; charset=utf-8", APP_JS),
        "/style.css" => asset_response("text/css; charset=utf-8", STYLE_CSS),
        "/api/graph" => match graph_json(shared).await {
            Ok(json) => asset_response("application/json", &json),
            Err(e) => text_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("failed to build graph: {}", e),
            ),
        },
        _ => text_response(StatusCode::NOT_FOUND, "not found"),
    }
}

/// Rebuild the graph document from current state and serialize it.
async fn graph_json(shared: &Shared) -> Result<String> {
    let db = wright_state::database::InstalledDb::open(&shared.db_path, Some(&shared.ledger_dir))
        .await
        .context("failed to open database")?;
    let doc = super::build_graph(&shared.config, &db).await?;
    serde_json::to_string(&doc).context("failed to serialize graph")
}

fn asset_response(content_type: &str, body: &str) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(hyper::header::CONTENT_TYPE, content_type)
        .body(Full::new(Bytes::copy_from_slice(body.as_bytes())))
        .expect("static response is always valid")
}

fn text_response(status: StatusCode, body: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Full::new(Bytes::copy_from_slice(body.as_bytes())))
        .expect("static response is always valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shared state backed by a tempdir: one plan on disk, an empty (but
    /// migrated) database file.
    async fn test_shared(temp: &tempfile::TempDir) -> Arc<Shared> {
        let plans_dir = temp.path().join("plans");
        std::fs::create_dir_all(plans_dir.join("hello")).unwrap();
        std::fs::write(
            plans_dir.join("hello").join("plan.toml"),
            "name = \"hello\"\nversion = \"1.0.0\"\nrelease = 1\ndescription = \"d\"\nlicense = \"MIT\"\narch = \"x86_64\"\n",
        )
        .unwrap();

        let mut config = GlobalConfig::default();
        config.general.plans_dir = plans_dir;
        config.general.extra_plans_dirs = Vec::new();

        let db_path = temp.path().join("wright.db");
        let ledger_dir = temp.path().join("ledger");
        // Run migrations up front so request-time opens see a valid schema.
        wright_state::database::InstalledDb::open(&db_path, Some(&ledger_dir))
            .await
            .unwrap();

        Arc::new(Shared {
            config,
            db_path,
            ledger_dir,
        })
    }

    #[tokio::test]
    async fn serves_ui_assets_and_graph_json() {
        let temp = tempfile::tempdir().unwrap();
        let shared = test_shared(&temp).await;
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(run(listener, shared, async move {
            let _ = rx.await;
            Ok(())
        }));
        let base = format!("http://{}", addr);
        let client = reqwest::Client::new();

        let res = client.get(format!("{}/", base)).send().await.unwrap();
        assert_eq!(res.status(), 200);
        assert!(
            res.headers()
                .get(hyper::header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap()
                .contains("text/html")
        );
        assert!(res.text().await.unwrap().contains("id=\"canvas-wrap\""));

        for (path, content_type) in [("/app.js", "text/javascript"), ("/style.css", "text/css")] {
            let res = client
                .get(format!("{}{}", base, path))
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), 200, "{}", path);
            assert!(
                res.headers()
                    .get(hyper::header::CONTENT_TYPE)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .contains(content_type),
                "{}",
                path
            );
        }

        let res = client
            .get(format!("{}/api/graph", base))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let body: serde_json::Value = res.json().await.unwrap();
        let nodes = body["nodes"].as_array().unwrap();
        assert!(nodes.iter().any(|n| n["name"] == "hello"));
        assert!(body["edges"].is_array());

        let res = client
            .get(format!("{}/no-such-route", base))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 404);

        let _ = tx.send(());
        server.await.unwrap().unwrap();
    }
}
