//! The two sockets a served interface needs.
//!
//! # Why the datagram socket binds the wildcard
//!
//! A UDP socket bound to an interface's own address never receives a
//! datagram addressed to `255.255.255.255`, and a client with no
//! address has nothing else to send one to. So the socket binds the
//! wildcard address, and the interface it serves is named to the
//! kernel instead -- which is what keeps one socket per interface from
//! being one socket for every interface.
//!
//! `SO_BINDTODEVICE` is how that is said. It does two things at once:
//! the socket receives only the named interface's traffic, which makes
//! the separation between two overlapping networks the kernel's to
//! enforce rather than a filter this code could get wrong; and it is
//! what lets a second socket bind the same port, two sockets on one
//! port being a conflict only when they name the same interface or no
//! interface at all. On the kernel this was run against it needs no
//! privilege at all -- it was once a capability away -- and the daemon
//! does not depend on that either way: it already needs the capability
//! for the frame below, so the worst a kernel that asks for it again
//! costs is an earlier refusal with the call's name on it.
//!
//! # Why the egress is named per message as well
//!
//! A limited-broadcast reply from an interface with no default route
//! cannot be sent without naming the interface it leaves by: the
//! kernel has no route to `255.255.255.255` and refuses. Naming the
//! egress in the message rather than relying on the socket's device
//! needs no privilege, says in one place which interface an answer
//! left by, and is the same mechanism whether or not a socket is bound
//! to a device -- so it is what the send path uses.
//!
//! # Why there is a second socket at all
//!
//! A reply to a client with no address is addressed to the address it
//! is being offered, which ordinary IP routing cannot deliver: there
//! is no route to it and no address resolution that could find it.
//! Below the IP layer there is -- a frame addressed to the hardware
//! address the client does hold, carrying an IP packet addressed to
//! the address it does not. That takes a packet socket, the one thing
//! here that needs privilege the kernel will not grant by any other
//! means.

use std::io;
use std::mem;
use std::net::Ipv4Addr;
use std::ops::Deref;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::ptr;
use std::sync::Arc;

use crate::datapath::ETHERNET_PROTOCOL_IPV4;
use crate::datapath::error::{Error, Result, syscall};
use crate::datapath::interface::ETHERNET_ADDRESS_BYTES;

/// The address families this file names, as the family member of a
/// socket address holds them. Constants rather than conversions at the
/// call site: each is a small positive number written in a header, so
/// a conversion would be an error path that cannot be taken.
const INET_FAMILY: libc::sa_family_t = libc::AF_INET as libc::sa_family_t;

/// The family a link-layer address declares.
const PACKET_FAMILY: libc::sa_family_t = libc::AF_PACKET as libc::sa_family_t;

/// The length of an IPv4 socket address, as the kernel is told it.
const INET_ADDRESS_BYTES: libc::socklen_t = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;

/// The length of a link-layer socket address, as the kernel is told
/// it.
const LINK_ADDRESS_BYTES: libc::socklen_t = mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t;

/// Room for one control message carrying one `in_pktinfo`.
const CONTROL_BYTES: usize =
    // SAFETY: `CMSG_SPACE` is arithmetic on a length and reads
    // nothing; it is unsafe only because the C macro it mirrors is.
    unsafe { libc::CMSG_SPACE(mem::size_of::<libc::in_pktinfo>() as u32) } as usize;

/// A control buffer must be aligned for the header the kernel writes
/// into it, and this file aligns its buffer to eight octets. The
/// assertion is here so that a platform where that is not enough fails
/// to build rather than reading a misaligned header at run time.
const _: () = assert!(mem::align_of::<libc::cmsghdr>() <= 8);

/// A control buffer, aligned for the header the kernel puts in it.
#[repr(align(8))]
struct Control([u8; CONTROL_BYTES]);

impl Control {
    /// An empty control buffer.
    fn new() -> Self {
        Control([0; CONTROL_BYTES])
    }
}

/// What the kernel reported about a datagram as it handed it over.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Receipt {
    /// How many octets of the datagram were written into the buffer.
    pub(crate) length: usize,
    /// The index of the interface it arrived on.
    pub(crate) ifindex: u32,
    /// The address it was sent to, which is `255.255.255.255` for a
    /// client that has no address to be answered at and the
    /// interface's own address for one that has.
    pub(crate) destination: Ipv4Addr,
}

/// A datagram socket serving one interface.
///
/// Opened, told which interface it serves, and then bound, in that
/// order, because `SO_BINDTODEVICE` after a bind does not move a
/// socket that is already in the port's table.
#[derive(Debug)]
pub(crate) struct UdpSocket {
    /// The descriptor, closed when this value is dropped.
    fd: OwnedFd,
}

impl UdpSocket {
    /// Open the socket, asking the kernel to report where each
    /// datagram arrived and allowing a broadcast reply.
    ///
    /// Non-blocking, because the receive side is driven by the
    /// runtime's readiness rather than by a thread parked in a system
    /// call.
    pub(crate) fn open() -> Result<Self> {
        // SAFETY: `socket` takes three integers and returns a
        // descriptor or -1. Nothing is borrowed across the call.
        let fd = unsafe {
            libc::socket(
                libc::AF_INET,
                libc::SOCK_DGRAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                libc::IPPROTO_UDP,
            )
        };
        if fd < 0 {
            return Err(syscall("socket(AF_INET, SOCK_DGRAM)"));
        }

        // SAFETY: the descriptor is fresh, is owned by nothing else,
        // and is not -1.
        let socket = UdpSocket {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        };

        socket.set_flag(libc::IPPROTO_IP, libc::IP_PKTINFO, "IP_PKTINFO")?;
        socket.set_flag(libc::SOL_SOCKET, libc::SO_BROADCAST, "SO_BROADCAST")?;

        Ok(socket)
    }

    /// Serve only `ifname`, so that this socket sees one interface's
    /// traffic and can share a port with the sockets serving the
    /// others.
    ///
    /// Measured to need no privilege on the kernel this was built
    /// against, though it has historically wanted the same capability
    /// the packet socket below does. Either way a refusal carries the
    /// call's name and the kernel's own errno.
    pub(crate) fn serve_only(&self, ifname: &str) -> Result<()> {
        self.set_option(
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            ifname.as_bytes(),
            "SO_BINDTODEVICE",
        )
    }

    /// Bind the wildcard address at `port`.
    ///
    /// The wildcard rather than the interface's address, because a
    /// socket bound to a specific address never receives the limited
    /// broadcast a client with no address sends.
    pub(crate) fn bind(&self, port: u16) -> Result<()> {
        let address = socket_address(Ipv4Addr::UNSPECIFIED, port);

        // SAFETY: the address is a live, fully initialised
        // `sockaddr_in`, and its length is that structure's own.
        let bound = unsafe {
            libc::bind(
                self.fd.as_raw_fd(),
                (&raw const address).cast::<libc::sockaddr>(),
                INET_ADDRESS_BYTES,
            )
        };
        if bound < 0 {
            return Err(syscall("bind"));
        }

        Ok(())
    }

    /// The port this socket is bound to.
    ///
    /// For a caller that bound port zero and needs to be told which
    /// port it got, which is how the receive path is exercised where
    /// the protocol's own port cannot be bound at all. The daemon
    /// always names a port, so nothing but a test asks.
    #[cfg(test)]
    pub(crate) fn port(&self) -> Result<u16> {
        let mut address: libc::sockaddr_in = zeroed();
        let mut length = INET_ADDRESS_BYTES;

        // SAFETY: both pointers are to live locals, and the length
        // tells the kernel how much of the address it may write.
        let named = unsafe {
            libc::getsockname(
                self.fd.as_raw_fd(),
                (&raw mut address).cast::<libc::sockaddr>(),
                &raw mut length,
            )
        };
        if named < 0 {
            return Err(syscall("getsockname"));
        }

        Ok(u16::from_be(address.sin_port))
    }

    /// Read one datagram, and what the kernel says about where it
    /// arrived.
    ///
    /// The caller's buffer is one octet longer than the largest
    /// datagram this front-end will parse, which is what makes an
    /// over-long datagram a datagram that is too long rather than a
    /// short one that looks well formed.
    pub(crate) fn receive(&self, buffer: &mut [u8]) -> io::Result<Receipt> {
        let mut control = Control::new();
        let mut piece = libc::iovec {
            iov_base: buffer.as_mut_ptr().cast::<libc::c_void>(),
            iov_len: buffer.len(),
        };
        let mut message: libc::msghdr = zeroed();
        message.msg_iov = &raw mut piece;
        message.msg_iovlen = 1;
        message.msg_control = control.0.as_mut_ptr().cast::<libc::c_void>();
        message.msg_controllen = CONTROL_BYTES;

        // SAFETY: every pointer in the message is to a live local that
        // outlives the call, and each length is that local's own.
        let read = unsafe { libc::recvmsg(self.fd.as_raw_fd(), &raw mut message, 0) };
        if read < 0 {
            return Err(io::Error::last_os_error());
        }
        let length = usize::try_from(read)
            .ok()
            .filter(|length| *length <= buffer.len())
            .ok_or_else(|| io::Error::other("the kernel read more than the buffer holds"))?;

        if message.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(io::Error::other(
                "the arrival interface did not fit in the control buffer",
            ));
        }

        let (ifindex, destination) = arrival(&message).ok_or_else(|| {
            io::Error::other("the kernel reported no arrival interface for a datagram")
        })?;

        Ok(Receipt {
            length,
            ifindex,
            destination,
        })
    }

    /// Send `datagram` to `destination` at `port`, out of `egress`,
    /// from `source`.
    ///
    /// The egress is named in the message because a limited broadcast
    /// from an interface with no default route has no route to be sent
    /// by, and the source is named with it so that the address in the
    /// IP header is the same one the reply's own server identifier
    /// carries.
    ///
    /// The socket is non-blocking, so a full transmit queue is
    /// reported rather than waited on. There is nothing useful to wait
    /// for: the client retransmits, and a server that blocked here
    /// would stop answering every other network while it did.
    pub(crate) fn send(
        &self,
        destination: Ipv4Addr,
        port: u16,
        egress: u32,
        source: Ipv4Addr,
        datagram: &[u8],
    ) -> io::Result<()> {
        let mut address = socket_address(destination, port);
        let mut control = Control::new();
        let mut piece = libc::iovec {
            // The kernel reads this and does not write it; `sendmsg`
            // takes one pointer type for both directions.
            iov_base: datagram.as_ptr().cast_mut().cast::<libc::c_void>(),
            iov_len: datagram.len(),
        };
        let mut message: libc::msghdr = zeroed();
        message.msg_name = (&raw mut address).cast::<libc::c_void>();
        message.msg_namelen = INET_ADDRESS_BYTES;
        message.msg_iov = &raw mut piece;
        message.msg_iovlen = 1;
        message.msg_control = control.0.as_mut_ptr().cast::<libc::c_void>();
        message.msg_controllen = CONTROL_BYTES;

        name_egress(&mut message, egress, source)?;

        // SAFETY: every pointer in the message is to a live local that
        // outlives the call, and each length is that local's own.
        let sent = unsafe { libc::sendmsg(self.fd.as_raw_fd(), &raw const message, 0) };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        if usize::try_from(sent).unwrap_or(0) != datagram.len() {
            return Err(io::Error::other("the datagram was not sent whole"));
        }

        Ok(())
    }

    /// Turn one boolean socket option on.
    fn set_flag(&self, level: libc::c_int, name: libc::c_int, call: &'static str) -> Result<()> {
        self.set_option(level, name, &1_i32.to_ne_bytes(), call)
    }

    /// Set one socket option from the bytes of its value.
    fn set_option(
        &self,
        level: libc::c_int,
        name: libc::c_int,
        value: &[u8],
        call: &'static str,
    ) -> Result<()> {
        let length = libc::socklen_t::try_from(value.len()).map_err(|_| Error::OptionTooLong {
            call,
            length: value.len(),
        })?;

        // SAFETY: the value is a live slice that outlives the call and
        // the length given is its own.
        let set = unsafe {
            libc::setsockopt(
                self.fd.as_raw_fd(),
                level,
                name,
                value.as_ptr().cast::<libc::c_void>(),
                length,
            )
        };
        if set < 0 {
            return Err(syscall(call));
        }

        Ok(())
    }
}

impl AsRawFd for UdpSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

/// One interface's datagram socket, held by both halves of its
/// datapath.
///
/// Shared rather than opened twice, because a reply to a client that
/// holds an address is sent by UDP from the port the request arrived
/// at, and a second socket could not bind that port beside this one.
#[derive(Clone, Debug)]
pub(crate) struct Shared(Arc<UdpSocket>);

impl Shared {
    /// Share one socket between a receiver and a sender.
    pub(crate) fn new(socket: UdpSocket) -> Self {
        Shared(Arc::new(socket))
    }
}

impl Deref for Shared {
    type Target = UdpSocket;

    fn deref(&self) -> &UdpSocket {
        &self.0
    }
}

impl AsRawFd for Shared {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

/// A socket that puts a whole frame on an interface.
///
/// Opened with no protocol, which is the difference between a socket
/// that sends and one that also receives every IP packet on the
/// machine into a queue nothing drains. Nothing is read from this
/// socket; the datagram socket beside it is where requests arrive.
#[derive(Debug)]
pub(crate) struct PacketSocket {
    /// The descriptor, closed when this value is dropped.
    fd: OwnedFd,
}

impl PacketSocket {
    /// Open the socket a frame leaves by.
    ///
    /// This is the call that needs the privilege to use the network
    /// below the IP layer, and the refusal without it carries the
    /// kernel's own errno, which is what tells an operator that a
    /// capability is missing rather than that something is broken.
    pub(crate) fn open() -> Result<Self> {
        // SAFETY: `socket` takes three integers and returns a
        // descriptor or -1. Nothing is borrowed across the call.
        let fd = unsafe { libc::socket(libc::AF_PACKET, libc::SOCK_RAW | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return Err(syscall("socket(AF_PACKET, SOCK_RAW)"));
        }

        // SAFETY: the descriptor is fresh, is owned by nothing else,
        // and is not -1.
        Ok(PacketSocket {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        })
    }

    /// Put `frame` on the interface at `egress`, addressed to
    /// `destination`.
    ///
    /// The frame carries its own Ethernet header, which is the whole
    /// point of this socket; the address below names the interface and
    /// the protocol the frame declares, both of which the kernel needs
    /// in order to know where to put it.
    pub(crate) fn send(
        &self,
        egress: u32,
        destination: [u8; ETHERNET_ADDRESS_BYTES],
        frame: &[u8],
    ) -> io::Result<()> {
        let mut address: libc::sockaddr_ll = zeroed();
        address.sll_family = PACKET_FAMILY;
        address.sll_protocol = ETHERNET_PROTOCOL_IPV4.to_be();
        address.sll_ifindex = i32::try_from(egress)
            .map_err(|_| io::Error::other("the egress interface index does not fit"))?;
        address.sll_halen = u8::try_from(destination.len()).unwrap_or(0);
        for (slot, octet) in address.sll_addr.iter_mut().zip(destination) {
            *slot = octet;
        }

        // SAFETY: the address is a live, fully initialised
        // `sockaddr_ll`, the frame is a live slice, and both outlive
        // the call.
        let sent = unsafe {
            libc::sendto(
                self.fd.as_raw_fd(),
                frame.as_ptr().cast::<libc::c_void>(),
                frame.len(),
                0,
                (&raw const address).cast::<libc::sockaddr>(),
                LINK_ADDRESS_BYTES,
            )
        };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        if usize::try_from(sent).unwrap_or(0) != frame.len() {
            return Err(io::Error::other("the frame was not sent whole"));
        }

        Ok(())
    }
}

/// A value of a plain C structure with every byte zero.
///
/// Zeroed rather than left uninitialised, because the kernel writes
/// only the members a call is about and reading one it did not write
/// would be reading whatever was on the stack. Every structure this is
/// called for is integers, arrays and addresses, so all-zero is one of
/// its values.
fn zeroed<T>() -> T {
    // SAFETY: called only for the C structures in this file, each of
    // which is plain data with no reference, no enumeration and no
    // niche.
    unsafe { mem::zeroed() }
}

/// An IPv4 socket address, in the byte order the kernel takes it in.
fn socket_address(address: Ipv4Addr, port: u16) -> libc::sockaddr_in {
    let mut socket: libc::sockaddr_in = zeroed();
    socket.sin_family = INET_FAMILY;
    socket.sin_port = port.to_be();
    socket.sin_addr = libc::in_addr {
        s_addr: address.to_bits().to_be(),
    };

    socket
}

/// Say which interface a message leaves by, and which address it
/// leaves from.
///
/// One control message, written into the buffer the caller already
/// attached. The buffer is exactly the size of this one message, so
/// there is no second message to make room for.
fn name_egress(message: &mut libc::msghdr, egress: u32, source: Ipv4Addr) -> io::Result<()> {
    let mut info: libc::in_pktinfo = zeroed();
    info.ipi_ifindex = i32::try_from(egress)
        .map_err(|_| io::Error::other("the egress interface index does not fit"))?;
    info.ipi_spec_dst = libc::in_addr {
        s_addr: source.to_bits().to_be(),
    };

    // SAFETY: the message's control buffer is live, is aligned for a
    // header, and its length was set to the space for exactly one.
    let header = unsafe { libc::CMSG_FIRSTHDR(message) };
    if header.is_null() {
        return Err(io::Error::other("a control message does not fit"));
    }

    let payload = mem::size_of::<libc::in_pktinfo>();
    let length = u32::try_from(payload)
        .map_err(|_| io::Error::other("a control message's payload does not fit its length"))?;

    // SAFETY: the header points into the live, aligned control buffer
    // above, with room for one message of this size, and the payload
    // is written through the pointer the kernel's own macro returns.
    unsafe {
        (*header).cmsg_level = libc::IPPROTO_IP;
        (*header).cmsg_type = libc::IP_PKTINFO;
        (*header).cmsg_len = libc::CMSG_LEN(length) as usize;
        ptr::write_unaligned(libc::CMSG_DATA(header).cast::<libc::in_pktinfo>(), info);
    }

    Ok(())
}

/// The interface a datagram arrived on and the address it was sent to.
///
/// `None` when the kernel attached no such message, which it does not
/// do for a socket that asked for one -- so the caller reports it
/// rather than guessing an interface.
fn arrival(message: &libc::msghdr) -> Option<(u32, Ipv4Addr)> {
    // SAFETY: the message and its control buffer are live and the
    // buffer's length is the one the kernel was given, so walking it
    // with the kernel's own macros stays inside it.
    let mut header = unsafe { libc::CMSG_FIRSTHDR(message) };

    while !header.is_null() {
        // SAFETY: the header is non-null and came from the walk above,
        // so it points at a message inside the live control buffer.
        let (level, kind) = unsafe { ((*header).cmsg_level, (*header).cmsg_type) };

        if level == libc::IPPROTO_IP && kind == libc::IP_PKTINFO {
            // SAFETY: an `IP_PKTINFO` message's payload is an
            // `in_pktinfo`, which is what the two members above
            // establish. The read is unaligned because the buffer
            // promises alignment for the header and not for the
            // payload behind it.
            let info =
                unsafe { ptr::read_unaligned(libc::CMSG_DATA(header).cast::<libc::in_pktinfo>()) };

            return Some((
                u32::try_from(info.ipi_ifindex).ok()?,
                Ipv4Addr::from_bits(u32::from_be(info.ipi_addr.s_addr)),
            ));
        }

        // SAFETY: both pointers are the ones the previous call
        // returned, which is what this macro takes.
        header = unsafe { libc::CMSG_NXTHDR(message, header) };
    }

    None
}
