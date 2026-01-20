ARG ALPINE_VERSION=3.22
ARG RUST_VERSION=1.91.1
ARG CHEF_VERSION=0.1.73

############################## Planning Phase ##############################
FROM lukemathwalker/cargo-chef:${CHEF_VERSION}-rust-${RUST_VERSION}-alpine AS chef
WORKDIR /opt/app

# Create recipe of crate dependencies
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

################################ Build Phase ###############################
FROM chef AS builder

ENV DOCKER_DEPLOYMENT=1

# Install build dependencies (GCC only, no clang)
RUN apk update && apk add --no-cache \
    pkgconfig cmake make g++ ninja git bash curl \
    postgresql-dev dav1d-dev ffmpeg-dev

# Build libjxl from source
WORKDIR /opt/jxl
RUN git clone --depth=1 https://github.com/libjxl/libjxl.git . \
 && mkdir build && cd build \
 && cmake -DCMAKE_BUILD_TYPE=Release \
          -DBUILD_TESTING=OFF \
          -DBUILD_BENCHMARKS=OFF \
          -DBUILD_EXAMPLES=OFF \
          -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
          .. \
 && cmake --build . --target install

# Go back to app
WORKDIR /opt/app
COPY --from=planner /opt/app/recipe.json recipe.json

# Build dependencies (cached)
ARG TARGET_CPU
ARG CODEGEN_OPTIONS="-C target-feature=-crt-static -C target-cpu=$TARGET_CPU"
RUN RUSTFLAGS=${CODEGEN_OPTIONS} cargo chef cook --release --recipe-path recipe.json

# Build application
COPY . .
RUN RUSTFLAGS=${CODEGEN_OPTIONS} cargo build --release

################################# Run Phase ################################
FROM alpine:${ALPINE_VERSION} AS runtime
WORKDIR /opt/app

ENV RUST_BACKTRACE=full

ARG PUID=1000
ARG PGID=1000

# Install runtime libraries
RUN apk update && apk add --no-cache libpq ffmpeg-libavdevice \
    libstdc++ libgcc

# Set permissions
RUN mkdir -p /opt/app /data \
 && addgroup -g ${PGID} app \
 && adduser -SDH -h /opt/app -g '' -G app -u ${PUID} app \
 && chown -R app:app /opt/app /data

# Copy over application binary and libjxl shared libs
COPY --from=builder /usr/local/lib /usr/local/lib
COPY --from=builder /opt/app/target/release/server server

# Run it!
USER app
ENTRYPOINT ["/opt/app/server"]

ARG SERVER_PORT=6666
ENV SERVER_PORT=${SERVER_PORT}
EXPOSE ${SERVER_PORT}

VOLUME ["/data/"]

ARG DOCKER_REPO
ARG BUILD_DATE
ARG SOURCE_COMMIT
LABEL \
    maintainer="" \
    org.opencontainers.image.title="${DOCKER_REPO}" \
    org.opencontainers.image.url="https://github.com/liamw1/oxibooru" \
    org.opencontainers.image.documentation="https://github.com/liamw1/oxibooru/blob/${SOURCE_COMMIT}/docs/INSTALL.md" \
    org.opencontainers.image.created="${BUILD_DATE}" \
    org.opencontainers.image.source="https://github.com/liamw1/oxibooru" \
    org.opencontainers.image.revision="${SOURCE_COMMIT}" \
    org.opencontainers.image.licenses="GPL-3.0"
