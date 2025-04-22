FROM docker.io/rust:1.86.0-alpine@sha256:541a1720c1cedddae9e17b4214075bf57c20bc7b176b4bba6bce3437c44d51ef AS build

## cargo package name: customize here or provide via --build-arg
ARG pkg=pollux

WORKDIR /build

COPY src/ src/ 
COPY migrations/ migrations/ 
COPY Cargo.toml Cargo.toml 
COPY Cargo.lock Cargo.lock

ENV OPENSSL_STATIC=1

RUN --mount=type=cache,target=/build/target \
	--mount=type=cache,target=/usr/local/cargo/registry \
	--mount=type=cache,target=/usr/local/cargo/git \
	set -eux && \
	apk add --no-cache \
	musl-dev \
	openssl-dev \
	pkgconf \
	libgcc \
	libstdc++ \
	build-base \
	cmake \
	perl \
	coreutils \
	&& \
	cargo build --release && \
	objcopy --compress-debug-sections target/release/$pkg ./main

################################################################################

FROM alpine:3.21.3@sha256:a8560b36e8b8210634f77d9f7f9efd7ffa463e380b75e2e74aff4511df3ef88c

WORKDIR /app

## copy the main binary
COPY --from=build /build/main ./

## copy runtime assets which may or may not exist
#COPY --from=build /build/Rocket.tom[l] ./static
#COPY --from=build /build/stati[c] ./static
#COPY --from=build /build/template[s] ./templates

## ensure the container listens globally on port 8080
ENV ROCKET_ADDRESS=0.0.0.0
ENV ROCKET_PORT=8080
ENV RUST_LOG=INFO

CMD [ "/app/main" ]
