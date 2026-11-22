//! The differential rig: teneral's bytes beside a real dnsmasq's,
//! case by case, from a corpus of exchanges captured in a lab.
//!
//! Nothing here runs dnsmasq, and nothing here may. A behaviour no
//! RFC specifies is learned by watching dnsmasq run and recording
//! what it did, never by reading its source, and the recording is
//! made in a lab rather than in a test container -- so the corpus is
//! captured bytes, and the rig compares against those bytes.
//!
//! This is a frame rather than a working comparison. The repository
//! holds no DHCP or DNS code yet, so there is nothing of teneral's to
//! put beside a captured reply, and every case is `pending`. What the
//! rig does check today is the link each case claims: that it names a
//! row of `docs/provenance.md`, that the row exists, and that its
//! fixtures decode to the bytes it says were observed. A case that
//! names no row fails, because such a case asserts a behaviour on
//! nobody's authority.
//!
//! `make test` runs this. See `tests/differential/README.md` and
//! `docs/differential-rig.md`.

mod corpus;
mod fixture;
mod provenance;
mod subject;

use corpus::{Case, Entry, Status};
use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;

/// The provenance record, named the way a reader of a failure should
/// see it: a path from the root of the repository.
const RECORD: &str = "docs/provenance.md";

fn main() -> ExitCode {
    let rows = match provenance::rows() {
        Ok(rows) => rows,
        Err(why) => {
            eprintln!("differential rig: {why}");
            return ExitCode::FAILURE;
        }
    };
    let entries = match corpus::load() {
        Ok(entries) => entries,
        Err(why) => {
            eprintln!("differential rig: {why}");
            return ExitCode::FAILURE;
        }
    };

    println!(
        "differential rig: {} case(s) in {}, {} row(s) in {}, {} subject(s) registered",
        entries.len(),
        corpus::DIR_FROM_ROOT,
        rows.len(),
        RECORD,
        subject::SUBJECTS.len(),
    );

    let mut failures: Vec<String> = Vec::new();
    if entries.is_empty() {
        failures.push(format!(
            "{} holds no cases. An empty corpus makes this rig pass without checking anything, \
             which is the one way it can fail silently.",
            corpus::DIR_FROM_ROOT
        ));
    }
    for entry in &entries {
        check(entry, &rows, &mut failures);
    }

    println!();
    if failures.is_empty() {
        println!("{} case(s) checked, none failed.", entries.len());
        return ExitCode::SUCCESS;
    }

    eprintln!("differential rig: {} problem(s).\n", failures.len());
    for failure in &failures {
        eprintln!("  * {failure}\n");
    }
    eprintln!(
        "A case exists to tie bytes that were measured to the behaviour they are evidence of. \
         Fixing one of these means correcting the citation or the bytes, never deleting the \
         check: a case that cites nothing is worse than no case, because it reads as settled."
    );
    ExitCode::FAILURE
}

/// Everything the rig can check about one case.
fn check(entry: &Entry, rows: &BTreeSet<String>, failures: &mut Vec<String>) {
    println!("\n  {}", entry.name);

    let case = match &entry.case {
        Ok(case) => case,
        Err(why) => {
            println!("      metadata     FAILED");
            failures.push(format!("{}: {why}", entry.name));
            return;
        }
    };

    println!("      summary      {}", case.summary);
    println!(
        "      capture      observed {}, {} frames {}, from {}",
        case.capture.observed, case.capture.link_type, case.capture.frames, case.capture.pcap
    );
    println!("                   sha256 {}", case.capture.pcap_sha256);
    println!("                   server {}", case.capture.server);
    println!("                   client {}", case.capture.client);
    println!("                   write-up {}", case.capture.write_up);

    check_provenance(entry, case, rows, failures);
    let frames = check_fixtures(entry, case, failures);
    check_comparison(entry, case, frames, failures);
}

/// The check this rig exists for: every case names a row of the
/// provenance record, and every row it names is really there.
fn check_provenance(
    entry: &Entry,
    case: &Case,
    rows: &BTreeSet<String>,
    failures: &mut Vec<String>,
) {
    if case.provenance.is_empty() {
        println!("      provenance   FAILED: this case names no row");
        failures.push(format!(
            "{}: names no provenance row. `provenance` is a list of row ids from the `Row id` \
             column of {RECORD}, and it has to hold at least one. The row is what makes a \
             captured exchange evidence of a measured behaviour rather than an assertion about \
             dnsmasq, so a case that cites none has nothing behind it.",
            entry.name
        ));
        return;
    }

    let missing: Vec<&String> = case
        .provenance
        .iter()
        .filter(|row| !rows.contains(*row))
        .collect();
    if missing.is_empty() {
        println!("      provenance   ok: {}", case.provenance.join(", "));
        return;
    }

    println!(
        "      provenance   FAILED: {} of {} row(s) not in {RECORD}",
        missing.len(),
        case.provenance.len()
    );
    for row in missing {
        let hint = match provenance::closest(row, rows) {
            Some(near) => format!(" The closest row that does exist is `{near}`."),
            None => String::new(),
        };
        failures.push(format!(
            "{}: names provenance row `{row}`, which {RECORD} does not declare.{hint} Either add \
             that row, with the citation that makes it a measurement rather than a claim, or \
             correct the identifier to the row this case is evidence for.",
            entry.name
        ));
    }
}

/// The fixtures decode, and to the sizes the case says were observed.
fn check_fixtures(entry: &Entry, case: &Case, failures: &mut Vec<String>) -> Option<Frames> {
    let request = read_frame(
        &entry.dir,
        &case.request,
        case.capture.request_bytes,
        &entry.name,
        failures,
    );
    let response = read_frame(
        &entry.dir,
        &case.response,
        case.capture.response_bytes,
        &entry.name,
        failures,
    );
    match (request, response) {
        (Some(request), Some(response)) => {
            println!(
                "      fixtures     ok: {} {} bytes, {} {} bytes",
                case.request,
                request.len(),
                case.response,
                response.len()
            );
            Some(Frames { request, response })
        }
        _ => {
            println!("      fixtures     FAILED");
            None
        }
    }
}

/// One captured exchange's bytes.
struct Frames {
    request: Vec<u8>,
    response: Vec<u8>,
}

fn read_frame(
    dir: &Path,
    file: &str,
    expected: usize,
    case_name: &str,
    failures: &mut Vec<String>,
) -> Option<Vec<u8>> {
    let path = dir.join(file);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(why) => {
            failures.push(format!(
                "{case_name}: cannot read {}: {why}",
                path.display()
            ));
            return None;
        }
    };
    let bytes = match fixture::decode(&text) {
        Ok(bytes) => bytes,
        Err(why) => {
            failures.push(format!("{case_name}: {}: {why}", path.display()));
            return None;
        }
    };
    if bytes.len() != expected {
        failures.push(format!(
            "{case_name}: {} decodes to {} bytes, and the case records {expected} as observed. \
             One of the two has been edited: a fixture is a frame as it was captured, so the \
             bytes are not the thing to adjust to make this agree.",
            path.display(),
            bytes.len()
        ));
        return None;
    }
    Some(bytes)
}

/// The comparison itself, where there is anything to compare with.
fn check_comparison(
    entry: &Entry,
    case: &Case,
    frames: Option<Frames>,
    failures: &mut Vec<String>,
) {
    // A case has to say what it asserts, whether or not anything can
    // assert it yet. Until a subject exists this prose is the whole
    // specification of the comparison, and an empty one leaves a
    // captured exchange in the tree with nothing recorded about what
    // it is supposed to establish.
    if case.comparison.asserts.trim().is_empty() {
        println!("      comparison   FAILED: the case does not say what it asserts");
        failures.push(format!(
            "{}: `comparison.asserts` is empty. It is the case's specification -- what must \
             match the captured reply and what deliberately need not -- and while the status is \
             `pending` it is the only statement of it there is.",
            entry.name
        ));
    }

    let subject = subject::find(&case.comparison.subject);
    match (&case.comparison.status, subject) {
        (Status::Pending, None) => {
            println!(
                "      comparison   pending: no `{}` subject exists in this repository yet",
                case.comparison.subject
            );
        }
        (Status::Pending, Some(_)) => {
            println!("      comparison   FAILED: a subject is registered and the case is pending");
            failures.push(format!(
                "{}: a subject named `{}` is registered, but this case still reads \
                 `status = \"pending\"`, so its comparison does not run. Flip it to \
                 `\"compared\"` in the commit that registers the subject -- a case left pending \
                 beside working code is a comparison everyone believes is happening.",
                entry.name, case.comparison.subject
            ));
        }
        (Status::Compared, None) => {
            println!("      comparison   FAILED: no subject to compare against");
            failures.push(format!(
                "{}: reads `status = \"compared\"` but no subject named `{}` is registered in \
                 tests/differential/subject.rs, so there is nothing to compare the captured \
                 reply with. A case that compares nothing must not report a comparison.",
                entry.name, case.comparison.subject
            ));
        }
        (Status::Compared, Some(subject)) => {
            let Some(frames) = frames else {
                return;
            };
            match compare(subject, &frames) {
                Ok(()) => println!(
                    "      comparison   ok: `{}` matches the captured reply",
                    case.comparison.subject
                ),
                Err(why) => {
                    println!("      comparison   FAILED");
                    failures.push(format!("{}: {why}", entry.name));
                }
            }
        }
    }
}

/// Teneral's answer to the captured request, against the captured
/// reply.
fn compare(subject: &subject::Subject, frames: &Frames) -> Result<(), String> {
    let answer = (subject.answer)(&frames.request).map_err(|why| {
        format!(
            "`{}` could not answer the captured request: {why}",
            subject.name
        )
    })?;
    if answer == frames.response {
        return Ok(());
    }
    let at = answer
        .iter()
        .zip(frames.response.iter())
        .position(|(ours, theirs)| ours != theirs);
    let detail = match at {
        Some(offset) => format!(
            "they first differ at byte {offset}: the capture has {:#04x} and `{}` produced \
             {:#04x}",
            frames.response[offset], subject.name, answer[offset]
        ),
        None => "one is a prefix of the other".to_owned(),
    };
    Err(format!(
        "`{}` produced {} bytes and the captured reply is {} bytes; {detail}. The capture is \
         the measurement, so a difference is either a defect here or a deliberate divergence \
         -- and a deliberate one belongs in the case, asserted, rather than removed.",
        subject.name,
        answer.len(),
        frames.response.len()
    ))
}
