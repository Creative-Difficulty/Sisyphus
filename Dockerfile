# Sisyphus server.
#
#   docker build -t sisyphus-server .
#   docker run -p 8080:8080 -e DATABASE_URL=postgres://user:pass@host:5432/sisyphus sisyphus-server
#
# The web page, its fonts and the database migrations are built into the
# binary; migrations run on start.

FROM rust:1.95-slim-trixie AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p sisyphus-server

FROM debian:trixie-slim
RUN useradd --system --no-create-home sisyphus
COPY --from=build /src/target/release/sisyphus-server /usr/local/bin/sisyphus-server
# Fredoka is embedded in the binary; ship its licence with it (SIL OFL).
COPY crates/client/assets/fonts/Fredoka-OFL.txt /usr/share/licenses/sisyphus/Fredoka-OFL.txt
USER sisyphus
ENV BIND_ADDR=0.0.0.0:8080
EXPOSE 8080
CMD ["sisyphus-server"]
