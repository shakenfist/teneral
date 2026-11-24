#!/bin/bash
#
# Check the shape of the binding store's traits, mechanically.
#
# Two rules about the store's interface are worth more than the
# comments that state them, because both fail silently when they are
# broken and both are broken by adding something rather than by
# changing anything:
#
#   1. Every method names the network it applies to, unless the trait's
#      own documentation enumerates it as store-global. A method that
#      takes an address and lets the implementation scope by network is
#      the shape that leaks between networks, and it passes every
#      single-network test ever written.
#
#   2. Every mutating method returns the position it was assigned
#      rather than nothing. A write that returned `()` would be a write
#      whose event nobody could find -- and a store whose history
#      silently misses a change is the project's signature failure
#      mode, in the layer meant to prevent it.
#
# The enumerated exceptions are read out of the trait's doc comment, so
# the list a reader sees and the list this script trusts cannot drift
# apart: there is only one. A name enumerated that is not a method of
# the trait is reported too, because a stale exception is an exception
# nobody is checking.
#
# Re-runnable, and run by hand rather than from pre-commit: it parses
# Rust with awk, which is adequate for a declaration-only trait and is
# not a thing to put in front of every commit in the repository.
#
# Usage: scripts/check-store-trait.sh [path/to/store.rs]

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
file="${1:-$root/teneral-store/src/store.rs}"

if [ ! -f "$file" ]; then
    echo "$(basename "$0"): no such file: $file" >&2
    exit 2
fi

status=0
awk -v file="$file" '
# The doc comment of whatever comes next, kept line by line so that
# the enumerated exceptions can be read out of one paragraph of it.
/^[[:space:]]*\/\/\// {
    line = $0
    sub(/^[[:space:]]*\/\/\/[[:space:]]?/, "", line)
    doc[++doclines] = line
    next
}

# An attribute sits between a doc comment and the item it documents,
# so it must not clear the comment. Anything else does -- which is why
# the rule that clears it comes *after* the one that reads it below:
# awk runs every matching rule in the order they are written, so a
# clearing rule placed first would wipe the comment on the very line
# that wanted it.
/^[[:space:]]*#\[/ { next }

/^pub trait [A-Za-z_][A-Za-z0-9_]*/ {
    trait = $3
    sub(/[^A-Za-z0-9_].*$/, "", trait)
    traits++
    delete global
    globals = 0

    # The enumerated store-global methods: the paragraph that starts
    # with the marker, up to the first blank line of the comment.
    for (i = 1; i <= doclines; i++) {
        if (doc[i] ~ /Store-global methods, enumerated/) {
            for (j = i; j <= doclines && doc[j] != ""; j++) {
                rest = doc[j]
                while (match(rest, /`[A-Za-z_][A-Za-z0-9_]*`/)) {
                    name = substr(rest, RSTART + 1, RLENGTH - 2)
                    if (!(name in global)) { global[name] = 1; globals++ }
                    rest = substr(rest, RSTART + RLENGTH)
                }
            }
            break
        }
    }

    printf "%s: trait %s\n", file, trait
    if (globals == 0) {
        printf "  no store-global methods are enumerated\n"
    } else {
        printf "  store-global, as the trait documents:"
        for (name in global) { printf " %s", name }
        printf "\n"
    }

    in_trait = 1
    doclines = 0
    delete doc
    next
}

{ doclines = 0; delete doc }

in_trait && /^}/ {
    for (name in global) {
        if (!(name in seen)) {
            printf "  %-26s FAIL  enumerated as store-global but not a method here\n", name
            failed++
        }
    }
    delete seen
    in_trait = 0
    next
}

in_trait && !collecting && /^[[:space:]]*(async[[:space:]]+)?fn[[:space:]]/ {
    collecting = 1
    signature = ""
}

collecting {
    signature = signature " " $0
    if ($0 ~ /;[[:space:]]*$/) {
        collecting = 0
        check(signature)
    }
    next
}

function check(signature,    name, returns, scope, mutating, verdict, at) {
    gsub(/[[:space:]]+/, " ", signature)

    match(signature, / fn [A-Za-z_][A-Za-z0-9_]*/)
    name = substr(signature, RSTART + 4, RLENGTH - 4)
    seen[name] = 1
    checked++

    at = index(signature, "->")
    if (at == 0) {
        returns = "()"
    } else {
        returns = substr(signature, at + 2)
        sub(/;[[:space:]]*$/, "", returns)
        gsub(/^ +| +$/, "", returns)
    }

    scope = (signature ~ /[,(] ?network ?:/) ? "network" : "global"
    mutating = (name ~ /^(create|delete|update|set|attach|detach|claim|release|revoke|renew|allocate|truncate|evict)([_A-Z]|$)/)

    verdict = "ok"
    if (scope == "global" && !(name in global)) {
        verdict = "FAIL  takes no network and is not enumerated as store-global"
        failed++
    } else if (returns == "()" || returns == "Result<()>") {
        verdict = "FAIL  returns nothing, so no caller can learn its position"
        failed++
    } else if (mutating && returns !~ /Cursor/) {
        verdict = "FAIL  mutates but does not return the cursor it was assigned"
        failed++
    }

    printf "  %-26s %-8s -> %-28s %s\n", name, scope, returns, verdict
}

END {
    printf "%d method(s) checked across %d trait(s), ", checked, traits
    if (failed > 0) {
        printf "%d failed.\n", failed
        exit 1
    }
    printf "none failed.\n"
}
' "$file" || status=$?

exit "$status"
