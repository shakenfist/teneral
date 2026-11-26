//! Why this layer would not turn a datagram into a request, or a set
//! of parameters into an answer.
//!
//! Every variant is a refusal with a cause, and the causes are
//! separate variants rather than one malformed-packet error carrying
//! a sentence. Two things need that. A log line that says which of
//! these happened is the difference between "a client on this bridge
//! is sending something teneral will not read" and an operator
//! reading hex; and the corpus of malformed inputs asserts a *named*
//! refusal per input, so a guard that was removed and replaced by a
//! later failure somewhere else does not look like a pass.
//!
//! None of these is a failure of this process. A refusal here means
//! somebody put bytes on a link that no server should answer, which
//! is the ordinary condition of a network and not an incident.
//!
//! Not everything wrong with a datagram is in here, and the line is
//! deliberate: an option whose framing is sound and whose value the
//! decoder declines is recorded on the request and costs that option,
//! not the answer. Only a fault that leaves the message unanswerable
//! -- or unlocatable, which a broken option record does -- is a
//! variant below.

use std::fmt;

/// The result of an operation in this module.
pub type Result<T> = std::result::Result<T, Error>;

/// Why a datagram was not read, or an answer not built.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The datagram was longer than this layer will look at.
    ///
    /// Refused before it is parsed rather than after, which is the
    /// point of the variant: the work of parsing is the cost an
    /// attacker is trying to impose.
    TooLarge {
        /// How many bytes arrived.
        length: usize,
    },

    /// The datagram was shorter than the fixed BOOTP header and the
    /// magic cookie, so at least one field it must have is not there.
    ///
    /// This is the refusal every truncation lands on, including a
    /// datagram of no bytes at all.
    TooShort {
        /// How many bytes arrived.
        length: usize,
    },

    /// `hlen` claimed more octets than `chaddr` holds.
    ///
    /// The guard this crate exists around. It is checked before any
    /// accessor is called, because the decoder's `chaddr()` sizes a
    /// slice by this byte: above sixteen one accessor panics and the
    /// other returns neighbouring bytes as a hardware address the
    /// client chose. A clamp was rejected -- it would invent a
    /// hardware address for a packet that does not have a valid one,
    /// and this front-end identifies a client by that address.
    HardwareLengthOverflowsField {
        /// What the packet claimed.
        hlen: u8,
    },

    /// `hlen` was zero, so the packet presents no hardware address.
    ///
    /// Refused rather than read as an empty address. A client with no
    /// hardware address is identifiable only by a client identifier,
    /// which is a case nothing here can answer today, and an empty
    /// address would otherwise become a selector that matched every
    /// such client in the network.
    NoHardwareLength,

    /// `chaddr` was present and entirely zero.
    ///
    /// Distinct from the length being zero, because the cause is
    /// different: this is a client that filled the field in with
    /// nothing rather than one that declared it empty. Either way it
    /// is not an address a lease can be keyed by.
    HardwareAddressUnset,

    /// The four octets where the magic cookie belongs were something
    /// else, so the options area is not DHCP's.
    ///
    /// Checked here because the decoder does not check it: a
    /// vendor-specific BOOTP message decodes without complaint and
    /// its options area is read as though it were DHCP's.
    NotTheMagicCookie {
        /// What was there instead.
        found: [u8; 4],
    },

    /// `op` was not `BOOTREQUEST`, so this is not a message from a
    /// client.
    ///
    /// A server's own reply, looped back or reflected by a bridge, is
    /// the ordinary cause.
    NotABootRequest {
        /// What `op` said.
        opcode: u8,
    },

    /// An option declared a length that ran past the end of the
    /// datagram.
    ///
    /// Refused rather than read as far as the bytes go, and refused
    /// as a whole datagram rather than as one option. A record whose
    /// length overruns means the boundaries of every record after it
    /// are unknown, so there is nothing to drop and carry on from.
    /// That is what separates this from an option whose value merely
    /// would not decode, which is recorded on the request and costs
    /// the option alone.
    OptionRunsPastTheEnd {
        /// The option that overran.
        code: u8,
        /// The length it declared.
        declared: usize,
        /// How many bytes were left after its length byte.
        remaining: usize,
    },

    /// An option had a code and then the datagram ended, so its
    /// length byte is missing.
    ///
    /// A framing error like the one above, and refused for the same
    /// reason: without the length there is no record to step over.
    OptionLengthMissing {
        /// The option that was cut off.
        code: u8,
    },

    /// More options' values were declined than this layer will drop.
    ///
    /// Dropping one is cheap and reading the rest of the datagram is
    /// what a client with an unreadable host name deserves. Dropping
    /// an unbounded number is not: each one costs a decode of the
    /// whole datagram, so without a limit the work one packet buys is
    /// chosen by its sender, and it is spent on the one thread
    /// serving every network in this process. The limit makes that
    /// cost a constant, and a sender past it is not a client whose
    /// lease is being protected.
    TooManyUnreadableOptions {
        /// How many may be dropped before this.
        limit: usize,
    },

    /// There was no message type option, so the message is BOOTP's
    /// rather than DHCP's.
    ///
    /// RFC 2131 requires the option in every DHCP message. A BOOTP
    /// client is not something this front-end serves, and answering
    /// one as though its request were a DISCOVER would hand out a
    /// lease to a client that cannot renew it.
    NoMessageType,

    /// The message type was one only a server sends, or one this
    /// front-end does not serve.
    ///
    /// An OFFER or an ACK arriving on the server's port is either
    /// another server on the link or a reflection, and in both cases
    /// the right answer is silence with a name on it.
    NotAClientMessage {
        /// The message type that arrived.
        message_type: u8,
    },

    /// The decoder refused the datagram.
    ///
    /// Carried as rendered text rather than as the decoder's own
    /// error type, so that the dependency with the defect this module
    /// guards stays inside this module. A caller that had to match on
    /// its error type would be a caller a version bump could break.
    Malformed {
        /// What the decoder said.
        detail: String,
    },

    /// A prefix length longer than an address has bits.
    ///
    /// Read from the interface the request arrived on rather than
    /// from the packet, so this is a configuration fault or a
    /// programming one and not an attack. It is still a refusal: a
    /// mask computed from it would be wrong rather than absent, and a
    /// client would carry it for the life of its lease.
    PrefixTooLong {
        /// What was asked for.
        prefix_length: u8,
    },

    /// A lease time no lease time option can carry.
    ///
    /// The store holds the configured value as a signed 64-bit count
    /// of seconds and the option is an unsigned 32-bit one, so the
    /// conversion is a real narrowing and belongs here, where the
    /// width of the field is known. Zero and below are refused for
    /// the same reason: a lease of no seconds is not a shorter lease,
    /// it is an answer no client can use.
    LeaseTimeNotRepresentable {
        /// The configured value.
        seconds: i64,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::TooLarge { length } => write!(
                formatter,
                "a datagram of {length} bytes is larger than this layer will parse"
            ),
            Error::TooShort { length } => write!(
                formatter,
                "a datagram of {length} bytes is shorter than a BOOTP header and its cookie"
            ),
            Error::HardwareLengthOverflowsField { hlen } => write!(
                formatter,
                "hlen claims {hlen} octets of hardware address and the field holds 16"
            ),
            Error::NoHardwareLength => {
                write!(
                    formatter,
                    "hlen is zero, so no hardware address is presented"
                )
            }
            Error::HardwareAddressUnset => {
                write!(
                    formatter,
                    "chaddr is entirely zero, which is not an address"
                )
            }
            Error::NotTheMagicCookie { found } => write!(
                formatter,
                "the options area begins {found:?} rather than with the DHCP magic cookie"
            ),
            Error::NotABootRequest { opcode } => {
                write!(
                    formatter,
                    "op is {opcode}, which is not a request from a client"
                )
            }
            Error::OptionRunsPastTheEnd {
                code,
                declared,
                remaining,
            } => write!(
                formatter,
                "option {code} declares {declared} bytes and {remaining} are left in the datagram"
            ),
            Error::OptionLengthMissing { code } => {
                write!(formatter, "option {code} has no length byte")
            }
            Error::TooManyUnreadableOptions { limit } => write!(
                formatter,
                "more than {limit} options were framed with a value that would not decode"
            ),
            Error::NoMessageType => write!(
                formatter,
                "there is no message type option, so this is a BOOTP message and not a DHCP one"
            ),
            Error::NotAClientMessage { message_type } => write!(
                formatter,
                "message type {message_type} is not one a client sends to a server"
            ),
            Error::Malformed { detail } => {
                write!(formatter, "the datagram would not decode: {detail}")
            }
            Error::PrefixTooLong { prefix_length } => write!(
                formatter,
                "a prefix of {prefix_length} bits is longer than an IPv4 address"
            ),
            Error::LeaseTimeNotRepresentable { seconds } => write!(
                formatter,
                "a lease of {seconds} seconds is not a duration the lease time option can carry"
            ),
        }
    }
}

impl std::error::Error for Error {}
