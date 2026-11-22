//! Reading the row identifiers out of `docs/provenance.md`.
//!
//! The provenance record is a markdown table whose first column is a
//! row id in backticks. Recognising a data row by that prefix is a
//! crude way to read markdown, and it is the deliberate choice: the
//! alternative is a second, machine-readable copy of the same list,
//! and two copies of a list is exactly how the link between a case
//! and the behaviour it is evidence for goes quietly stale. The file
//! a human reads is the file the rig reads.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// Where the provenance record lives, relative to this crate.
const RELATIVE: &str = "../docs/provenance.md";

/// The provenance record's path, for a message a reader can act on.
pub fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(RELATIVE)
}

/// Every row id the provenance record declares.
pub fn rows() -> Result<BTreeSet<String>, String> {
    let path = path();
    let text = std::fs::read_to_string(&path)
        .map_err(|why| format!("cannot read {}: {why}", path.display()))?;

    let rows: BTreeSet<String> = text.lines().filter_map(row_id).map(str::to_owned).collect();
    if rows.is_empty() {
        return Err(format!(
            "{} declares no row ids. A data row reads \"| `some-row-id` | **Behaviour** ... |\"; \
             if the table has changed shape, this parser has to change with it rather than \
             silently find nothing -- a rig that reads an empty list of rows would accept any \
             citation at all.",
            path.display()
        ));
    }
    Ok(rows)
}

/// The row id of one line of the record, if that line is a data row.
fn row_id(line: &str) -> Option<&str> {
    let after_pipe = line.strip_prefix("| `")?;
    let (id, _) = after_pipe.split_once('`')?;
    if id.is_empty() { None } else { Some(id) }
}

/// The declared row id most like `wanted`, when one is close enough to
/// be worth suggesting. A mistyped identifier is the likeliest way a
/// case comes to name a row that does not exist, and the fix is much
/// easier to see next to the name that was meant.
pub fn closest<'rows>(wanted: &str, rows: &'rows BTreeSet<String>) -> Option<&'rows str> {
    let shared = |candidate: &str| {
        wanted
            .chars()
            .zip(candidate.chars())
            .take_while(|(a, b)| a == b)
            .count()
    };
    rows.iter()
        .map(|row| (shared(row), row.as_str()))
        .filter(|(prefix, _)| *prefix >= 8)
        .max_by_key(|(prefix, _)| *prefix)
        .map(|(_, row)| row)
}
