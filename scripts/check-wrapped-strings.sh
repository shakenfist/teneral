#!/bin/bash
#
# Catch a string literal that was wrapped across source lines without
# the backslash that joins them.
#
# Rust has no implicit concatenation, so a literal written as
#
#     "the first half of a sentence
#      and the second"
#
# is one string containing a newline and the indentation after it, and
# a literal somebody later joined onto one line to stop rustfmt
# complaining is one string containing that indentation as a run of
# spaces. Either way the message a person reads has a gap in the middle
# of it, and nothing else notices: rustfmt cannot reflow a string
# literal, clippy has no opinion about its contents, and a test that
# asserts a message `contains` a value passes over the gap.
#
# Two such literals reached a pull request in one change -- the detail
# of an API refusal and a line in the daemon's log, both of them
# user-visible text in a stream documented as one line per message --
# which is why this is a check rather than a habit.
#
# The shape is a run of spaces between two word characters, which is
# something no formatter produces and no deliberate sentence contains.
# Eight is the threshold because the wrapped indentation in this tree
# is at least sixteen and the widest deliberate alignment inside a
# literal is six. A line that aligns a table on purpose and trips this
# anyway says `aligned-on-purpose` in a comment on the same line.

set -euo pipefail

# The run of spaces, between characters that make it a sentence rather
# than an indent: a literal's own leading indentation is at the start
# of a line and so matches nothing here.
shape='[[:alnum:],;:.)] {8,}[[:alnum:]]'

files=("$@")
if [ ${#files[@]} -eq 0 ]; then
    mapfile -t files < <(git ls-files '*.rs')
fi

if [ ${#files[@]} -eq 0 ]; then
    exit 0
fi

found=$(grep -nHP "$shape" "${files[@]}" | grep -v 'aligned-on-purpose' || true)

if [ -n "$found" ]; then
    echo "A string literal looks wrapped without the backslash that joins it:" >&2
    echo "$found" >&2
    echo >&2
    echo "Each line above has a run of spaces in the middle of it, which is what" >&2
    echo "a reader of the message will see. End each wrapped line with a \\." >&2
    exit 1
fi
