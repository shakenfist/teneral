//! The framing both halves of the control plane share.
//!
//! A varint byte count and then the encoded message, which is the form
//! prost writes for a length-delimited message rather than a header of
//! this project's own. Both directions, every message.
//!
//! # The bound, and where it is checked
//!
//! A message may be at most [`MAX_MESSAGE_BYTES`], and the bound is
//! checked **against the length prefix, before any body is read**. The
//! order is the whole point. This input is authenticated and local,
//! which is a reason to expect it to be well formed and not a reason
//! to allocate whatever a peer's bytes asked for: one process serves
//! every network here, so an out-of-memory kill is a cluster-wide
//! outage and not a local untidiness.
//!
//! So nothing in this module indexes a slice, unwraps, or does
//! arithmetic that could overflow on a value that came off a socket.
//! The length is accumulated in a `u64` from at most five varint
//! bytes, which cannot reach the top of the type, and it is compared
//! with the bound before it is ever converted into a size.
//!
//! A frame over the bound is refused. The connection then closes,
//! because a stream whose body was deliberately not read has nothing
//! left to resynchronise on -- the next byte is somewhere in the
//! middle of a message.
//!
//! # Why a collection is many frames
//!
//! Because the bound is then a bound on one resource rather than a
//! ceiling on how large a network may grow. A list of leases arrives
//! as a sequence of messages ended by a marker, which also means a
//! collection read and a watch snapshot are one code path instead of
//! two descriptions of one thing.

use prost::Message;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::{Error, Result};

/// The most bytes one message may be.
///
/// A megabyte, which is three orders of magnitude more than the
/// largest message this schema can produce -- a resource with every
/// field set is some hundreds of bytes -- and small enough that a
/// peer cannot ask this process for memory worth noticing. A
/// collection does not test it, each item being a frame of its own.
pub const MAX_MESSAGE_BYTES: u64 = 1024 * 1024;

/// The most bytes a length prefix may occupy.
///
/// Five continuation bytes carry thirty-five bits, which is well past
/// the bound above, so a prefix longer than this is not a length that
/// could ever be accepted: it is either a different protocol on the
/// socket or an attempt to find an arithmetic that wraps. Stopping at
/// a fixed count is what keeps the accumulation below inside its type
/// without a checked shift at each step.
const MAX_LENGTH_BYTES: u32 = 5;

/// Write `message`, length-delimited.
///
/// The bound is checked here too, on the daemon's own output. A
/// message this process cannot send is better refused by the handler
/// that built it than written as a frame the peer must close the
/// connection over -- and a bound enforced on one side only is a bound
/// that moves the day the other side is reimplemented.
pub async fn write_message<W, M>(writer: &mut W, message: &M) -> Result<()>
where
    W: AsyncWrite + Unpin,
    M: Message,
{
    let length = message.encoded_len();
    if u64::try_from(length).unwrap_or(u64::MAX) > MAX_MESSAGE_BYTES {
        return Err(Error::TooLarge {
            length: u64::try_from(length).unwrap_or(u64::MAX),
        });
    }

    // Encoded whole and then written, rather than encoded into the
    // socket. A half-written frame is indistinguishable from a frame
    // whose length prefix lied, and the peer's only recovery from
    // either is to close the connection.
    let mut frame = Vec::with_capacity(length.saturating_add(MAX_LENGTH_BYTES as usize));
    message
        .encode_length_delimited(&mut frame)
        .map_err(|error| Error::Framing {
            detail: format!("a message of {length} bytes would not encode: {error}"),
        })?;

    writer
        .write_all(&frame)
        .await
        .map_err(|error| Error::io("writing a message to the socket", error))?;
    writer
        .flush()
        .await
        .map_err(|error| Error::io("flushing a message to the socket", error))?;

    Ok(())
}

/// Read one length-delimited message.
///
/// A clean end of stream before any byte of the prefix is
/// [`Error::Closed`] rather than a failure: a peer that has finished
/// is the ordinary way a connection ends.
pub async fn read_message<R, M>(reader: &mut R) -> Result<M>
where
    R: AsyncRead + Unpin,
    M: Message + Default,
{
    let length = read_length(reader).await?;

    // The one place a size derived from a peer's bytes becomes an
    // allocation, and it is after the comparison rather than before
    // it. `length` is at most the bound here, so the conversion cannot
    // fail on any target this daemon runs on and the capacity is at
    // most a megabyte.
    let length = usize::try_from(length).map_err(|_| Error::TooLarge { length })?;
    let mut body = vec![0_u8; length];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::UnexpectedEof => Error::Closed {
                context: format!("reading the {length} bytes a frame said it carried"),
            },
            _ => Error::io("reading a message body from the socket", error),
        })?;

    M::decode(body.as_slice()).map_err(|error| Error::Decode {
        context: format!("a frame of {length} bytes"),
        source: error,
    })
}

/// Read the varint length prefix, and judge it.
///
/// Returns the length only when it is one this wire will carry, so the
/// caller has nothing left to check.
async fn read_length<R>(reader: &mut R) -> Result<u64>
where
    R: AsyncRead + Unpin,
{
    let mut length: u64 = 0;

    for position in 0..MAX_LENGTH_BYTES {
        let byte = match reader.read_u8().await {
            Ok(byte) => byte,
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(if position == 0 {
                    // Nothing at all arrived, which is how a peer says
                    // it has finished.
                    Error::Closed {
                        context: "waiting for the next message".to_owned(),
                    }
                } else {
                    Error::Framing {
                        detail: format!(
                            "the connection ended {position} bytes into a length prefix"
                        ),
                    }
                });
            }
            Err(error) => {
                return Err(Error::io("reading a message's length prefix", error));
            }
        };

        // `position` is below five, so the shift is at most 28 and the
        // seven bits being placed cannot leave a u64. No checked
        // arithmetic is needed because the loop's bound is what makes
        // the overflow unreachable rather than handled.
        length |= u64::from(byte & 0x7f) << (position * 7);

        if byte & 0x80 == 0 {
            return if length > MAX_MESSAGE_BYTES {
                // Refused here, with the body still unread and nothing
                // allocated for it.
                Err(Error::TooLarge { length })
            } else {
                Ok(length)
            };
        }
    }

    Err(Error::Framing {
        detail: format!(
            "its length did not end within {MAX_LENGTH_BYTES} bytes, so it is not a length this \
             wire issues"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::{MAX_MESSAGE_BYTES, read_message, write_message};
    use crate::error::Error;
    use crate::proto;

    /// The varint prefix for a frame claiming `length` bytes.
    ///
    /// Written by hand rather than taken from the encoder, because
    /// every test here is about a prefix the encoder would never
    /// produce.
    fn prefix(mut length: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        loop {
            let seven = u8::try_from(length & 0x7f).unwrap_or(0);
            length >>= 7;
            if length == 0 {
                bytes.push(seven);
                return bytes;
            }
            bytes.push(seven | 0x80);
        }
    }

    #[tokio::test]
    async fn a_message_survives_the_round_trip() {
        let sent = proto::Hello {
            protocol_version: 1,
            token_secret: Some("a-secret".to_owned()),
        };

        let mut frame = Vec::new();
        write_message(&mut frame, &sent).await.unwrap();

        let read: proto::Hello = read_message(&mut frame.as_slice()).await.unwrap();
        assert_eq!(read, sent);
    }

    #[tokio::test]
    async fn a_length_over_the_bound_is_refused_before_the_body_is_read() {
        // The reader holds the prefix and nothing else. A reader that
        // went looking for the body would fail reading it, with a
        // different error, so this asserts the order of the two
        // operations and not merely that the bound exists.
        let claimed = MAX_MESSAGE_BYTES + 1;
        let frame = prefix(claimed);

        let outcome = read_message::<_, proto::Hello>(&mut frame.as_slice()).await;

        match outcome {
            Err(Error::TooLarge { length }) => assert_eq!(length, claimed),
            other => panic!("a frame over the bound was not refused: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_length_at_the_bound_is_not_refused_for_its_size() {
        // The bound is inclusive, and this is where an off-by-one in
        // the comparison would show: the body is absent, so the read
        // gets as far as looking for it and fails there rather than
        // refusing the length.
        let frame = prefix(MAX_MESSAGE_BYTES);

        let outcome = read_message::<_, proto::Hello>(&mut frame.as_slice()).await;

        match outcome {
            Err(Error::Closed { .. }) => {}
            other => panic!("a frame at the bound was judged on its size: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_length_prefix_that_never_ends_is_refused_rather_than_accumulated() {
        // Six continuation bytes. A reader that kept accumulating
        // would be doing arithmetic on a number a peer chose.
        let frame = vec![0xff_u8; 6];

        let outcome = read_message::<_, proto::Hello>(&mut frame.as_slice()).await;

        match outcome {
            Err(Error::Framing { .. }) => {}
            other => panic!("an unending length prefix was not refused: {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_empty_stream_is_a_closed_connection_and_not_a_failure() {
        let outcome = read_message::<_, proto::Hello>(&mut [].as_slice()).await;

        match outcome {
            Err(Error::Closed { .. }) => {}
            other => panic!("a finished peer was not reported as closed: {other:?}"),
        }
    }
}
