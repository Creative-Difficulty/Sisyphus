//! The web page and its assets, built into the binary so the server needs no
//! files next to it.

use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use sqlx::PgPool;

const INDEX: &str = include_str!("../web/index.html");
const STYLE: &str = include_str!("../web/style.css");
const SCRIPT: &str = include_str!("../web/app.js");
// The same Fredoka files as the client (SIL Open Font License).
const FONT_REGULAR: &[u8] = include_bytes!("../../client/assets/fonts/Fredoka-Regular.ttf");
const FONT_SEMIBOLD: &[u8] = include_bytes!("../../client/assets/fonts/Fredoka-SemiBold.ttf");
const FONT_LICENSE: &str = include_str!("../../client/assets/fonts/Fredoka-OFL.txt");

pub fn routes() -> Router<PgPool> {
    Router::new()
        .route(
            "/",
            get(|| async { asset("text/html; charset=utf-8", INDEX) }),
        )
        .route("/style.css", get(|| async { asset("text/css", STYLE) }))
        .route(
            "/app.js",
            get(|| async { asset("text/javascript", SCRIPT) }),
        )
        .route(
            "/fonts/Fredoka-Regular.ttf",
            get(|| async { asset("font/ttf", FONT_REGULAR) }),
        )
        .route(
            "/fonts/Fredoka-SemiBold.ttf",
            get(|| async { asset("font/ttf", FONT_SEMIBOLD) }),
        )
        .route(
            "/fonts/OFL.txt",
            get(|| async { asset("text/plain; charset=utf-8", FONT_LICENSE) }),
        )
}

fn asset(content_type: &'static str, body: impl IntoResponse) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, content_type)], body)
}
