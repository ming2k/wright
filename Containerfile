FROM alpine:latest AS builder

RUN apk add --no-cache \
    rust \
    cargo \
    musl-dev \
    sqlite-dev \
    xz-dev \
    bzip2-dev \
    zstd-dev \
    perl \
    make

WORKDIR /src
COPY . .

RUN cargo build --release

FROM alpine:latest

RUN apk add --no-cache \
    sqlite-libs \
    xz-libs \
    bzip2-libs \
    zstd-libs \
    bubblewrap

COPY --from=builder /src/target/release/wright /usr/bin/
COPY --from=builder /src/target/release/wright-build /usr/bin/
COPY --from=builder /src/target/release/wright-repo /usr/bin/

ENTRYPOINT ["wright"]
