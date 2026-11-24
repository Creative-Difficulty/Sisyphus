//! The web page and its assets, built into the binary so the server needs no
//! files next to it.

use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;

use crate::AppState;

const INDEX: &str = include_str!("../web/index.html");
const STYLE: &str = include_str!("../web/style.css");
const SCRIPT: &str = include_str!("../web/app.js");
const THEME_SCRIPT: &str = include_str!("../web/theme.js");
// Pulse, the reviewers' page (`review.rs` serves its data, only to
// reviewers; the page itself holds nothing).
const REVIEW: &str = include_str!("../web/review.html");
const REVIEW_STYLE: &str = include_str!("../web/review.css");
const REVIEW_SCRIPT: &str = include_str!("../web/review.js");
// The same Fredoka files as the client (SIL Open Font License).
/// The logo (a placeholder for now), shared with the app.
const LOGO: &str = include_str!("../../client/assets/logo.svg");
const FONT_REGULAR: &[u8] = include_bytes!("../../client/assets/fonts/Fredoka-Regular.ttf");
const FONT_SEMIBOLD: &[u8] = include_bytes!("../../client/assets/fonts/Fredoka-SemiBold.ttf");
const FONT_LICENSE: &str = include_str!("../../client/assets/fonts/Fredoka-OFL.txt");
// The local prompt classifier the app downloads on first start. The app
// checks its SHA-256, so it never changes under the same name.
const PROMPT_MODEL: &[u8] = include_bytes!("../../../models/sisyphus-prompt-1.bin");
const PROMPT_MODEL_LICENSE: &str =
    include_str!("../../../models/LICENSE-blink-multilingual-64M.txt");

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            get(|| async { asset("text/html; charset=utf-8", INDEX) }),
        )
        // A public profile: the same page, which reads the id from the path.
        .route(
            "/u/{id}",
            get(|| async { asset("text/html; charset=utf-8", INDEX) }),
        )
        .route("/style.css", get(|| async { asset("text/css", STYLE) }))
        .route(
            "/app.js",
            get(|| async { asset("text/javascript", SCRIPT) }),
        )
        .route(
            "/review",
            get(|| async { asset("text/html; charset=utf-8", REVIEW) }),
        )
        .route(
            "/review.css",
            get(|| async { asset("text/css", REVIEW_STYLE) }),
        )
        .route(
            "/review.js",
            get(|| async { asset("text/javascript", REVIEW_SCRIPT) }),
        )
        .route("/logo.svg", get(|| async { asset("image/svg+xml", LOGO) }))
        .route(
            "/theme.js",
            get(|| async { asset("text/javascript", THEME_SCRIPT) }),
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
            "/models/sisyphus-prompt-1.bin",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "application/octet-stream"),
                        (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
                    ],
                    PROMPT_MODEL,
                )
            }),
        )
        .route(
            "/models/LICENSE-blink-multilingual-64M.txt",
            get(|| async { asset("text/plain; charset=utf-8", PROMPT_MODEL_LICENSE) }),
        )
        .route(
            "/fonts/OFL.txt",
            get(|| async { asset("text/plain; charset=utf-8", FONT_LICENSE) }),
        )
}

fn asset(content_type: &'static str, body: impl IntoResponse) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, content_type)], body)
}
