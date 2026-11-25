# Teneral - build and development targets.
#
# Every cargo invocation here runs inside a container. Rust toolchains
# are deliberately not installed on the development host, which runs
# several Rust projects at different toolchain versions, and a host
# that happens to carry a distribution rustc is still not used: there
# is no `cargo` in any recipe below that is not preceded by a
# `docker run`. `make build` prints the compiler it used for exactly
# that reason. See docs/development.md.

# The build image, and the single place the toolchain version is
# written. The Dockerfile takes it as a build argument with no default
# of its own, so bumping Rust is a one-line diff here and the two
# cannot end up disagreeing. Renovate's dockerfile manager cannot see
# a tag passed this way, so the bump is deliberate rather than
# automatic -- which is the behaviour wanted for a compiler.
RUST_IMAGE := rust:1.100.0-trixie
TENERAL_IMAGE := teneral-build

# Cargo download cache, bind-mounted into the container so crates are
# not re-downloaded on every build. Override CARGO_CACHE to point at a
# location outside the checkout that survives across CI runs: the
# in-checkout default is deleted by `actions/checkout`, whose default
# `clean: true` runs `git clean -ffdx` and so removes this gitignored
# directory every run. Resolved to an absolute path so an override may
# itself be absolute. `make clean` only removes the cache when it lies
# inside the checkout, so pointing this at a shared directory is safe.
CARGO_CACHE ?= .cargo-cache
# `?=` treats CARGO_CACHE= in the environment as set, so an override
# fed by an unset CI variable leaves this empty. Every target degrades
# badly on that -- the mounts become -v "/registry":..., the mkdir
# rule becomes `mkdir -p /registry`, and `clean` would expand to
# `rm -rf /` -- and the resulting permission error names none of it.
# One check at parse time covers the lot.
ifeq ($(strip $(CARGO_CACHE)),)
$(error CARGO_CACHE is empty; unset it or give it a path)
endif
CARGO_CACHE_DIR := $(abspath $(CARGO_CACHE))

# Run as the invoking user so target/ and Cargo.lock come out owned by
# them rather than by root.
UID := $(shell id -u)
GID := $(shell id -g)

# Shared pieces of the docker-run invocation. HOME is a world-writable
# directory the image creates, because the uid above has no entry in
# the container's /etc/passwd and so no home of its own; the image
# also sets CARGO_HOME inside it, which is what makes the cache mounts
# below take effect (the official rust image points CARGO_HOME at
# /usr/local/cargo, and a cache mounted anywhere else is then simply
# ignored). CARGO_BUILD_JOBS is forwarded only when set in the
# caller's environment, so parallelism can be bounded on a small
# machine; docker omits the variable entirely when it is unset.
DOCKER_BASE_ARGS := \
	-v "$(CURDIR)":/workspace \
	-w /workspace \
	-u $(UID):$(GID) \
	-e HOME=/build \
	-e CARGO_BUILD_JOBS
CACHE_MOUNTS := \
	-v "$(CARGO_CACHE_DIR)/registry":/build/.cargo/registry \
	-v "$(CARGO_CACHE_DIR)/git":/build/.cargo/git
CACHE_MOUNTS_RO := \
	-v "$(CARGO_CACHE_DIR)/registry":/build/.cargo/registry:ro \
	-v "$(CARGO_CACHE_DIR)/git":/build/.cargo/git:ro

# Networked invocation with a writable cache, for the targets that
# download but compile nothing: `fetch` and `lock`, which resolve and
# download only.
DOCKER_RUN := docker run --rm $(DOCKER_BASE_ARGS) $(CACHE_MOUNTS)

# Offline invocation for every target that compiles crates. A build
# script (build.rs) runs arbitrary code at compile time for each
# dependency in the tree -- the supply-chain attack surface. Two
# defences: --network none severs the network namespace, so a
# compromised dependency cannot reach a command-and-control host or
# exfiltrate anything (its download call fails and the build aborts
# loudly); and the cache is mounted read-only, so a build script
# cannot poison it for later runs. Crates must be pre-populated by
# `make fetch`, the one target allowed network, which every offline
# target below depends on.
DOCKER_RUN_OFFLINE := docker run --rm --network none $(DOCKER_BASE_ARGS) $(CACHE_MOUNTS_RO)

.PHONY: all image ensure-cache fetch lock build release test lint deny deny-offline clean clean-image help

all: build

help:
	@echo "Teneral build targets:"
	@echo "  make image    - Build the container image everything else runs in"
	@echo "  make fetch    - Pre-download crates into the cargo cache"
	@echo "  make lock     - Refresh Cargo.lock after a dependency change"
	@echo "  make build    - Build debug binaries"
	@echo "  make release  - Build optimised binaries, as a release does"
	@echo "  make test     - Run the test suite"
	@echo "  make lint     - Run rustfmt and clippy checks"
	@echo "  make deny     - Run every cargo-deny check, advisories included"
	@echo "  make deny-offline - Run the cargo-deny checks that need no network"
	@echo "  make clean    - Remove build artifacts"

# Build the image the rest of this file runs in. The Dockerfile copies
# nothing, so the build context is the Dockerfile itself rather than
# the checkout: sending the whole tree, target/ included, to the
# docker daemon on every invocation would cost far more than the image
# build does.
image:
	docker build --build-arg RUST_IMAGE=$(RUST_IMAGE) -t $(TENERAL_IMAGE) - < Dockerfile

# Create the cargo cache directories.
$(CARGO_CACHE)/registry $(CARGO_CACHE)/git:
	mkdir -p $@

# Ensure the cargo cache directories are writable by the build user.
# A previous root-owned docker run can leave them owned by root, and
# the failure that follows is a permission error deep inside cargo
# that names none of this.
ensure-cache: image $(CARGO_CACHE)/registry $(CARGO_CACHE)/git
	@if [ ! -w "$(CARGO_CACHE)/registry" ] || [ ! -w "$(CARGO_CACHE)/git" ]; then \
		echo "Fixing cargo cache permissions..."; \
		docker run --rm \
			-v "$(CARGO_CACHE_DIR)":/cache \
			$(TENERAL_IMAGE) \
			chown -R $(UID):$(GID) /cache; \
	fi

# Populate the cargo cache. This is the ONLY target permitted network
# access to a registry. `cargo fetch` downloads every crate named in
# Cargo.lock but compiles nothing, so no build script runs here -- the
# untrusted code only executes later, offline, in the compile targets
# below. --locked additionally refuses to proceed if Cargo.lock is
# stale, so a dependency edit that has not been through `make lock`
# fails here and says so.
fetch: ensure-cache
	$(DOCKER_RUN) $(TENERAL_IMAGE) cargo fetch --locked

# Refresh Cargo.lock after editing a Cargo.toml. The only target
# allowed to write the lockfile: `fetch` passes --locked and every
# compile passes --frozen, so without this there is no in-container
# path to a lockfile update at all, and the first `make build` after
# adding or bumping a dependency fails at `fetch` with "the lock file
# needs to be updated but --locked was passed". Still runs no build
# script -- `cargo fetch` resolves and downloads, it does not compile.
lock: ensure-cache
	$(DOCKER_RUN) $(TENERAL_IMAGE) cargo fetch

# Build debug binaries. `rustc --version` first so the build output
# records which compiler produced it: the development host carries a
# distribution Rust of its own, and this is how a reader tells at a
# glance that it was not the one used.
build: fetch
	$(DOCKER_RUN_OFFLINE) \
		$(TENERAL_IMAGE) \
		sh -c "rustc --version && cargo build --frozen --workspace"

# Build optimised binaries. This is what the release workflow runs, and
# the only reason it is a separate target from `build`: a release must
# not ship a debug binary, and the workflow should not have to know the
# cargo flags that distinguish the two. Offline and against the
# read-only cache like every other compile target, so a dependency's
# build script still runs with no network.
release: fetch
	$(DOCKER_RUN_OFFLINE) \
		$(TENERAL_IMAGE) \
		sh -c "rustc --version && cargo build --release --frozen --workspace"

test: fetch
	$(DOCKER_RUN_OFFLINE) \
		$(TENERAL_IMAGE) \
		cargo test --frozen --workspace

# The lint gate: formatting, then clippy over every target in the
# workspace with warnings as errors, then the store on its own.
#
# --all-targets reaches tests, benches and examples, which the default
# build does not, and -D warnings is what promotes a `warn` lint to a
# gate -- so a lint left at `warn` in Cargo.toml still blocks here.
# This target is also the only place clippy runs at all: `make build`
# invokes rustc, which does not evaluate clippy's lints however they
# are set in Cargo.toml.
#
# The last step is the store compiled by itself, and it is here because
# it cannot be anywhere else. The store depends on tokio for its
# channels and takes no runtime feature, which is what makes
# `spawn_blocking` not exist inside it -- the rule that the store owns
# the threads its blocking happens on, enforced by a feature list
# rather than by vigilance. Cargo unifies a dependency's features
# across everything it is building at once, so a sibling crate that
# does take a runtime feature puts `spawn_blocking` back within the
# store's reach in a --workspace build. Built alone the crate gets only
# its own features, so this is the one invocation where writing one in
# the store fails. It is in this target rather than a target of its own
# so that pre-commit and CI run it without being told to.
lint: fetch
	$(DOCKER_RUN_OFFLINE) \
		$(TENERAL_IMAGE) \
		sh -c "cargo fmt --all --check && cargo clippy --frozen --workspace --all-targets -- -D warnings && cargo check --frozen -p teneral-store"

# The supply-chain gate, in two halves, because only one of them is
# deterministic.
#
# `deny-offline` runs the three checks whose answer is a function of
# the tree alone: licences, bans and sources. The same Cargo.lock
# gives the same verdict today and next month, so this half is a
# commit hook (see .pre-commit-config.yaml). It is also the half that
# enforces the licence constraint, which is why deny.toml exists at
# all: teneral is Apache-2.0 and may not acquire a copyleft
# dependency. --frozen is cargo-deny's own spelling of --locked plus
# --offline, so this needs no network and runs against the read-only
# cache like every other non-downloading target.
#
# `deny` runs those three plus `advisories`, which compares the tree
# against the RustSec database and so needs both network -- to clone
# or update that database -- and a cache it can write. Its answer
# changes when somebody publishes an advisory, which is exactly why
# it is not a commit hook: as one it would fail a commit that touches
# no Rust at all, for a reason the committer did not cause. This is
# the target to run when you want to ask the whole question
# deliberately, and the one a scheduled supply-chain lane runs.
deny-offline: fetch
	$(DOCKER_RUN_OFFLINE) \
		$(TENERAL_IMAGE) \
		cargo deny --frozen check licenses bans sources

deny: ensure-cache
	$(DOCKER_RUN) $(TENERAL_IMAGE) cargo deny --locked check all

# Clean build artifacts.
#
# The cargo cache is only removed when it lives inside the checkout.
# CARGO_CACHE may point at a shared directory that outlives this
# checkout (see its comment at the top of this file), and `clean` has
# no business deleting that. An empty CARGO_CACHE is rejected at parse
# time, so this recipe only has to decide in-tree versus out-of-tree.
clean:
	rm -rf target/
	@case "$(CARGO_CACHE_DIR)/" in \
		"$(CURDIR)"/?*) rm -rf "$(CARGO_CACHE_DIR)/" ;; \
		*) echo "Kept $(CARGO_CACHE_DIR) (not below $(CURDIR); delete by hand)" ;; \
	esac

clean-image:
	docker rmi -f $(TENERAL_IMAGE) 2>/dev/null || true
