//! Teneral: a daemon that answers a request from an identified client
//! on a known interface with the content configured for that client.
//!
//! This binary is a skeleton. It reports its version and exits. The
//! DHCP and DNS front ends, the binding store and the configuration
//! API are the subject of later work; see `docs/plans/`.

/// What a running teneral calls itself, so a process in a cluster can
/// be identified from its own output rather than from how it was
/// installed.
fn version_banner() -> String {
    format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
}

fn main() {
    println!("{}", version_banner());
}

#[cfg(test)]
mod tests {
    use super::version_banner;

    #[test]
    fn version_banner_names_the_binary_and_a_version() {
        let banner = version_banner();
        assert!(banner.starts_with("teneral "));
        assert_eq!(banner.split_whitespace().count(), 2);
    }
}
