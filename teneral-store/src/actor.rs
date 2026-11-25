//! Who made a write.

use crate::tokens::TokenId;

/// The party a write is recorded against.
///
/// Three things can write to this store and they are not the same
/// kind of thing. A consumer presents a token and is that token for
/// the length of its connection. A local process connecting as the
/// uid the daemon runs as presents nothing, its credential being read
/// from the kernel, and is how the first token comes to exist at all.
/// And the daemon writes on its own behalf -- a sweep, a lease the
/// datapath recorded -- with no request behind it to attribute to.
///
/// It is a parameter of every mutating method rather than state the
/// store holds, because one store serves every connection: an actor
/// kept on the store would be the actor of whichever request happened
/// to set it last, which is the attribution bug this exists to
/// prevent rather than a shortcut past it.
///
/// **Rendered into one `TEXT` value and never parsed back.** Nothing
/// in this crate or above it reads an actor as structure; it is read
/// by a person, in a listing or a log, asking who moved something. So
/// there is no normalised table, no join, and no parser whose
/// disagreement with [`Actor::as_recorded`] could make an old row
/// unreadable. A token that has since been deleted still names itself
/// on every row it wrote, which a foreign key would have had to
/// either forbid or cascade away.
///
/// Every rendering below is safe to put in a column without escaping
/// by construction rather than by checking: a token id is hex the
/// engine generated, a uid is a number, and a daemon part is a
/// literal in this tree. None of them can contain a newline or the
/// separator, so the value an operator reads is the value that was
/// written.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Actor {
    /// A consumer that presented this token.
    Token(TokenId),
    /// A local process connecting as the uid the daemon runs as,
    /// which is the only caller that may act without a token.
    Peer(u32),
    /// The daemon itself, in the named part of it that wrote. The
    /// name is a literal rather than a runtime string because the
    /// parts of the daemon are known when it is built, and an actor
    /// assembled from configuration would be an attribution an
    /// operator could choose.
    Daemon(&'static str),
}

impl Actor {
    /// The value the actor is recorded as.
    ///
    /// The kind leads it, so a reader can tell a token id from a uid
    /// without knowing how either is formatted, and so that a value
    /// written before this store recorded actors at all -- which
    /// carries no kind and no separator -- cannot be mistaken for one
    /// of these.
    pub fn as_recorded(&self) -> String {
        match self {
            Actor::Token(id) => format!("token:{}", id.as_str()),
            Actor::Peer(uid) => format!("peer:{uid}"),
            Actor::Daemon(part) => format!("daemon:{part}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Actor;
    use crate::tokens::TokenId;

    #[test]
    fn each_kind_of_actor_renders_to_a_value_that_says_which_kind_it_is() {
        // The three renderings have to be distinguishable from each
        // other and from the word a row written before attribution
        // existed carries, because all four land in one column and a
        // person reads them together.
        assert_eq!(
            Actor::Token(TokenId::for_test("abcdef")).as_recorded(),
            "token:abcdef"
        );
        assert_eq!(Actor::Peer(0).as_recorded(), "peer:0");
        assert_eq!(Actor::Daemon("sweeper").as_recorded(), "daemon:sweeper");

        for actor in [
            Actor::Token(TokenId::for_test("abcdef")),
            Actor::Peer(0),
            Actor::Daemon("sweeper"),
        ] {
            assert_ne!(actor.as_recorded(), "unrecorded");
            assert!(actor.as_recorded().contains(':'));
        }
    }
}
