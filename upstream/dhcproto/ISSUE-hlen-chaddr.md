# Draft issue for bluecatengineering/dhcproto

**Not submitted.** See [README.md](README.md) for the project's
policy and the disclosure question. A fix for this is drafted
alongside it in [PR-hlen-chaddr.md](PR-hlen-chaddr.md); if both go
out, add a line to the issue pointing at the pull request.
Everything below the line is the issue body, ready to paste.

Suggested title:

    v4 chaddr() accessors slice by the packet's unvalidated hlen byte

---

### Summary

Both `chaddr()` accessors in the v4 module size a slice using
`hlen`, which is read from the packet without validation, while
`chaddr` itself is the fixed 16-byte BOOTP field. A single
unauthenticated DHCP packet with `hlen > 16` is enough to panic a
server that calls `Message::chaddr()` on a decoded message, which
is typically the first thing a server does with one.

RFC 2131 fixes `chaddr` at 16 octets and `hlen` is a `u8`, so
`hlen` can name a length the field cannot hold. Nothing rejects
that today.

Affects 0.15.0, the current release, and `master` at 5ba6f4e
(`Cargo.toml` version 0.16.0, unpublished). Both accessors carry
it at the same lines in both.

### 1. `v4::Message::chaddr()` — panics

The three relevant lines in `src/v4/mod.rs`:

```rust
// decode, line 477
hlen: decoder.read_u8()?,          // straight off the wire, 0..=255

// decode, line 486
chaddr: decoder.read::<16>()?,     // fixed 16 bytes

// accessor, line 288
pub fn chaddr(&self) -> &[u8] {
    &self.chaddr[..(self.hlen as usize)]
}
```

Reproducer, dependencies `dhcproto = "0.15.0"` only:

```rust
use dhcproto::{Decodable, Decoder, v4::Message};

fn main() {
    // A minimal, otherwise well-formed BOOTREQUEST whose hlen byte
    // claims 255 while chaddr is the usual 16-byte field.
    let mut p = vec![0u8; 240];
    p[0] = 1; // op: BOOTREQUEST
    p[1] = 1; // htype: ethernet
    p[2] = 255; // hlen
    p[4..8].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]); // xid
    p[28..32].copy_from_slice(&[0x02, 0x00, 0x00, 0x11]); // chaddr
    p[236..240].copy_from_slice(&[99, 130, 83, 99]); // magic cookie
    p.extend_from_slice(&[53, 1, 1, 255]); // DHCPDISCOVER, end

    let msg = Message::decode(&mut Decoder::new(&p)).expect("decodes fine");
    println!("hlen = {}", msg.hlen());
    let _ = msg.chaddr(); // panics
}
```

Output:

```
hlen = 255
thread 'main' panicked at src/v4/mod.rs:289:21:
range end index 255 out of range for slice of length 16
```

Note that `decode` succeeds: the packet is rejected nowhere, and
the panic arrives later, in an accessor that looks total.

### 2. `v4::borrowed::Message::chaddr()` — panics, or returns the wrong bytes

`src/v4/borrowed.rs`:

```rust
// line 22, the only length check
if buffer.len() < 240 { return Err(DecodeError::NotEnoughBytes); }

// line 91
pub fn chaddr(&self) -> &'a [u8] {
    &self.buffer[28..28 + self.hlen() as usize]
}
```

Here the slice is taken from the whole packet buffer rather than
from a 16-byte array, so which way it fails depends on the
packet's length rather than on anything about the client:

| Buffer length | `hlen = 255` | Result |
|---|---|---|
| 240 (the minimum) | `28..283` | panic, `range end index 283 out of range for slice of length 240` |
| 300 (an ordinary DISCOVER) | `28..283` | **no panic**; returns 255 bytes |

In the second row the slice stays inside the caller's buffer, so
this is not a memory-safety problem -- it is safe code returning
the wrong bytes. But the bytes it returns run through `sname`,
`file` and into the options area, all of which the client also
chose. So `chaddr()` can report a 255-byte "hardware address"
whose tail the client controls, which matters for anything keying
a lease table or a reservation on the client's hardware address.

Reproducer, same single dependency:

```rust
use dhcproto::v4::borrowed::Message;

fn main() {
    for len in [240usize, 300] {
        let mut p = vec![0u8; len];
        p[0] = 1;
        p[1] = 1;
        p[2] = 255; // hlen
        p[28..32].copy_from_slice(&[0x02, 0x00, 0x00, 0x11]); // real chaddr
        p[236..240].copy_from_slice(&[99, 130, 83, 99]);
        // Mark everything past chaddr, so it is obvious in the output
        // where the returned bytes came from.
        for b in p[44..236].iter_mut() {
            *b = 0x41;
        }
        let msg = Message::new(&p).expect("new succeeds");
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let c = msg.chaddr();
            (c.len(), c[16..24].to_vec())
        }));
        match r {
            Ok((n, past)) => println!("len {len}: no panic, chaddr() = {n} bytes, \
bytes 16..24 past the real chaddr = {past:02x?}"),
            Err(_) => println!("len {len}: panicked"),
        }
    }
}
```

Output:

```
len 240: panicked
len 300: no panic, chaddr() = 255 bytes, bytes 16..24 past the real chaddr = [41, 41, 41, 41, 41, 41, 41, 41]
```

(The panic hook also writes `range end index 283 out of range for
slice of length 240` to stderr for the first case, since
`catch_unwind` does not suppress it.)

### Suggested fix

The setter is already careful -- `set_chaddr` truncates at 16 --
so this is the read path only, and either of these closes both
accessors:

1. **Clamp in the accessors.** `&self.chaddr[..min(self.hlen as
   usize, 16)]`, and the same bound in the borrowed path. Smallest
   change, keeps every existing packet decoding.
2. **Validate at decode.** Reject `hlen > 16` with a
   `DecodeError`, which is arguably the more correct behaviour
   since such a packet is malformed, but it will start rejecting
   packets that previously decoded.

I would suggest (1) for the accessors regardless of whether (2) is
also adopted, so that a `Message` constructed by hand cannot
panic either. A regression test for each accessor along the lines
of the reproducers above would pin it.

I have a patch doing (1) with a regression test per accessor,
which passes `cargo test --all-features`, `cargo fmt --check` and
`cargo clippy -- -D warnings` on the toolchain `rust-toolchain.toml`
pins, and which fails exactly those two tests with the fix reverted.
Happy to open it as a pull request.
