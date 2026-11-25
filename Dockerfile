# check=skip=InvalidDefaultArgInFrom
#
# That check warns that the ARG below has no default, which is
# deliberate; the reason is at the ARG itself. A parser directive has
# to be the first line of the file, before any other comment, which is
# why the explanation is not here.
#
# The image every cargo invocation in this repository runs inside.
# Rust toolchains are deliberately not installed on the development
# host, which runs several Rust projects at different toolchain
# versions; see docs/development.md.
#
# This is a thin derivation of the official rust image rather than a
# devcontainer. It carries what the build and the lint gate need and
# nothing else: no editor integration, because nothing here consumes
# it yet, and adding it later is additive.
#
# RUST_IMAGE has no default. The Makefile passes it, and that is the
# single place the toolchain version is written -- a default here
# would be a second place to bump and a chance for the two to
# disagree. Build this image with `make image`, not by hand.
ARG RUST_IMAGE
FROM ${RUST_IMAGE}

# The official rust images ship no cargo-fmt or cargo-clippy binary,
# so `cargo fmt` and `cargo clippy` fail with "no such command" until
# the components are added. The lint gate runs both.
RUN rustup component add rustfmt clippy

# The licence gate. --root puts the binary in /usr/local/bin so it is
# on PATH regardless of where CARGO_HOME points below, and --locked
# builds it from its own published lockfile rather than resolving a
# fresh dependency tree at image build time.
RUN cargo install --locked --root /usr/local cargo-deny

# The protobuf compiler, which the control plane's build script calls
# to turn its schema into Rust. It is installed here, when the image
# is built, because this is the one place in this repository allowed
# to reach the network: every compile target runs with --network none
# and against a read-only cargo cache, so a build script that had to
# fetch a compiler would fail there and should.
#
# The distribution's package rather than a release downloaded from
# upstream: an apt package is signed, pinned by the base image's
# suite, and reproducible without this file holding a checksum that
# somebody has to remember to bump. What it costs is that the protobuf
# version is the distribution's choice, which is acceptable for a
# schema using nothing newer than proto3.
#
# --no-install-recommends keeps the documentation and the development
# headers out, and the apt lists go afterwards so they are not carried
# in the layer.
RUN apt-get update \
    && apt-get install --yes --no-install-recommends protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# A writable HOME for the build user. Containers run as the invoking
# user's uid so that target/ and Cargo.lock come out owned by them,
# and that uid does not exist in /etc/passwd here, so it gets no home
# of its own. CARGO_HOME is pointed inside it because the official
# image sets CARGO_HOME=/usr/local/cargo: left alone, the registry
# cache the Makefile mounts under /build/.cargo is shadowed, every
# build downloads into the container layer, and the download is
# thrown away with the container.
RUN mkdir -p /build/.cargo/registry /build/.cargo/git && chmod -R 0777 /build
ENV CARGO_HOME=/build/.cargo
