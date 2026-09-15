FROM rust:1.88-bookworm AS chef

RUN cargo install cargo-chef --locked --version 0.1.78

WORKDIR /source

FROM chef AS planner

COPY . .

RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

COPY --from=planner /source/recipe.json recipe.json

RUN cargo chef cook --locked --release --recipe-path recipe.json \
    --bin admin \
    --bin migrate \
    --bin public_api

COPY . .

ARG BUILD_VERSION=0.1.0-dev
ENV LADYBIRD_REPORTS_VERSION=${BUILD_VERSION}

RUN cargo build --locked --release \
    --bin admin \
    --bin migrate \
    --bin public_api

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 reports \
    && install --directory --owner=reports --group=reports /data/attachments

COPY --from=builder /source/target/release/admin /usr/local/bin/ladybird-reports-admin
COPY --from=builder /source/target/release/migrate /usr/local/bin/ladybird-reports-migrate
COPY --from=builder /source/target/release/public_api /usr/local/bin/ladybird-reports-public-api

USER reports
ENV ATTACHMENT_ROOT=/data/attachments
EXPOSE 3000 3001

CMD ["/usr/local/bin/ladybird-reports-admin"]
