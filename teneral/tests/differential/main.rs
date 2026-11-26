//! The differential rig: teneral's bytes beside a real dnsmasq's,
//! case by case, from a corpus of exchanges captured in a lab.
//!
//! Nothing here runs dnsmasq, and nothing here may. A behaviour no
//! RFC specifies is learned by watching dnsmasq run and recording
//! what it did, never by reading its source, and the recording is
//! made in a lab rather than in a test container -- so the corpus is
//! captured bytes, and the rig compares against those bytes.
//!
//! Every case names a row of `docs/provenance.md`, that row has to
//! exist, and the fixtures have to decode to the bytes the case says
//! were observed. A case that names no row fails, because such a case
//! asserts a behaviour on nobody's authority.
//!
//! Where a subject is registered the case is compared as well, field
//! by field against the configuration it was captured under. That
//! comparison reports every difference rather than the first: one byte
//! offset says two frames differ, and a list of fields says what
//! teneral got wrong.
//!
//! `make test` runs this. See `tests/differential/README.md` and
//! `docs/differential-rig.md`.

mod compare;
mod corpus;
mod dhcpv4;
mod fixture;
mod frame;
mod provenance;
mod subject;

use corpus::{Case, Entry, Expect, Status};
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

    println!(
        "      network      {} on {} at {}/{}, lease {}s, {} range(s), {} lease(s) held",
        case.network.name,
        case.network.interface,
        case.network.server_address,
        case.network.prefix_length,
        case.network.lease_seconds,
        case.network.ranges.len(),
        case.network.leases.len(),
    );

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
///
/// A case with no `response` recorded no reply, which is a measurement
/// and not an omission: four of this corpus's behaviours are "the
/// reference implementation did not answer this". `response` and
/// `response_bytes` have to be absent together, since one without the
/// other is a case that has lost half of what it claims.
fn check_fixtures(entry: &Entry, case: &Case, failures: &mut Vec<String>) -> Option<Frames> {
    let request = read_frame(
        &entry.dir,
        &case.request,
        case.capture.request_bytes,
        &entry.name,
        failures,
    );

    let response = match (&case.response, case.capture.response_bytes) {
        (Some(file), Some(bytes)) => {
            match read_frame(&entry.dir, file, bytes, &entry.name, failures) {
                Some(frame) => Recorded::Reply(frame),
                None => Recorded::Unreadable,
            }
        }
        (None, None) => Recorded::Silence,
        (Some(_), None) | (None, Some(_)) => {
            failures.push(format!(
                "{}: `response` and `capture.response_bytes` have to be present together or \
                 absent together. Absent means the capture recorded no reply, which is a \
                 measurement; one without the other is a case that has mislaid half of what it \
                 says was seen.",
                entry.name
            ));
            Recorded::Unreadable
        }
    };

    match (request, response) {
        (Some(request), Recorded::Reply(response)) => {
            println!(
                "      fixtures     ok: {} {} bytes, {} {} bytes",
                case.request,
                request.len(),
                case.response.as_deref().unwrap_or("-"),
                response.len()
            );
            Some(Frames {
                request,
                response: Some(response),
            })
        }
        (Some(request), Recorded::Silence) => {
            println!(
                "      fixtures     ok: {} {} bytes, and no reply was captured",
                case.request,
                request.len()
            );
            Some(Frames {
                request,
                response: None,
            })
        }
        _ => {
            println!("      fixtures     FAILED");
            None
        }
    }
}

/// What the capture holds where a reply would be.
enum Recorded {
    /// A reply, which decoded.
    Reply(Vec<u8>),
    /// No reply, which is the measurement.
    Silence,
    /// A reply that did not decode, already reported.
    Unreadable,
}

/// One captured exchange's bytes.
struct Frames {
    request: Vec<u8>,
    /// Absent where the capture recorded no reply.
    response: Option<Vec<u8>>,
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

    // A case that names no field assertions fails like a case that
    // names no provenance row, and for the same reason: it reads as
    // settled and establishes nothing. The exception is a case whose
    // whole assertion is that teneral answers nothing, where there are
    // no fields to name.
    let named =
        case.comparison.fields.len() + case.comparison.agree.len() + case.comparison.diverges.len();
    if named == 0 && case.comparison.expect == Expect::Answer {
        println!("      comparison   FAILED: the case names no field assertion");
        failures.push(format!(
            "{}: `comparison` names no field in `fields`, `agree` or `diverges`, so its \
             comparison checks nothing while reporting that it ran. Name the fields that must \
             match the capture, the agreements that must hold within a reply, and the fields \
             teneral differs on deliberately.",
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
            let answered = match (subject.answer)(&case.network, &frames.request) {
                Ok(answered) => answered,
                Err(why) => {
                    println!("      comparison   FAILED");
                    failures.push(format!(
                        "{}: `{}` could not answer the captured request: {why}",
                        entry.name, case.comparison.subject
                    ));
                    return;
                }
            };
            let problems = compare::compare(
                &case.comparison,
                &frames.request,
                frames.response.as_deref(),
                &answered,
            );
            if problems.is_empty() {
                if case.comparison.expect == Expect::Silence {
                    println!(
                        "      comparison   ok: teneral answered nothing, as the case asserts"
                    );
                    return;
                }
                println!(
                    "      comparison   ok: {} field(s), {} agreement(s), {} declared \
                     divergence(s)",
                    case.comparison.fields.len(),
                    case.comparison.agree.len(),
                    case.comparison.diverges.len(),
                );
                return;
            }
            println!("      comparison   FAILED: {} problem(s)", problems.len());
            for problem in problems {
                failures.push(format!("{}: {problem}", entry.name));
            }
        }
    }
}
