//! What an interface says about itself, asked of the kernel.
//!
//! Four questions: an interface's index, the name of an index, the
//! hardware address a frame leaves with, and the address and prefix
//! every option but the lease time is derived from.
//!
//! # Through a socket, never through `/sys/class/net`
//!
//! Both ways of enumerating interfaces work today and only one of
//! them keeps working. `/sys/class/net` shows the network namespace
//! its mount was made in, which is not necessarily the namespace the
//! asking thread is in; an `ioctl` on a socket follows the thread.
//! There is one namespace here and the two agree, so the choice costs
//! nothing now and is the difference between working and silently
//! wrong the day this process serves interfaces in namespaces it has
//! moved into. A front-end that read a name out of sysfs would answer
//! a request from the network that claimed an interface of that name
//! in the wrong namespace, which is the failure this whole project is
//! shaped to prevent arriving by the back door.
//!
//! Reading the addresses goes the same way: `getifaddrs` asks the
//! kernel over a netlink socket it opens for the call, in the calling
//! thread's namespace, rather than reading a filesystem whose mount
//! was made somewhere else.
//!
//! # Asked for every datagram, remembered between none
//!
//! A front-end holds no per-network state, and an interface's address
//! is a fact about the machine rather than something configured here,
//! so it is read for each request and kept for none. The cost is a
//! netlink round trip per datagram, which a protocol whose clients
//! speak twice a lease time can afford, and what it buys is that an
//! address an operator changes takes effect on the next request rather
//! than on the next restart -- and that there is no second place to
//! look when a client got an answer nobody expected.

use std::ffi::{CStr, c_char};
use std::mem;
use std::net::Ipv4Addr;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::ptr;

use crate::datapath::error::{Error, Result, syscall};
use crate::wire::ServerFacts;

/// The bits an IPv4 address has.
const ADDRESS_BITS: u8 = 32;

/// The octets an Ethernet address has.
///
/// Public because [`Interfaces::hardware_address`] returns an array of
/// this length and [`crate::datapath::Delivery`] carries one, so
/// anything naming either needs the number this crate used.
pub const ETHERNET_ADDRESS_BYTES: usize = 6;

/// A socket to ask interface questions through.
///
/// The socket is never bound, connected, read or written. It exists
/// because an `ioctl` needs a descriptor to be made on, and because
/// which namespace that descriptor belongs to is what decides which
/// interfaces the answers are about.
#[derive(Debug)]
pub struct Interfaces {
    /// The descriptor the `ioctl`s are made on.
    socket: OwnedFd,
}

impl Interfaces {
    /// Open the socket the questions are asked through.
    ///
    /// A datagram socket in the IPv4 family, which needs no privilege
    /// and carries no traffic.
    pub fn open() -> Result<Self> {
        // SAFETY: `socket` takes three integers and returns a
        // descriptor or -1. Nothing is borrowed across the call.
        let fd = unsafe {
            libc::socket(
                libc::AF_INET,
                libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
                libc::IPPROTO_UDP,
            )
        };
        if fd < 0 {
            return Err(syscall("socket(AF_INET, SOCK_DGRAM)"));
        }

        // SAFETY: the descriptor is fresh, is owned by nothing else,
        // and is not -1.
        let socket = unsafe { OwnedFd::from_raw_fd(fd) };

        Ok(Interfaces { socket })
    }

    /// The index the kernel knows `ifname` by, in this socket's
    /// namespace.
    ///
    /// An index is only meaningful inside the namespace it was
    /// resolved in, so this answer travels no further than the sockets
    /// created beside it.
    pub fn index_of(&self, ifname: &str) -> Result<u32> {
        let mut request = self.named(ifname)?;

        self.ask("ioctl(SIOCGIFINDEX)", libc::SIOCGIFINDEX, &mut request)?;

        // SAFETY: `SIOCGIFINDEX` is the request that fills
        // `ifru_ifindex`, so that is the member of the union the
        // kernel wrote.
        let index = unsafe { request.ifr_ifru.ifru_ifindex };

        u32::try_from(index).map_err(|_| Error::IndexOutOfRange {
            index: i64::from(index),
        })
    }

    /// The name of the interface at `index`, in this socket's
    /// namespace.
    ///
    /// This is the half of the receive path that turns what the kernel
    /// reports about a datagram's arrival into the name an operator
    /// attached a network to.
    pub fn name_of(&self, index: u32) -> Result<String> {
        let mut request = zeroed_request();
        request.ifr_ifru.ifru_ifindex =
            i32::try_from(index).map_err(|_| Error::IndexOutOfRange {
                index: i64::from(index),
            })?;

        self.ask("ioctl(SIOCGIFNAME)", libc::SIOCGIFNAME, &mut request)?;

        // SAFETY: the kernel writes a terminated name into
        // `ifr_name`, which is an array inside the value borrowed
        // here, so a terminator is found within it.
        let name = unsafe { CStr::from_ptr(request.ifr_name.as_ptr()) };

        name.to_str()
            .map(str::to_owned)
            .map_err(|_| Error::NameNotText { index })
    }

    /// The hardware address frames from `ifname` are sent with.
    ///
    /// Six octets, because the frame this front-end builds is an
    /// Ethernet one, and an interface of another kind is refused by
    /// name rather than having its address truncated into one.
    pub fn hardware_address(&self, ifname: &str) -> Result<[u8; ETHERNET_ADDRESS_BYTES]> {
        let mut request = self.named(ifname)?;

        self.ask("ioctl(SIOCGIFHWADDR)", libc::SIOCGIFHWADDR, &mut request)?;

        // SAFETY: `SIOCGIFHWADDR` is the request that fills
        // `ifru_hwaddr`, so that is the member of the union the kernel
        // wrote.
        let address = unsafe { request.ifr_ifru.ifru_hwaddr };
        if address.sa_family != libc::ARPHRD_ETHER {
            return Err(Error::NotEthernetInterface {
                ifname: ifname.to_owned(),
                hardware_type: address.sa_family,
            });
        }

        let mut hardware = [0_u8; ETHERNET_ADDRESS_BYTES];
        for (octet, byte) in hardware.iter_mut().zip(address.sa_data.iter()) {
            *octet = *byte as u8;
        }

        Ok(hardware)
    }

    /// The address and prefix `ifname` holds, which an answer's
    /// options are derived from.
    ///
    /// Exactly one IPv4 address, or a refusal naming the interface.
    pub fn facts(&self, ifname: &str) -> Result<ServerFacts> {
        only_address(ifname, &addresses(ifname)?)
    }

    /// An `ifreq` naming `ifname` and nothing else.
    fn named(&self, ifname: &str) -> Result<libc::ifreq> {
        let mut request = zeroed_request();
        name_into(ifname, &mut request.ifr_name)?;

        Ok(request)
    }

    /// Make one request of the kernel about one interface.
    ///
    /// The name of the call is passed rather than derived from the
    /// code, so that a failure says which question was asked in the
    /// spelling its manual page uses.
    fn ask(&self, call: &'static str, code: libc::Ioctl, request: &mut libc::ifreq) -> Result<()> {
        // SAFETY: the request is a fully initialised `ifreq`, every
        // code passed here reads one of its members and writes
        // another inside the same value, and the borrow lasts only
        // for the call.
        if unsafe { libc::ioctl(self.socket.as_raw_fd(), code, request as *mut libc::ifreq) } < 0 {
            return Err(syscall(call));
        }

        Ok(())
    }
}

/// An `ifreq` with every byte zero.
///
/// Zeroed rather than left uninitialised: the kernel writes only the
/// members a given request is about, and reading a member it did not
/// write would be reading whatever was on the stack. Every member of
/// this structure is plain data, so a zeroed one is a valid value of
/// it.
fn zeroed_request() -> libc::ifreq {
    // SAFETY: `ifreq` is a C structure of integers, arrays and
    // addresses, with no reference, no enumeration and no niche, so
    // all-zero is one of its values.
    unsafe { mem::zeroed() }
}

/// Copy an interface name into the field the kernel takes it in.
///
/// Refused rather than truncated. The kernel reads the field up to its
/// first zero octet, so a name that does not fit, or that holds a zero
/// of its own, would quietly name a different interface than the one
/// asked about -- which on this datapath means answering from the
/// wrong network.
fn name_into(ifname: &str, field: &mut [c_char]) -> Result<()> {
    let bytes = ifname.as_bytes();
    let limit = field.len();

    if bytes.is_empty() || bytes.contains(&0) {
        return Err(Error::NameNotUsable {
            ifname: ifname.to_owned(),
        });
    }
    if bytes.len() >= limit {
        return Err(Error::NameTooLong {
            ifname: ifname.to_owned(),
            limit,
        });
    }

    for (slot, byte) in field.iter_mut().zip(bytes) {
        *slot = *byte as c_char;
    }

    Ok(())
}

/// Every IPv4 address and netmask the interface `ifname` holds.
///
/// `getifaddrs` lists every interface, so the name is matched here. An
/// entry with an address and no mask describes no subnet, so it is not
/// one an answer could be derived from and is not counted as one.
fn addresses(ifname: &str) -> Result<Vec<(Ipv4Addr, Ipv4Addr)>> {
    let mut head: *mut libc::ifaddrs = ptr::null_mut();

    // SAFETY: the call writes the head of its list through the
    // pointer it is given, which is a pointer to a live local.
    if unsafe { libc::getifaddrs(&raw mut head) } != 0 {
        return Err(syscall("getifaddrs"));
    }

    // Freed by this value's `Drop`, so that no path out of the loop
    // below can leak the list.
    let list = List { head };
    let mut found = Vec::new();
    let mut entry = list.head;

    while !entry.is_null() {
        // SAFETY: the entry came from the kernel's list and is not
        // null, so it points at a live `ifaddrs` which the value above
        // keeps alive for the whole of this loop.
        let current = unsafe { &*entry };
        entry = current.ifa_next;

        if current.ifa_name.is_null() {
            continue;
        }
        // SAFETY: a non-null `ifa_name` is a terminated string the
        // kernel wrote, valid for as long as the list is.
        let name = unsafe { CStr::from_ptr(current.ifa_name) };
        if name.to_bytes() != ifname.as_bytes() {
            continue;
        }

        if let (Some(address), Some(netmask)) =
            (ipv4_of(current.ifa_addr), ipv4_of(current.ifa_netmask))
        {
            found.push((address, netmask));
        }
    }

    Ok(found)
}

/// The list `getifaddrs` allocated, freed however this scope ends.
struct List {
    /// The head of the list, which is what `freeifaddrs` takes.
    head: *mut libc::ifaddrs,
}

impl Drop for List {
    fn drop(&mut self) {
        // SAFETY: the pointer came from a `getifaddrs` that returned
        // success, has not been freed, and is freed exactly once
        // because this value owns it.
        unsafe { libc::freeifaddrs(self.head) }
    }
}

/// The IPv4 address a socket address holds, or `None` for anything
/// else.
///
/// A null pointer, an address in another family and an address of
/// another width all read as "not an IPv4 address here", which is what
/// the caller wants: an interface also holds IPv6 addresses, a
/// hardware address, and sometimes nothing at all.
fn ipv4_of(address: *const libc::sockaddr) -> Option<Ipv4Addr> {
    if address.is_null() {
        return None;
    }

    // SAFETY: a non-null socket address from the kernel's list points
    // at a `sockaddr` whose family member is readable. The read is
    // unaligned because the list makes no alignment promise.
    let family = unsafe { ptr::read_unaligned(&raw const (*address).sa_family) };
    if i32::from(family) != libc::AF_INET {
        return None;
    }

    // SAFETY: an `AF_INET` socket address is a `sockaddr_in`, which is
    // what the family read above establishes, and this read is
    // unaligned for the same reason.
    let inet = unsafe { ptr::read_unaligned(address.cast::<libc::sockaddr_in>()) };

    Some(Ipv4Addr::from_bits(u32::from_be(inet.sin_addr.s_addr)))
}

/// The one address an interface holds, as an answer is derived from
/// it.
///
/// Separate from the kernel call above so that the judgement -- none
/// is a refusal, several is a refusal, one is an answer -- is a
/// function of a list rather than a function of the machine the tests
/// happen to run on.
fn only_address(ifname: &str, addresses: &[(Ipv4Addr, Ipv4Addr)]) -> Result<ServerFacts> {
    let (address, netmask) = match addresses {
        [] => {
            return Err(Error::NoAddress {
                ifname: ifname.to_owned(),
            });
        }
        [one] => *one,
        several => {
            return Err(Error::SeveralAddresses {
                ifname: ifname.to_owned(),
                count: several.len(),
            });
        }
    };

    ServerFacts::new(address, prefix_length(ifname, netmask)?).map_err(|source| Error::Facts {
        ifname: ifname.to_owned(),
        source,
    })
}

/// How many bits of prefix a netmask describes.
///
/// A mask is a run of ones followed by a run of zeroes, and anything
/// else is refused rather than counted: the count would be a number
/// the mask does not mean, and a client would hold it for the life of
/// its lease.
fn prefix_length(ifname: &str, netmask: Ipv4Addr) -> Result<u8> {
    let bits = netmask.to_bits();
    let length = u8::try_from(bits.leading_ones()).unwrap_or(ADDRESS_BITS);
    let canonical = u32::MAX
        .checked_shl(u32::from(ADDRESS_BITS.saturating_sub(length)))
        .unwrap_or(0);

    if bits != canonical {
        return Err(Error::NetmaskNotAPrefix {
            ifname: ifname.to_owned(),
            netmask,
        });
    }

    Ok(length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interface_with_one_address_is_what_an_answer_is_derived_from() {
        let facts = only_address("gw-n1", &[(address("10.0.0.1"), netmask(24))])
            .expect("one address is one answer");

        assert_eq!(facts.address(), address("10.0.0.1"));
        assert_eq!(facts.prefix_length(), 24);
        assert_eq!(facts.netmask(), netmask(24));
        assert_eq!(facts.broadcast(), address("10.0.0.255"));
    }

    #[test]
    fn an_interface_with_no_address_is_refused_by_name() {
        let refusal = only_address("gw-n1", &[]).expect_err("nothing to answer from");

        assert!(
            matches!(&refusal, Error::NoAddress { ifname } if ifname == "gw-n1"),
            "the refusal names the interface: {refusal}"
        );
        assert!(refusal.to_string().contains("gw-n1"));
    }

    #[test]
    fn an_interface_with_two_addresses_is_refused_rather_than_answered_from_the_first() {
        let refusal = only_address(
            "gw-n1",
            &[
                (address("10.0.0.1"), netmask(24)),
                (address("10.0.0.2"), netmask(24)),
            ],
        )
        .expect_err("a server may not choose which address it is");

        assert!(
            matches!(
                &refusal,
                Error::SeveralAddresses { ifname, count } if ifname == "gw-n1" && *count == 2
            ),
            "the refusal names the interface and the count: {refusal}"
        );
    }

    #[test]
    fn a_netmask_becomes_the_prefix_it_describes() {
        for length in 0..=ADDRESS_BITS {
            assert_eq!(
                prefix_length("gw-n1", netmask(length)).expect("a mask is a prefix"),
                length,
                "a mask of {length} bits is a prefix of {length} bits"
            );
        }
    }

    #[test]
    fn a_netmask_that_is_not_a_prefix_is_refused_by_name() {
        let refusal = prefix_length("gw-n1", Ipv4Addr::new(255, 255, 0, 255))
            .expect_err("a mask with a hole in it is not a prefix");

        assert!(
            matches!(&refusal, Error::NetmaskNotAPrefix { ifname, .. } if ifname == "gw-n1"),
            "the refusal names the interface: {refusal}"
        );
    }

    #[test]
    fn a_name_longer_than_the_kernels_field_is_refused_rather_than_truncated() {
        let mut field = [0 as c_char; libc::IFNAMSIZ];
        let refusal = name_into(&"e".repeat(libc::IFNAMSIZ), &mut field)
            .expect_err("a name that does not fit names another interface");

        assert!(matches!(refusal, Error::NameTooLong { .. }), "{refusal}");
    }

    #[test]
    fn a_name_holding_a_zero_octet_is_refused_rather_than_cut_at_it() {
        let mut field = [0 as c_char; libc::IFNAMSIZ];
        let refusal = name_into("gw\0n1", &mut field)
            .expect_err("the kernel would read gw, which is a different interface");

        assert!(matches!(refusal, Error::NameNotUsable { .. }), "{refusal}");
    }

    #[test]
    fn a_name_that_fits_reaches_the_field_terminated() {
        let mut field = [0 as c_char; libc::IFNAMSIZ];
        name_into("gw-n1", &mut field).expect("a name that fits");

        // SAFETY: the field was zeroed and a shorter name was written
        // into it, so a terminator is inside the array.
        let written = unsafe { CStr::from_ptr(field.as_ptr()) };
        assert_eq!(written.to_bytes(), b"gw-n1");
    }

    /// An index resolves to a name through a socket, and back.
    /// Loopback is the one interface this test environment is
    /// promised, and the round trip is the point rather than the name.
    #[test]
    fn an_interface_index_resolves_to_a_name_through_a_socket() {
        let interfaces = Interfaces::open().expect("a socket to ask through");
        let index = interfaces.index_of("lo").expect("loopback has an index");

        assert_eq!(
            interfaces.name_of(index).expect("an index has a name"),
            "lo"
        );
    }

    #[test]
    fn an_interface_nothing_is_called_is_refused_with_the_kernels_errno() {
        let interfaces = Interfaces::open().expect("a socket to ask through");
        let refusal = interfaces
            .index_of("teneral-nope")
            .expect_err("no such interface");

        assert!(
            matches!(&refusal, Error::Syscall { call, .. } if call.contains("SIOCGIFINDEX")),
            "the refusal names the call: {refusal}"
        );
    }

    /// Loopback holds one address here, so the whole derivation runs
    /// against a real kernel even though nothing can be served on it.
    #[test]
    fn the_kernel_answers_what_an_interface_holds() {
        let interfaces = Interfaces::open().expect("a socket to ask through");
        let facts = interfaces.facts("lo").expect("loopback holds one address");

        assert_eq!(facts.address(), Ipv4Addr::LOCALHOST);
        assert_eq!(facts.prefix_length(), 8);
    }

    /// An address, written as an operator writes one.
    fn address(written: &str) -> Ipv4Addr {
        written.parse().expect("a written address")
    }

    /// The mask for a prefix of `length` bits.
    fn netmask(length: u8) -> Ipv4Addr {
        Ipv4Addr::from_bits(
            u32::MAX
                .checked_shl(u32::from(ADDRESS_BITS - length))
                .unwrap_or(0),
        )
    }
}
