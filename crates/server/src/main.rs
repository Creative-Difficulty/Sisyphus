//! Sisyphus server: accepts signed heartbeat uploads, stores them in
//! Postgres, and serves a small read-only web dashboard.
//!
//! Configuration comes from the environment (or a `.env` file):
//! - `DATABASE_URL`: Postgres connection URL (required)
//! - `BIND_ADDR`: address to listen on (default `0.0.0.0:8080`)

use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::routing::{get, post};
use sqlx::postgres::PgPoolOptions;

mod dashboard;
mod upload;
mod web;

#[tokio::main]
async fn main() {
    // A missing .env is fine; the variables may come from the environment.
    let _ = dotenvy::dotenv();

    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        eprintln!("DATABASE_URL is not set (see .env.example)");
        std::process::exit(1);
    });
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());

    let db = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
        .unwrap_or_else(|e| {
            eprintln!("could not connect to the database: {e}");
            std::process::exit(1);
        });
    if let Err(e) = sqlx::migrate!("./migrations").run(&db).await {
        eprintln!("could not run database migrations: {e}");
        std::process::exit(1);
    }

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/upload", post(upload::upload))
        .route("/api/dashboard", get(dashboard::dashboard))
        .merge(web::routes())
        .with_state(db);

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("could not listen on {bind_addr}: {e}");
            std::process::exit(1);
        });
    println!("sisyphus server listening on {bind_addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("server error");
}

/// The server's clock (unix ms). This is the trusted time.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
