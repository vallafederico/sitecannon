//! Local HTTP API over the tool registry, for agents and scripts that prefer HTTP to MCP.
//!
//!   GET  /tools                      tool list with JSON input schemas
//!   POST /tools/{name}               run a tool; body = JSON arguments
//!   GET  /projects                   same as the list_projects tool
//!   GET  /projects/{name}/report     the project's HTML report

use std::net::SocketAddr;

use anyhow::Result;
use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::tools;

async fn run_tool(name: String, args: Value) -> Response {
    let result = tokio::task::spawn_blocking(move || tools::call(&name, &args)).await;
    match result {
        Ok(Ok(v)) => Json(json!({"ok": true, "result": v})).into_response(),
        Ok(Err(e)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"ok": false, "error": format!("{e:#}")})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"ok": false, "error": e.to_string()})),
        )
            .into_response(),
    }
}

pub async fn serve(addr: SocketAddr) -> Result<()> {
    let app = Router::new()
        .route(
            "/",
            get(|| async {
                Json(json!({"name": "sitecannon", "version": env!("CARGO_PKG_VERSION"), "tools": "/tools"}))
            }),
        )
        .route("/tools", get(|| async { Json(tools::list()) }))
        .route(
            "/tools/{name}",
            post(|Path(name): Path<String>, body: Option<Json<Value>>| async move {
                run_tool(name, body.map(|Json(v)| v).unwrap_or_else(|| json!({}))).await
            }),
        )
        .route("/projects", get(|| run_tool("list_projects".into(), json!({}))))
        .route(
            "/projects/{name}/report",
            get(|Path(name): Path<String>| async move {
                let html = tools::resolve_project(&name)
                    .and_then(|p| Ok(std::fs::read_to_string(p.join("report.html"))?));
                match html {
                    Ok(h) => Html(h).into_response(),
                    Err(e) => (
                        StatusCode::NOT_FOUND,
                        [(header::CONTENT_TYPE, "text/plain")],
                        format!("{e:#}"),
                    )
                        .into_response(),
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!(
        "SiteCannon API on http://{addr}  (projects root: {})",
        tools::projects_root().display()
    );
    axum::serve(listener, app).await?;
    Ok(())
}
