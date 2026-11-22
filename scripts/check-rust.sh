#!/bin/bash
#
# The Rust half of pre-commit, as a thin shim onto the Makefile.
#
# Nothing Rust is installed on the development host, so every check
# here happens inside the build container, and the Makefile is the one
# place that knows how to start one. A hook that spelled out its own
# `docker run` would be a second copy of the cache mounts, the uid
# juggling and the offline discipline, free to drift from the first.
#
# Two modes, because pre-commit gets only the deterministic checks:
#
#   lint - rustfmt and clippy over the whole workspace.
#   deny - the cargo-deny checks whose verdict is a function of
#          Cargo.lock alone. Advisories are deliberately excluded:
#          their answer changes when somebody publishes to the RustSec
#          database, so as a commit hook they would fail a commit that
#          touches no Rust at all for a reason the committer did not
#          cause. `make deny` asks the whole question, advisories
#          included, and a scheduled lane runs it.

set -euo pipefail

usage() {
    echo "usage: $(basename "$0") {lint|deny}" >&2
    exit 2
}

[ "$#" -eq 1 ] || usage

case "$1" in
    lint) target=lint ;;
    deny) target=deny-offline ;;
    *) usage ;;
esac

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec make -C "$root" "$target"
