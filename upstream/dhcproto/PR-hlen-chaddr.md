# Draft pull request for bluecatengineering/dhcproto

**Not submitted.** See [README.md](README.md) for the project's
policy and the disclosure question. The patch is
[0001-clamp-chaddr-to-the-field.patch](0001-clamp-chaddr-to-the-field.patch),
against `master` at `5ba6f4e7781b`.

Suggested title:

    Clamp chaddr() to the 16-octet field rather than indexing by hlen

Everything below the line is the pull request body.

---

### What

Both `chaddr()` accessors in the v4 module size a slice using
`hlen`, which is read from the packet without validation, while
`chaddr` itself is the fixed 16-octet BOOTP field. `hlen` is a
`u8`, so it can name a length the field cannot hold, and nothing
rejects that today.

The consequence depends on which accessor:

- `v4::Message::chaddr()` (`src/v4/mod.rs`) slices a `[u8; 16]`,
  so any `hlen > 16` panics: `range end index 255 out of range for
  slice of length 16`. The message decodes successfully first, so
  the panic arrives later, from an accessor whose signature looks
  total.
- `v4::borrowed::Message::chaddr()` (`src/v4/borrowed.rs`) slices
  the whole packet buffer, which is only length-checked against a
  240-byte minimum. On a short buffer it panics; on an ordinary
  300-byte DISCOVER it does not, and returns up to 255 bytes
  running through `sname`, `file` and into the options as the
  client's hardware address. That stays inside the caller's
  buffer, so it is not a memory-safety problem -- it is safe code
  returning the wrong bytes, which matters for anything keying a
  lease or a reservation on `chaddr()`.

Since a DHCP server generally calls `chaddr()` on every packet it
parses, in the first case this is reachable from a single
unauthenticated packet.

### How

`min` on the length in both accessors, so the field bounds the
slice rather than the wire byte. The owned path clamps to
`self.chaddr.len()`; the borrowed path clamps to 16, matching the
literal field offsets that file already uses.

Only the read path needed it: `set_chaddr` already truncates at
16.

I chose clamping over rejecting `hlen > 16` at decode time because
it is the smaller change and keeps every packet that decodes today
decoding. Rejecting at decode is arguably more correct, since such
a packet is malformed, and I would be glad to do that instead or
as well if you prefer -- it is a one-line addition next to
`hlen: decoder.read_u8()?`. Clamping the accessors is worth having
either way, so that a `Message` built by hand cannot panic.

### Tests

One test per accessor, in the existing `mod tests` of each file,
reusing the `discover()` and `bootreq()` fixtures already there
with the `hlen` byte set to `0xff`. The borrowed test covers both
buffer lengths, since the short and long cases fail differently.

Verified on the pinned toolchain (`rust-toolchain.toml`, 1.90.0):

- `cargo test --all-features`: 71 unit and 18 doc tests pass.
- Reverting just the two accessor changes and keeping the tests
  fails exactly those two tests and nothing else, with the two
  panic messages above.
- `cargo fmt --all -- --check`: clean.
- `cargo clippy --all-features -- -D warnings`: clean.

A `CHANGELOG.md` entry is included under `## [unreleased]` →
`### Fixed`.
