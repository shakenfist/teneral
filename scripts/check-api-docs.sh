#!/bin/bash
#
# Check that the control API's reference page and its schema agree
# about which operations exist, mechanically.
#
# The wire's operations are the fields of the `kind` oneof of `Request`
# in `teneral-api/proto/teneral.v1.proto`, and `docs/control-api.md` is
# where a person learns what each one needs and what it refuses. Two
# rules keep the two from drifting apart, and both fail silently when
# broken, because a page that is wrong looks exactly like a page that
# is right:
#
#   1. Every operation in the schema is named on the page. An operation
#      added to the schema is added to the wire the moment it is built,
#      and a page that does not mention it is a page whose reader will
#      conclude it is not there -- or, worse, will use it without
#      knowing which capability it needs or what scope rule it carries.
#
#   2. Every operation the page names is in the schema. A page that
#      documents an operation which was renamed or never built tells a
#      consumer to send a request the daemon will refuse as unknown.
#
# An operation is "named on the page" for the first rule when its name
# appears anywhere in backticks. For the second rule the page's
# operations are the first cell of each row of a table in the section
# headed "The operations", up to the next section: that section is the
# one place the page enumerates them, and reading every backticked word
# on the page instead would take capabilities, refusal codes and flags
# for operations. The section's own title is therefore load-bearing; if
# it is renamed this script reports that it found no operations there,
# rather than passing.
#
# Re-runnable, and run by hand rather than from pre-commit, like
# `check-store-trait.sh`: it parses a schema and a page with awk, which
# is adequate for these two files and is not a thing to put in front of
# every commit in the repository.
#
# Usage: scripts/check-api-docs.sh [path/to/teneral.v1.proto [path/to/control-api.md]]

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
schema="${1:-$root/teneral-api/proto/teneral.v1.proto}"
page="${2:-$root/docs/control-api.md}"

for file in "$schema" "$page"; do
    if [ ! -f "$file" ]; then
        echo "$(basename "$0"): no such file: $file" >&2
        exit 2
    fi
done

# The operations in the schema: the fields of the oneof inside
# `message Request`, whose type is a message and whose name is the
# operation's. Comments are skipped, so a field mentioned in one is not
# counted.
in_schema="$(awk '
    /^message Request[[:space:]]*\{/ { inside = 1; next }
    inside && /^\}/ { inside = 0 }
    inside && /^[[:space:]]*[A-Za-z0-9_.]+[[:space:]]+[a-z][a-z0-9_]*[[:space:]]*=[[:space:]]*[0-9]+;/ {
        print $2
    }
' "$schema" | sort -u)"

# The operations the page enumerates: the first cell of each table row
# between the "The operations" heading and the next level-two heading,
# when that cell is exactly one backticked snake_case name.
on_page="$(awk '
    /^## / { inside = ($0 == "## The operations") ; next }
    inside && /^\| `[a-z][a-z0-9_]*` \|/ {
        name = $2
        gsub(/`/, "", name)
        print name
    }
' "$page" | sort -u)"

if [ -z "$in_schema" ]; then
    echo "$(basename "$0"): found no operations in the schema's Request message: $schema" >&2
    exit 2
fi

if [ -z "$on_page" ]; then
    echo "$(basename "$0"): found no operations in the page's \"The operations\" section: $page" >&2
    exit 2
fi

failed=0

echo "$schema: $(echo "$in_schema" | wc -l) operation(s)"
echo "$page: $(echo "$on_page" | wc -l) operation(s) enumerated"

while read -r name; do
    if ! grep -q -F "\`$name\`" "$page"; then
        printf '  %-30s FAIL  in the schema, and the page never names it\n' "$name"
        failed=$((failed + 1))
    fi
done <<< "$in_schema"

while read -r name; do
    if ! grep -q -x -F "$name" <<< "$in_schema"; then
        printf '  %-30s FAIL  named on the page, and not in the schema\n' "$name"
        failed=$((failed + 1))
    fi
done <<< "$on_page"

if [ "$failed" -gt 0 ]; then
    echo "$failed failed."
    exit 1
fi

echo "every operation is documented and every documented operation exists."
