//! The corpus: one directory per captured exchange.
//!
//! Each directory holds a `case.toml` describing the exchange and the
//! hex-text fixtures it names. Metadata on disk rather than in Rust
//! is what lets a capture run add a case by writing a directory, which
//! is how the cases for a front end will arrive: a lab deploy, a
//! capture, one directory per exchange.

use serde::Deserialize;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

/// The corpus path as a reader of a failure message should see it,
/// from the root of the repository.
pub const DIR_FROM_ROOT: &str = "teneral/tests/differential/corpus";

/// The file that makes a directory a case.
pub const METADATA: &str = "case.toml";

/// One captured exchange, as `case.toml` records it.
///
/// `deny_unknown_fields` so that a mistyped key is an error rather
/// than a field silently ignored -- including `provenence`, which
/// would otherwise leave a case citing nothing while looking as
/// though it cited something.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    /// What this exchange is, in a line.
    pub summary: String,
    /// The `docs/provenance.md` rows these bytes are evidence for, by
    /// row id. A case with none of them is an error.
    ///
    /// `default` so that a case which omits the key altogether is
    /// reported by the harness, which can say why a row is required
    /// and what to do about it, rather than by the deserializer,
    /// which can only say that a field is missing.
    #[serde(default)]
    pub provenance: Vec<String>,
    /// The captured request, as a fixture file beside `case.toml`.
    pub request: String,
    /// The captured reply, likewise -- absent where the capture
    /// recorded no reply at all.
    ///
    /// A silence is a measurement: four of the behaviours this corpus
    /// records are "the reference implementation did not answer
    /// this", and a case that could not say so would have to leave
    /// them out. The capture is what proves it: every frame that
    /// crossed the bridge for the seconds either side of the request
    /// is in the pcap the case names, and none of them is a reply.
    #[serde(default)]
    pub response: Option<String>,
    pub capture: Capture,
    pub network: Configuration,
    pub comparison: Comparison,
}

/// The configuration the capture was taken against.
///
/// Data rather than prose, because a DHCP reply is not a function of
/// the request alone: the lease time, the mask and three of the nine
/// options an answer carries come from the network and the interface,
/// so a rig that fed the subject nothing but bytes could only ever
/// compare a packet nobody had configured. The harness builds a store
/// from this and asks the subject to answer the captured request
/// against it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    /// The network the interface below belongs to.
    pub name: String,
    /// The interface the request arrived on, named within the
    /// namespace an interface in no namespace of its own is attached
    /// under. That namespace is not a field: there is one spelling of
    /// it and the front-end owns it.
    pub interface: String,
    /// The address that interface was observed to hold, which is the
    /// server identifier, the router and the nameserver.
    pub server_address: Ipv4Addr,
    /// The prefix beside it, in bits.
    pub prefix_length: u8,
    /// How long the network holds the addresses it gives out.
    pub lease_seconds: i64,
    /// The spans it may give addresses out of.
    pub ranges: Vec<Range>,
    /// The leases the server already held when the exchange was
    /// captured.
    ///
    /// Part of the configuration in the same sense as the ranges: a
    /// renewal, a release and a refusal are all answers to a client
    /// the server has a record of, so a case for one of them that
    /// started from an empty store would be asking a different
    /// question. Each lease here was granted by an earlier exchange in
    /// the same capture, which is what makes it a record of what was
    /// there rather than a convenient fiction.
    #[serde(default)]
    pub leases: Vec<Held>,
}

/// One span of addresses a network may give out.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Range {
    pub name: String,
    pub first: Ipv4Addr,
    pub last: Ipv4Addr,
}

/// One lease the server held when the exchange was captured.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Held {
    pub address: Ipv4Addr,
    /// The client holding it, by the hardware address an operator
    /// would type.
    pub mac: String,
}

/// Where the bytes came from. Every field here is provenance rather
/// than something the comparison needs, and the harness prints the
/// lot on every run: a corpus whose origin is recorded only in a
/// commit message is a corpus nobody can re-derive.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    /// The date the exchange was observed, `YYYY-MM-DD`.
    pub observed: String,
    /// The program that sent the reply, and what it was running on.
    pub server: String,
    /// The program that sent the request, likewise.
    pub client: String,
    /// The link layer the fixtures are frames of, as libpcap names it.
    pub link_type: String,
    /// The capture file the frames were taken from, and its digest.
    /// The file itself is deliberately not in this repository.
    pub pcap: String,
    pub pcap_sha256: String,
    /// Which frames of that capture these fixtures are.
    pub frames: String,
    /// The experiment write-up that holds the run and its output.
    pub write_up: String,
    /// How many bytes each fixture decodes to. Recorded so that an
    /// accidental edit to a fixture fails here rather than changing
    /// what the corpus claims was observed.
    pub request_bytes: usize,
    /// Absent exactly where `response` is.
    #[serde(default)]
    pub response_bytes: Option<usize>,
}

/// What the comparison asserts, and whether it can run yet.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Comparison {
    /// The subject that answers this exchange, by the name it
    /// registers under.
    pub subject: String,
    pub status: Status,
    /// Whether teneral answers this exchange at all.
    ///
    /// `answer` is the ordinary case. `silence` says teneral
    /// deliberately says nothing -- either because the capture holds
    /// no reply either, or because this is a request the reference
    /// implementation serves and this front-end refuses.
    #[serde(default)]
    pub expect: Expect,
    /// The fields of the reply that must match the capture, by name.
    pub fields: Vec<String>,
    /// The agreements within one reply that must hold, in the capture
    /// and in teneral's answer alike.
    ///
    /// For the things that cannot be compared with the capture because
    /// teneral legitimately chooses otherwise: which address comes out
    /// of a pool is its own choice, so `yiaddr` is asserted to agree
    /// with the address the reply was sent to rather than to be the
    /// address dnsmasq picked.
    #[serde(default)]
    pub agree: Vec<String>,
    /// The fields that must *differ*, because teneral differs on
    /// purpose.
    ///
    /// Asserted rather than left out: a divergence nobody wrote down
    /// is indistinguishable, a year later, from a case nobody wrote,
    /// and a divergence that quietly stops being one should fail here
    /// so that somebody decides whether the case or the code moved.
    #[serde(default)]
    pub diverges: Vec<String>,
    /// What the comparison asserts, in prose. The field lists above
    /// say what is checked; this says why, and what deliberately is
    /// not checked at all.
    pub asserts: String,
}

/// Whether teneral answers an exchange.
#[derive(Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Expect {
    /// teneral puts a reply on the wire, and it is compared.
    #[default]
    Answer,
    /// teneral puts nothing on the wire, which is the whole
    /// assertion.
    Silence,
}

/// Whether a case's comparison runs.
#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// Nothing in this repository can answer this exchange yet. The
    /// harness says so on every run rather than reporting a pass.
    Pending,
    /// A subject is registered and the comparison runs.
    Compared,
}

/// A directory in the corpus, loaded or not.
///
/// A directory that cannot be loaded is still an entry, because the
/// alternative is skipping it: a case whose metadata does not parse
/// would then be a case that checks nothing and says nothing.
pub struct Entry {
    pub name: String,
    pub dir: PathBuf,
    pub case: Result<Case, String>,
}

/// Every directory in the corpus, in name order.
pub fn load() -> Result<Vec<Entry>, String> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/differential/corpus");
    let listing =
        std::fs::read_dir(&dir).map_err(|why| format!("cannot read {}: {why}", dir.display()))?;

    let mut directories = Vec::new();
    for entry in listing {
        let entry = entry.map_err(|why| format!("cannot read {}: {why}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            directories.push(path);
        }
    }
    directories.sort();

    Ok(directories.into_iter().map(read_case).collect())
}

/// One case directory.
fn read_case(dir: PathBuf) -> Entry {
    let name = match dir.file_name().and_then(|name| name.to_str()) {
        Some(name) => name.to_owned(),
        // A path that came from read_dir has a final component, and
        // one that is not valid UTF-8 is a case nobody can name in a
        // provenance citation anyway.
        None => dir.display().to_string(),
    };
    let case = parse(&dir.join(METADATA));
    Entry { name, dir, case }
}

fn parse(path: &Path) -> Result<Case, String> {
    let text = std::fs::read_to_string(path).map_err(|why| {
        format!(
            "cannot read {}: {why}. Every directory in the corpus is a case and needs one: a \
             directory of bytes with no metadata names no provenance row, so there is nothing \
             to say what the bytes are evidence of.",
            path.display()
        )
    })?;
    toml::from_str(&text).map_err(|why| format!("{}: {why}", path.display()))
}
