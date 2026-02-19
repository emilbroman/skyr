FROM clux/muslrust:stable AS chef
WORKDIR /src
RUN cargo install --locked cargo-chef

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates/cdb/Cargo.toml crates/cdb/Cargo.toml
COPY crates/de/Cargo.toml crates/de/Cargo.toml
COPY crates/plugin_std_random/Cargo.toml crates/plugin_std_random/Cargo.toml
COPY crates/rdb/Cargo.toml crates/rdb/Cargo.toml
COPY crates/rtp/Cargo.toml crates/rtp/Cargo.toml
COPY crates/rte/Cargo.toml crates/rte/Cargo.toml
COPY crates/rtq/Cargo.toml crates/rtq/Cargo.toml
COPY crates/scl/Cargo.toml crates/scl/Cargo.toml
COPY crates/sclc/Cargo.toml crates/sclc/Cargo.toml
COPY crates/scs/Cargo.toml crates/scs/Cargo.toml
RUN set -eu; \
    mkdir -p crates/cdb/src crates/de/src crates/plugin_std_random/src crates/rdb/src crates/rtp/src crates/rte/src crates/rtq/src crates/scl/src crates/sclc/src crates/scs/src; \
    printf 'pub fn _stub() {}\n' > crates/cdb/src/lib.rs; \
    printf 'fn main() {}\n' > crates/de/src/main.rs; \
    printf 'fn main() {}\n' > crates/plugin_std_random/src/main.rs; \
    printf 'pub fn _stub() {}\n' > crates/rdb/src/lib.rs; \
    printf 'pub fn _stub() {}\n' > crates/rtp/src/lib.rs; \
    printf 'fn main() {}\n' > crates/rte/src/main.rs; \
    printf 'pub fn _stub() {}\n' > crates/rtq/src/lib.rs; \
    printf 'fn main() {}\n' > crates/scl/src/main.rs; \
    printf 'pub fn _stub() {}\n' > crates/sclc/src/lib.rs; \
    printf 'fn main() {}\n' > crates/scs/src/main.rs
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS deps
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

FROM deps AS build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN set -eu; \
    cargo build --release -p scs -p de -p rte -p plugin_std_random; \
    mkdir -p /artifacts; \
    for bin in scs de rte plugin_std_random; do \
      path="$(find /src /home/rust /root /volume /target -type f -path "*/release/${bin}" 2>/dev/null | head -n1 || true)"; \
      if [ -z "${path}" ]; then \
        echo "failed to locate built binary: ${bin}" >&2; \
        exit 1; \
      fi; \
      cp "${path}" "/artifacts/${bin}"; \
    done

FROM scratch
COPY --from=build /artifacts/scs /scs
COPY --from=build /artifacts/de /de
COPY --from=build /artifacts/rte /rte
COPY --from=build /artifacts/plugin_std_random /plugin_std_random
