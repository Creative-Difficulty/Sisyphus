# Sisyphus server.
#
# Production uses deploy/compose.yaml (Postgres + this image on port 80).
# Standalone:
#   docker build -t sisyphus-server .
#   docker run -p 8080:8080 -e DATABASE_URL=postgres://user:pass@host:5432/sisyphus sisyphus-server
#
# The web page, its fonts and the database migrations are built into the
# binary; migrations run on start.

FROM rust:1.95-slim-trixie AS build
WORKDIR /src
COPY . .
# Cache downloaded crates and build output between builds, so a redeploy
# only recompiles what changed. The binary is copied out of the cache.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p sisyphus-server \
    && cp target/release/sisyphus-server /usr/local/bin/sisyphus-server

FROM debian:trixie-slim
RUN useradd --system --no-create-home sisyphus
COPY --from=build /usr/local/bin/sisyphus-server /usr/local/bin/sisyphus-server
# Fredoka is embedded in the binary; ship its licence with it (SIL OFL).
COPY crates/client/assets/fonts/Fredoka-OFL.txt /usr/share/licenses/sisyphus/Fredoka-OFL.txt
USER sisyphus
ENV BIND_ADDR=0.0.0.0:8080
EXPOSE 8080
CMD ["sisyphus-server"]
