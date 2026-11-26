//! Bytes to a request, and a set of answer parameters to bytes.
//!
//! Nothing in this module reads the store, opens a socket or asks the
//! time. That is what makes every question about a malformed packet
//! answerable in a unit test, and it is why the whole of the
//! malformed corpus lives beside the code it exercises rather than in
//! a lab.
//!
//! # Why a guard runs before the decoder
//!
//! One process serves every network, so a malformed datagram that
//! panics is a remote denial of service against all of them at once
//! -- not local untidiness. The decoder this module calls has a
//! defect that is exactly that: both of its `chaddr()` accessors size
//! a slice by the packet's `hlen` byte, which nothing validates,
//! while `chaddr` is a fixed sixteen octets. Above sixteen the owned
//! accessor panics and the borrowed one returns bytes from `sname`,
//! `file` and the options area as the client's hardware address. The
//! fix is merged upstream and is in no release this project can
//! resolve; `upstream/dhcproto/` holds the staged patch and a
//! reproducer for each accessor.
//!
//! So [`parse`] refuses `hlen > 16` **before any accessor is
//! called**, which it can do without parsing anything at all: `hlen`
//! is the third byte of a BOOTP message, after `op` and `htype`. The
//! refusal is a named error rather than a clamp. Clamping would
//! invent a hardware address for a packet that does not have a valid
//! one, and this front-end identifies a client by that address.
//!
//! The same argument runs through the rest of the module. Nothing
//! here indexes a slice, unwraps, or does arithmetic that could
//! overflow, on anything derived from a datagram.
//!
//! # Decoded with a dependency, encoded without one
//!
//! Decoding is where the risk is, and it is a large amount of code to
//! write twice: the options area is a run of length-prefixed records
//! with concatenation rules of its own. So that direction uses
//! `dhcproto`, behind the guards above.
//!
//! Encoding does not, and the reason is the order of the options. The
//! option set an answer carries was measured rather than chosen, and
//! so was the order it was measured in; `dhcproto` holds a message's
//! options in a map keyed by option code and emits them in code
//! order, which is a different sequence. Building the reply here is
//! some forty lines of pushing fields onto a vector in the order RFC
//! 2131 lists them, with no arithmetic and no indexing, and it leaves
//! the bytes this project puts on the wire decided in this project.
//!
//! # Two shapes, for two reasons
//!
//! [`Request`] has public fields: it is a record of what arrived, and
//! each field's documentation says what [`parse`] established about
//! it before the caller saw it.
//!
//! [`Answer`] does not. Which fields an answer may carry depends on
//! which answer it is -- a refusal has no address and no lease time,
//! an answer to an INFORM has the configuration and no lease -- so it
//! is built through a constructor per answer, each of which takes the
//! request it answers. That is also how the fields RFC 2131 requires
//! a reply to echo come to be echoed without a caller remembering to.

mod answer;
mod error;
mod request;

#[cfg(test)]
mod behaviour;
#[cfg(test)]
mod malformed;
#[cfg(test)]
mod test_support;

pub use answer::{Answer, AnswerKind, LeaseSeconds, ServerFacts};
pub use error::{Error, Result};
pub use request::{OptionNotUnderstood, Request, RequestKind, parse};

/// The most bytes of datagram this layer will look at.
///
/// A DHCP message is 576 octets by specification and the field that
/// could claim more is the one being guarded, so anything larger is
/// refused unparsed rather than parsed carefully. The value is the
/// link's payload and not the protocol's minimum, because a client
/// may legitimately send a long options area and a server that
/// refused at 576 would be refusing on a number nothing measured.
pub const MAX_DATAGRAM_BYTES: usize = 1500;

/// The BOOTP header and the magic cookie, which every DHCP message
/// has whatever options follow.
///
/// 236 octets of fixed fields and then the four-octet cookie. A
/// datagram shorter than this is refused before anything reads a
/// field out of it.
const FIXED_BYTES: usize = 240;

/// Where `hlen` sits: the third byte, after `op` and `htype`.
///
/// Written as a constant because the guard that reads it is the one
/// piece of this module that must not depend on a parse having
/// happened first.
const HLEN_OFFSET: usize = 2;

/// The octets `chaddr` occupies, whatever `hlen` claims.
const CHADDR_BYTES: usize = 16;

/// Where the magic cookie sits, immediately after `file`.
const COOKIE_OFFSET: usize = 236;

/// `sname`, the server host name field, which a reply from here
/// leaves empty.
const SNAME_BYTES: usize = 64;

/// `file`, the boot file name field, which a reply from here leaves
/// empty.
const FILE_BYTES: usize = 128;

/// The four octets that say the options area is DHCP's rather than
/// vendor-specific BOOTP's: 99, 130, 83, 99.
const MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];

/// `op`, for a message from a client.
const BOOT_REQUEST: u8 = 1;

/// `op`, for a message from a server.
const BOOT_REPLY: u8 = 2;

/// The option code that pads the options area, carrying no length and
/// no value.
const OPTION_PAD: u8 = 0;

/// The option code that ends the options area, carrying no length and
/// no value.
const OPTION_END: u8 = 255;
