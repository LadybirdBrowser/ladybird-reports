FROM rust:1.88-bookworm AS builder

ARG BUILD_VERSION=0.1.0-dev
ENV LADYBIRD_REPORTS_VERSION=${BUILD_VERSION}

WORKDIR /source
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
COPY templates ./templates
COPY assets ./assets

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
