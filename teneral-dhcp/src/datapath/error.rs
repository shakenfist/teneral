//! Why the sockets could not do what they were asked.
//!
//! Three kinds of thing go wrong down here and they are worth keeping
//! apart, because an operator can act on two of them and nobody can
//! act on the third.
//!
//! A system call that fails carries its errno, and most of what an
//! operator needs is in the pair of the call's name and that errno:
//! `socket(AF_PACKET)` with `EPERM` says the process is missing a
//! capability, `bind` with `EACCES` says the port is privileged and
//! the process may not have it, and `bind` with `EADDRINUSE` says
//! something else is already serving. Naming the call is what turns a
//! bare errno into a diagnosis.
//!
//! A configuration fault is an interface that cannot be answered from:
//! one with no address, or with several, or whose netmask is not a
//! prefix. Each is a refusal rather than a guess, because a server
//! that picked one of two addresses to call itself would answer
//! differently depending on the order the kernel happened to list
//! them, and the one thing worse than not answering is answering
//! inconsistently. Every one of these names the interface, which is
//! the whole of what makes it fixable.
//!
//! And a reply that will not fit a frame, or a client whose hardware
//! address is not an Ethernet one, is this layer refusing to invent
//! something. Neither can be reached from the wire -- the layers above
//! refuse such a client before an answer is built -- so each is a
//! backstop rather than a live path, and a backstop that reports is
//! worth more than one that truncates.

use std::fmt;
use std::io;
use std::net::Ipv4Addr;

/// The result of asking the sockets to do something.
pub type Result<T> = std::result::Result<T, Error>;

/// Why a socket, an interface query or a frame did not work.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// A system call failed, and this is which one and what it said.
    Syscall {
        /// The call, as it is spelled in the manual page, so that a
        /// line in a log can be looked up.
        call: &'static str,
        /// What the kernel said.
        source: io::Error,
    },

    /// An interface name does not fit the field the kernel takes it
    /// in.
    ///
    /// A name this long cannot name a real interface, so it is a typo
    /// or a value from somewhere it should not have come from.
    NameTooLong {
        /// The name as it was given.
        ifname: String,
        /// The most octets the kernel's field holds, terminator
        /// included.
        limit: usize,
    },

    /// An interface name is empty, or holds a zero octet.
    ///
    /// The kernel reads such a field up to its first zero, so a name
    /// like this would quietly ask about a different interface than
    /// the one given -- which on this datapath means answering from
    /// the wrong network.
    NameNotUsable {
        /// The name as it was given.
        ifname: String,
    },

    /// An interface index does not fit the width the kernel takes it
    /// in.
    ///
    /// Refused rather than reinterpreted. The kernel does not produce
    /// one, and an index that had been wrapped into range would be a
    /// different interface's.
    IndexOutOfRange {
        /// The index, as whichever side of the conversion had it.
        index: i64,
    },

    /// A socket option's value is longer than the kernel's own length
    /// field.
    ///
    /// Unreachable from any value this datapath sets, each of which is
    /// a flag or an interface name that was checked for length before
    /// it got here.
    OptionTooLong {
        /// The option, in the spelling its manual page uses.
        call: &'static str,
        /// The length that would not fit.
        length: usize,
    },

    /// The interface's hardware address is not an Ethernet one.
    ///
    /// The reply to a client with no address is a hand-built Ethernet
    /// frame, so an interface of another kind cannot be served one.
    /// Refused by name rather than by truncating its address into six
    /// octets, which would put a frame on the wire from an address the
    /// interface does not have.
    NotEthernetInterface {
        /// The interface.
        ifname: String,
        /// The kind of hardware the kernel reported.
        hardware_type: u16,
    },

    /// The kernel named an interface in bytes that are not text.
    ///
    /// An interface name is bytes to the kernel and a string
    /// everywhere above here, the store's attachment key included, so
    /// a name that is not text is one no attachment could have been
    /// written for.
    NameNotText {
        /// The index the name was asked for.
        index: u32,
    },

    /// The interface holds no IPv4 address, so there is nothing to
    /// answer from.
    ///
    /// Every option but the lease time is derived from the address and
    /// prefix the interface holds, so an interface without one can be
    /// served no answer at all.
    NoAddress {
        /// The interface, which is what an operator configures.
        ifname: String,
    },

    /// The interface holds more than one IPv4 address.
    ///
    /// Refused rather than resolved by taking the first: the server
    /// identifier, the router and the nameserver are all this address,
    /// so choosing between two would make every answer depend on an
    /// order nothing promises.
    SeveralAddresses {
        /// The interface.
        ifname: String,
        /// How many addresses it holds.
        count: usize,
    },

    /// The interface's netmask is not a run of ones followed by a run
    /// of zeroes.
    ///
    /// A mask from the kernel is a prefix, so this is unreachable in
    /// practice and is refused rather than rounded for the same reason
    /// as the two above: a prefix computed from a mask that is not one
    /// would be wrong rather than absent, and a client would carry it
    /// for the life of its lease.
    NetmaskNotAPrefix {
        /// The interface.
        ifname: String,
        /// The mask as the kernel reported it.
        netmask: Ipv4Addr,
    },

    /// An answer could not be described from what the interface holds.
    ///
    /// The one cause is a prefix longer than an address has bits,
    /// which the kernel does not produce.
    Facts {
        /// The interface the address and prefix came from.
        ifname: String,
        /// What refused them.
        source: crate::wire::Error,
    },

    /// The client's hardware address is not an Ethernet address, so no
    /// frame can be addressed to it.
    ///
    /// A backstop. A client whose hardware address is not six octets
    /// presents nothing the store will hold as a selector value, so it
    /// is never answered and this is never reached from the wire.
    NotEthernet {
        /// How many octets the client declared.
        length: usize,
    },

    /// A reply is longer than a frame's length fields can describe.
    ///
    /// Also a backstop: the longest answer this front-end builds is a
    /// few hundred octets, and the encoder's own bound is far below
    /// what an IPv4 total length can carry.
    ReplyTooLong {
        /// The length that would not fit.
        length: usize,
    },

    /// A reply arrived at the sender for an interface it does not
    /// serve.
    ///
    /// One sender serves one interface, so this would be an answer
    /// derived from one network leaving by another network's
    /// interface. It cannot happen through the layer above, which
    /// builds a reply from the arrival it answered, and it is checked
    /// anyway because that is the project's central correctness
    /// property and a check costs two comparisons.
    WrongInterface {
        /// The interface this sender serves.
        serving: String,
        /// The interface the reply named.
        named: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Syscall { call, source } => write!(formatter, "{call}: {source}"),
            Error::NameTooLong { ifname, limit } => write!(
                formatter,
                "the interface name {ifname} is longer than the {limit} octets the kernel takes"
            ),
            Error::NameNotUsable { ifname } => write!(
                formatter,
                "the interface name {ifname:?} is empty or holds a zero octet, so the kernel would \
                 read a shorter name than the one given"
            ),
            Error::IndexOutOfRange { index } => write!(
                formatter,
                "the interface index {index} is not one the kernel can be asked about"
            ),
            Error::OptionTooLong { call, length } => write!(
                formatter,
                "a value of {length} octets is too long for {call}"
            ),
            Error::NotEthernetInterface {
                ifname,
                hardware_type,
            } => write!(
                formatter,
                "the interface {ifname} is of hardware type {hardware_type} rather than Ethernet, \
                 and a reply to a client with no address is an Ethernet frame"
            ),
            Error::NameNotText { index } => write!(
                formatter,
                "the name of interface {index} is not text, so no attachment could name it"
            ),
            Error::NoAddress { ifname } => write!(
                formatter,
                "the interface {ifname} holds no IPv4 address, and every option but the lease \
                 time is derived from one"
            ),
            Error::SeveralAddresses { ifname, count } => write!(
                formatter,
                "the interface {ifname} holds {count} IPv4 addresses, and a server that chose one \
                 of them would answer differently from one run to the next"
            ),
            Error::NetmaskNotAPrefix { ifname, netmask } => write!(
                formatter,
                "the netmask {netmask} on the interface {ifname} is not a prefix"
            ),
            Error::Facts { ifname, source } => write!(
                formatter,
                "the interface {ifname} cannot be answered from: {source}"
            ),
            Error::NotEthernet { length } => write!(
                formatter,
                "a client's hardware address is {length} octets, so no Ethernet frame can be \
                 addressed to it"
            ),
            Error::ReplyTooLong { length } => {
                write!(formatter, "a reply of {length} octets will not fit a frame")
            }
            Error::WrongInterface { serving, named } => write!(
                formatter,
                "a reply for {named} reached the sender for {serving}"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Syscall { source, .. } => Some(source),
            Error::Facts { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<Error> for io::Error {
    /// The send seam reports an operating system error, because
    /// sending bytes on an interface is a system call and every
    /// implementation of it will have one. The errno is kept where
    /// there is one, so that a caller matching on
    /// [`io::ErrorKind`] still sees what the kernel said.
    fn from(error: Error) -> Self {
        match error {
            Error::Syscall { call, source } => {
                io::Error::new(source.kind(), format!("{call}: {source}"))
            }
            other => io::Error::other(other.to_string()),
        }
    }
}

/// The error for a failed call, with the errno the kernel left.
pub(crate) fn syscall(call: &'static str) -> Error {
    Error::Syscall {
        call,
        source: io::Error::last_os_error(),
    }
}
