# build dve
FROM rust:trixie AS builder

WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends cmake && rm -rf /var/lib/apt/lists/*
COPY . .

WORKDIR /app/lib/parser
RUN patch --reject-file=- -N -p1 <../../sendtable.patch || :

WORKDIR /app
RUN cargo build --release

# make image
FROM cgr.dev/chainguard/wolfi-base
RUN apk add --no-cache libstdc++

WORKDIR /dve
COPY --from=builder \
     /app/target/release/demo-voice-extract \
     /app/lib/celt/libtier0_client.so \
     /app/lib/celt/vaudio_celt_client.so \
     .

WORKDIR /data

LABEL org.opencontainers.image.source=https://github.com/xkti/demo-voice-extract
ENTRYPOINT ["/dve/demo-voice-extract"]
