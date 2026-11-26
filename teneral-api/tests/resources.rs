//! What the daemon answers about the store's resources.
//!
//! Every test here runs against two networks holding the same address,
//! because that is the property this project exists to get right and
//! one network proves nothing about it. The two differ in exactly the
//! places a cross-network answer would show -- the far end of a range,
//! the hostname on a binding, the holder of a lease -- so an answer
//! from the wrong network is a failed assertion rather than a thing a
//! reader has to notice.
//!
//! The harness is in `support`, which also holds the two lists of
//! requests the scope tests walk.

mod support;

use prost::Message;
use support::{
    ALPHA, BETA, BOUND, CLIENT, Daemon, FREE, LEASED, TempDirectory, address, binding_in,
    collection_count, every_interface_request, every_scoped_request, lease_in, mac, namespace,
    presenting, refusal_in, resource_in, wire_address, wire_cursor,
};
use teneral_api::proto::{self, RefusalCode, request, response};
use teneral_api::{named_network, resource};
use teneral_store::{BindingStore, Capability, EventKind, Selector, SelectorKind, TokenScope};

/// The network an answer says it was about, or a panic: every answer
/// carries it, so its absence is a failure rather than a case to
/// handle.
fn network_of(response: &proto::Response) -> String {
    response
        .network
        .clone()
        .unwrap_or_else(|| panic!("an answer named no network: {response:?}"))
}

/// The position an answer says it was true at.
fn position_of(response: &proto::Response) -> proto::Cursor {
    response
        .position
        .clone()
        .unwrap_or_else(|| panic!("an answer named no position: {response:?}"))
}

#[tokio::test]
async fn every_read_answers_from_the_network_it_was_asked_about() {
    let directory = TempDirectory::new("reads-per-network");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;
    daemon.lease(ALPHA, LEASED, "11:11:11:11:11:11").await;
    daemon.lease(BETA, LEASED, "22:22:22:22:22:22").await;

    let mut connection = daemon.connect(None).await;

    for network in [ALPHA, BETA] {
        // The network itself.
        let answer = connection
            .request(request::Kind::GetNetwork(proto::GetNetworkRequest {
                name: network.to_owned(),
            }))
            .await
            .expect("a network is answered");
        assert_eq!(network_of(&answer), network);
        match resource_in(&answer).state {
            Some(proto::resource::State::Network(found)) => assert_eq!(found.name, network),
            other => panic!("a network read answered with {other:?}"),
        }

        // The interface it has claimed, which is in a namespace of its
        // own: an answer from the other network would name the other
        // namespace.
        let answer = connection
            .request(request::Kind::GetAttachment(proto::GetAttachmentRequest {
                network: network.to_owned(),
                namespace: namespace(network),
                ifname: "eth0".to_owned(),
            }))
            .await
            .expect("an attachment is answered");
        assert_eq!(network_of(&answer), network);
        match resource_in(&answer).state {
            Some(proto::resource::State::Attachment(found)) => {
                assert_eq!(found.network, network);
                assert_eq!(found.namespace, namespace(network));
            }
            other => panic!("an attachment read answered with {other:?}"),
        }

        // The range, whose far end is what the two networks differ in.
        let answer = connection
            .request(request::Kind::GetAddressRange(
                proto::GetAddressRangeRequest {
                    network: network.to_owned(),
                    name: "pool".to_owned(),
                },
            ))
            .await
            .expect("a range is answered");
        assert_eq!(network_of(&answer), network);
        match resource_in(&answer).state {
            Some(proto::resource::State::AddressRange(found)) => {
                assert_eq!(found.network, network);
                assert_eq!(
                    found.last,
                    wire_address(if network == ALPHA {
                        "10.0.0.100"
                    } else {
                        "10.0.0.200"
                    }),
                    "a range read answered from the other network"
                );
            }
            other => panic!("a range read answered with {other:?}"),
        }

        // The binding both networks hold for one client, on one
        // address, which is the case the whole store exists to get
        // right: the same selector and the same address in two
        // networks are two unrelated configurations.
        let answer = connection
            .request(request::Kind::GetBinding(proto::GetBindingRequest {
                network: network.to_owned(),
                selector: Some(mac(CLIENT)),
            }))
            .await
            .expect("a binding is answered");
        assert_eq!(network_of(&answer), network);
        let found = binding_in(&answer);
        assert_eq!(found.network, network);
        assert_eq!(found.address, wire_address(BOUND));
        assert_eq!(found.hostname.as_deref(), Some(&*format!("{network}-host")));

        // The lease both networks hold on one address, for different
        // clients.
        let answer = connection
            .request(request::Kind::GetLease(proto::GetLeaseRequest {
                network: network.to_owned(),
                address: wire_address(LEASED),
            }))
            .await
            .expect("a lease is answered");
        assert_eq!(network_of(&answer), network);
        let found = lease_in(&answer);
        assert_eq!(found.network, network);
        assert_eq!(found.address, wire_address(LEASED));
        assert_eq!(
            found.holder.expect("a lease names its holder").value,
            if network == ALPHA {
                "11:11:11:11:11:11"
            } else {
                "22:22:22:22:22:22"
            },
            "a lease read answered with the other network's holder"
        );

        // The lookup, which answers with the selector it matched.
        let answer = connection
            .request(request::Kind::LookupBinding(proto::LookupBindingRequest {
                network: network.to_owned(),
                identity: Some(presenting(CLIENT)),
            }))
            .await
            .expect("a lookup is answered");
        assert_eq!(network_of(&answer), network);
        let found = binding_in(&answer);
        assert_eq!(found.hostname.as_deref(), Some(&*format!("{network}-host")));
        assert_eq!(
            found.selector.expect("a lookup names what matched").kind,
            proto::SelectorKind::Mac as i32
        );

        // And the digest, which two networks configured alike do not
        // share: they were configured at different positions.
        let answer = connection
            .request(request::Kind::Digest(proto::DigestRequest {
                network: network.to_owned(),
            }))
            .await
            .expect("a digest is answered");
        assert_eq!(network_of(&answer), network);
    }

    // The digests of the two networks differ, which is the property
    // that makes one usable as a mirror's check at all.
    let digest_of = |answer: &proto::Response| match answer.answer.as_ref() {
        Some(response::Answer::Digest(digest)) => digest.value.clone(),
        other => panic!("a digest read answered with {other:?}"),
    };
    let alpha = connection
        .request(request::Kind::Digest(proto::DigestRequest {
            network: ALPHA.to_owned(),
        }))
        .await
        .expect("a digest is answered");
    let beta = connection
        .request(request::Kind::Digest(proto::DigestRequest {
            network: BETA.to_owned(),
        }))
        .await
        .expect("a digest is answered");
    assert_ne!(digest_of(&alpha), digest_of(&beta));

    drop(connection);
    daemon.stop().await;
}

#[tokio::test]
async fn every_collection_streams_its_items_and_ends_with_a_marker() {
    let directory = TempDirectory::new("collections");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;
    daemon.lease(ALPHA, LEASED, "11:11:11:11:11:11").await;
    daemon.lease(BETA, LEASED, "22:22:22:22:22:22").await;

    let mut connection = daemon.connect(None).await;

    for network in [ALPHA, BETA] {
        for (what, kind, expected) in [
            (
                "attachments",
                request::Kind::ListAttachments(proto::ListAttachmentsRequest {
                    network: network.to_owned(),
                }),
                1,
            ),
            (
                "ranges",
                request::Kind::ListAddressRanges(proto::ListAddressRangesRequest {
                    network: network.to_owned(),
                }),
                1,
            ),
            (
                "bindings",
                request::Kind::ListBindings(proto::ListBindingsRequest {
                    network: network.to_owned(),
                }),
                1,
            ),
            (
                "leases",
                request::Kind::ListLeases(proto::ListLeasesRequest {
                    network: network.to_owned(),
                }),
                1,
            ),
        ] {
            let answers = connection
                .request_collection(kind)
                .await
                .expect("a collection is answered");

            // One message per item and then the marker, rather than
            // one message holding a list.
            assert_eq!(
                answers.len(),
                expected + 1,
                "the {what} of {network} arrived as {} messages",
                answers.len()
            );
            assert_eq!(collection_count(&answers), expected as u64);

            for answer in &answers {
                assert_eq!(
                    network_of(answer),
                    network,
                    "an item of the {what} of {network} named another network"
                );
            }
        }

        // The leases this client holds, which is a query of its own
        // rather than a filter over the listing above.
        let answers = connection
            .request_collection(request::Kind::LeasesHeldBy(proto::LeasesHeldByRequest {
                network: network.to_owned(),
                identity: Some(presenting(if network == ALPHA {
                    "11:11:11:11:11:11"
                } else {
                    "22:22:22:22:22:22"
                })),
            }))
            .await
            .expect("a collection is answered");

        assert_eq!(collection_count(&answers), 1);
        assert_eq!(lease_in(&answers[0]).network, network);

        // And the same query for the *other* network's client, which
        // holds nothing here however much it holds next door.
        let answers = connection
            .request_collection(request::Kind::LeasesHeldBy(proto::LeasesHeldByRequest {
                network: network.to_owned(),
                identity: Some(presenting(if network == ALPHA {
                    "22:22:22:22:22:22"
                } else {
                    "11:11:11:11:11:11"
                })),
            }))
            .await
            .expect("a collection is answered");

        assert_eq!(
            collection_count(&answers),
            0,
            "a client's leases in {network} included the other network's"
        );
    }

    // The listing of networks names each network in its own item and
    // names none on the marker, the collection being about the store.
    let answers = connection
        .request_collection(request::Kind::ListNetworks(proto::ListNetworksRequest {}))
        .await
        .expect("a collection is answered");

    assert_eq!(collection_count(&answers), 2);
    assert_eq!(network_of(&answers[0]), ALPHA);
    assert_eq!(network_of(&answers[1]), BETA);
    assert_eq!(
        answers[2].network, None,
        "the marker of a store-wide collection named a network"
    );

    drop(connection);
    daemon.stop().await;
}

#[tokio::test]
async fn every_write_is_confirmed_at_the_position_it_was_assigned_and_names_its_network() {
    let directory = TempDirectory::new("writes-per-network");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    let mut connection = daemon.connect(None).await;
    let mut positions = Vec::new();

    for network in [ALPHA, BETA] {
        // A range and a binding in each network, with the same name
        // and the same address in both, which is a conflict in neither.
        for kind in [
            request::Kind::CreateAddressRange(proto::CreateAddressRangeRequest {
                network: network.to_owned(),
                name: "second".to_owned(),
                first: wire_address("10.0.1.1"),
                last: wire_address("10.0.1.9"),
            }),
            request::Kind::CreateBinding(proto::CreateBindingRequest {
                network: network.to_owned(),
                selector: Some(mac("66:77:88:99:aa:bb")),
                address: wire_address("10.0.1.4"),
                hostname: Some(format!("{network}-second")),
                revoke_lease: None,
            }),
        ] {
            let answer = connection
                .request(kind)
                .await
                .expect("a write is confirmed");

            assert!(
                matches!(
                    answer.answer,
                    Some(response::Answer::Confirmation(proto::Confirmation {}))
                ),
                "a write answered with {:?}",
                answer.answer
            );
            assert_eq!(network_of(&answer), network);
            positions.push(position_of(&answer));
        }
    }

    // Each write was assigned its own position, in the store's one
    // order: a write confirmed at a position another write already had
    // would make a consumer's ordering against its stream meaningless.
    let sequences: Vec<u64> = positions.iter().map(|position| position.sequence).collect();
    let mut sorted = sequences.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sequences, sorted, "two writes shared a position");

    // A network of its own, created, configured and then removed at
    // the position the removal was assigned -- the one resource whose
    // own name is the network it is about.
    let answer = connection
        .request(request::Kind::CreateNetwork(proto::CreateNetworkRequest {
            name: "gamma".to_owned(),
        }))
        .await
        .expect("a write is confirmed");
    assert_eq!(network_of(&answer), "gamma");
    let created = position_of(&answer);

    let answer = connection
        .request(request::Kind::UpdateNetwork(proto::UpdateNetworkRequest {
            name: "gamma".to_owned(),
            expected: Some(created.clone()),
            lease_seconds: 7_200,
        }))
        .await
        .expect("a write is confirmed");
    assert_eq!(network_of(&answer), "gamma");
    let configured = position_of(&answer);
    assert!(configured.sequence > created.sequence);

    // Read back over the same wire, because a write confirmed at a
    // position says where it happened and not what it did.
    let answer = connection
        .request(request::Kind::GetNetwork(proto::GetNetworkRequest {
            name: "gamma".to_owned(),
        }))
        .await
        .expect("a network is answered");
    let found = match resource_in(&answer).state {
        Some(proto::resource::State::Network(network)) => network,
        other => panic!("that answer carried {other:?} rather than a network"),
    };
    assert_eq!(found.lease_seconds, 7_200);
    assert_eq!(found.cursor, Some(configured.clone()));

    let answer = connection
        .request(request::Kind::DeleteNetwork(proto::DeleteNetworkRequest {
            name: "gamma".to_owned(),
            expected: Some(configured.clone()),
        }))
        .await
        .expect("a write is confirmed");
    assert_eq!(network_of(&answer), "gamma");
    assert!(position_of(&answer).sequence > configured.sequence);

    // And a lease is taken away, which is the only write to one on
    // this wire.
    let lease = daemon.lease(ALPHA, LEASED, "11:11:11:11:11:11").await;
    let answer = connection
        .request(request::Kind::RevokeLease(proto::RevokeLeaseRequest {
            network: ALPHA.to_owned(),
            address: wire_address(LEASED),
            expected: Some(wire_cursor(&lease.cursor)),
        }))
        .await
        .expect("a write is confirmed");
    assert_eq!(network_of(&answer), ALPHA);

    drop(connection);
    daemon.stop().await;
}

#[tokio::test]
async fn a_credential_scoped_to_one_network_is_refused_on_the_other_for_every_operation() {
    let directory = TempDirectory::new("scoped-credential");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;
    daemon.lease(ALPHA, LEASED, "11:11:11:11:11:11").await;
    daemon.lease(BETA, LEASED, "22:22:22:22:22:22").await;

    // Everything except admin, inside one network. The capabilities
    // are deliberately wide: a credential that could not read at all
    // would be refused by a check that asked the wrong question.
    let minted = daemon
        .mint(
            "alpha-only",
            TokenScope::Networks(vec![ALPHA.to_owned()]),
            &[Capability::Read, Capability::Write, Capability::Watch],
        )
        .await;

    let mut connection = daemon.connect(Some(&minted.secret)).await;

    let requests = every_scoped_request(BETA);

    // The list is the whole of the operations that name a network,
    // which the function that decides that -- exhaustively, so that an
    // operation added to the schema has to be classified -- is what
    // this checks against.
    assert_eq!(
        requests.len(),
        25,
        "the operations that name a network have changed, and this list has not"
    );
    for kind in &requests {
        assert_eq!(
            named_network(kind),
            Some(BETA),
            "a request in this list does not name the network it is aimed at"
        );
    }

    for kind in requests {
        let answer = connection
            .request(kind.clone())
            .await
            .expect("a request is answered");
        let refusal = refusal_in(&answer);

        assert_eq!(
            refusal.code,
            RefusalCode::OutsideScope as i32,
            "an operation outside the credential's scope was not refused for it: {kind:?}"
        );
        assert!(
            !refusal.detail.contains(BETA),
            "a refusal named the network the caller may not see: {}",
            refusal.detail
        );
        assert_eq!(answer.network, None, "a refusal named a network");
        assert_eq!(answer.position, None, "a refusal carried a position");
    }

    // And the same credential is served for its own network, so what
    // the loop above proved is a scope and not a broken connection.
    let answer = connection
        .request(request::Kind::GetNetwork(proto::GetNetworkRequest {
            name: ALPHA.to_owned(),
        }))
        .await
        .expect("a request is answered");
    assert_eq!(network_of(&answer), ALPHA);

    // The listing of networks is filtered rather than refused: a
    // network's name is its identity, so naming one outside the scope
    // would say that it exists.
    let answers = connection
        .request_collection(request::Kind::ListNetworks(proto::ListNetworksRequest {}))
        .await
        .expect("a collection is answered");
    assert_eq!(collection_count(&answers), 1);
    assert_eq!(network_of(&answers[0]), ALPHA);

    drop(connection);
    daemon.stop().await;
}

#[tokio::test]
async fn the_questions_whose_answer_is_a_network_refuse_without_naming_it() {
    let directory = TempDirectory::new("resolved-scope");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    let minted = daemon
        .mint(
            "alpha-only",
            TokenScope::Networks(vec![ALPHA.to_owned()]),
            &[Capability::Read],
        )
        .await;

    let mut connection = daemon.connect(Some(&minted.secret)).await;

    // The three are asked about the other network's interface, and
    // about an interface nothing has claimed, and the two have to be
    // answered identically: the difference between "that belongs to a
    // network you may not see" and "nothing has claimed that" is
    // itself the leak.
    for (outside, unclaimed) in every_interface_request(BETA)
        .into_iter()
        .zip(every_interface_request(ALPHA).into_iter().map(unclaimed))
    {
        let answer = connection
            .request(outside.clone())
            .await
            .expect("a request is answered");
        let refused = refusal_in(&answer);

        assert_eq!(
            refused.code,
            RefusalCode::NoSuchResource as i32,
            "a question whose answer is a network was refused with the wrong code: {outside:?}"
        );
        assert!(
            !refused.detail.contains(BETA),
            "a refusal named the network that holds the interface: {}",
            refused.detail
        );
        assert_eq!(answer.network, None);

        let answer = connection
            .request(unclaimed.clone())
            .await
            .expect("a request is answered");
        let missing = refusal_in(&answer);

        assert_eq!(
            missing.code, refused.code,
            "an interface nothing has claimed and one the caller may not see were answered \
             differently: {unclaimed:?}"
        );
        assert_eq!(missing.resource_kind, refused.resource_kind);
    }

    // The caller's own interface is answered in full, so the refusals
    // above are about the scope and not about the operations.
    for kind in every_interface_request(ALPHA) {
        let answer = connection
            .request(kind.clone())
            .await
            .expect("a request is answered");

        assert_eq!(
            network_of(&answer),
            ALPHA,
            "a question about the caller's own network was answered with {answer:?}"
        );
    }

    drop(connection);
    daemon.stop().await;
}

#[tokio::test]
async fn the_owning_peer_is_told_that_no_network_has_claimed_an_interface() {
    let directory = TempDirectory::new("unclaimed-interface");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    let mut connection = daemon.connect(None).await;

    // A caller with no scope has no network to be kept from, so the
    // diagnosis an unclaimed interface is worth is the one it gets:
    // nothing has claimed this, which is a different fault from a
    // network that has claimed it and configured nothing.
    let answer = connection
        .request(request::Kind::Explain(proto::ExplainRequest {
            namespace: namespace(ALPHA),
            ifname: "eth9".to_owned(),
            identity: Some(presenting(CLIENT)),
        }))
        .await
        .expect("a request is answered");

    match answer.answer.as_ref() {
        Some(response::Answer::Explanation(explanation)) => {
            assert_eq!(explanation.network, None);
            assert_eq!(explanation.matched, None);
            assert_eq!(explanation.ifname, "eth9");
        }
        other => panic!("an unclaimed interface was explained as {other:?}"),
    }
    assert_eq!(answer.network, None);

    // And the explanation for an interface a network *has* claimed
    // names that network, the binding that would answer, and the
    // selectors the lookup would have consulted in order.
    let answer = connection
        .request(request::Kind::Explain(proto::ExplainRequest {
            namespace: namespace(BETA),
            ifname: "eth0".to_owned(),
            identity: Some(presenting(CLIENT)),
        }))
        .await
        .expect("a request is answered");

    match answer.answer.as_ref() {
        Some(response::Answer::Explanation(explanation)) => {
            assert_eq!(explanation.network.as_deref(), Some(BETA));
            assert_eq!(
                explanation
                    .matched
                    .as_ref()
                    .and_then(|binding| binding.hostname.as_deref()),
                Some("beta-host"),
                "an explanation answered from the other network"
            );
            assert_eq!(explanation.considered.len(), 1);
        }
        other => panic!("a claimed interface was explained as {other:?}"),
    }
    assert_eq!(network_of(&answer), BETA);

    drop(connection);
    daemon.stop().await;
}

#[tokio::test]
async fn a_lookup_on_an_interface_is_answered_in_the_network_the_caller_named() {
    let directory = TempDirectory::new("lookup-on-interface");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    let mut connection = daemon.connect(None).await;

    // The ordinary case: the caller names the network the interface
    // does belong to, and is answered from it. Both networks have a
    // binding for this client on this address, so an answer from the
    // other one is a hostname away rather than invisible.
    for network in [ALPHA, BETA] {
        let answer = connection
            .request(request::Kind::LookupBindingOnInterface(
                proto::LookupBindingOnInterfaceRequest {
                    network: network.to_owned(),
                    namespace: namespace(network),
                    ifname: "eth0".to_owned(),
                    identity: Some(presenting(CLIENT)),
                },
            ))
            .await
            .expect("a lookup is answered");

        assert_eq!(network_of(&answer), network);
        let found = binding_in(&answer);
        assert_eq!(found.network, network);
        assert_eq!(found.address, wire_address(BOUND));
        assert_eq!(found.hostname.as_deref(), Some(&*format!("{network}-host")));
    }

    drop(connection);
    daemon.stop().await;
}

#[tokio::test]
async fn a_lookup_naming_one_network_for_the_interface_of_another_is_refused_and_counted() {
    let directory = TempDirectory::new("cross-network-lookup");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    let mut connection = daemon.connect(None).await;

    assert_eq!(
        counter(&mut connection).await,
        0,
        "the counter that should always be zero did not start there"
    );

    // The one request on this wire that can make the store's
    // cross-network check fire on data rather than on a defect: the
    // network comes from the caller and the interface's network comes
    // from the store, and here they disagree.
    let answer = connection
        .request(request::Kind::LookupBindingOnInterface(
            proto::LookupBindingOnInterfaceRequest {
                network: ALPHA.to_owned(),
                namespace: namespace(BETA),
                ifname: "eth0".to_owned(),
                identity: Some(presenting(CLIENT)),
            },
        ))
        .await
        .expect("a request is answered");
    let refusal = refusal_in(&answer);

    // Refused rather than answered, and -- for this caller, whose
    // scope is every network -- named in full: it asked about one
    // network, the interface is in another, and there is no network
    // here it may not see. Somebody has to go and look at that.
    assert_eq!(refusal.code, RefusalCode::Internal as i32);
    assert!(
        refusal.detail.contains(ALPHA) && refusal.detail.contains(BETA),
        "a caller that may see both networks was not told which two disagreed: {}",
        refusal.detail
    );
    assert_eq!(refusal.held_by.as_deref(), Some(BETA));
    assert_eq!(answer.network, None);

    // And counted, which is the half nothing else in this suite can
    // move: a single non-zero reading of this is unambiguous and needs
    // no baseline to read.
    assert_eq!(
        counter(&mut connection).await,
        1,
        "a cross-network answer was refused without being counted"
    );

    // An interface no network has claimed is a different fault with a
    // different fix, so it is not found rather than counted.
    let answer = connection
        .request(request::Kind::LookupBindingOnInterface(
            proto::LookupBindingOnInterfaceRequest {
                network: ALPHA.to_owned(),
                namespace: namespace(ALPHA),
                ifname: "eth9".to_owned(),
                identity: Some(presenting(CLIENT)),
            },
        ))
        .await
        .expect("a request is answered");

    assert_eq!(refusal_in(&answer).code, RefusalCode::NoSuchResource as i32);
    assert_eq!(
        counter(&mut connection).await,
        1,
        "an interface nothing has claimed was counted as a cross-network answer"
    );

    drop(connection);
    daemon.stop().await;
}

#[tokio::test]
async fn a_scoped_caller_cannot_tell_a_cross_network_lookup_from_an_unclaimed_interface() {
    let directory = TempDirectory::new("cross-network-scoped");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    let minted = daemon
        .mint(
            "alpha-only",
            TokenScope::Networks(vec![ALPHA.to_owned()]),
            &[Capability::Read],
        )
        .await;

    // The owning peer's connection is only here to read the counter,
    // which is an operator's question.
    let mut operator = daemon.connect(None).await;
    let mut scoped = daemon.connect(Some(&minted.secret)).await;

    assert_eq!(counter(&mut operator).await, 0);

    // The caller names the network it may reach, so it passes the
    // scope check, and asks about an interface the other network
    // holds. It is inside its scope and still must learn nothing: a
    // code it could tell apart from the refusal below would say that
    // the interface exists in a network it may not see.
    let crossed = scoped
        .request(request::Kind::LookupBindingOnInterface(
            proto::LookupBindingOnInterfaceRequest {
                network: ALPHA.to_owned(),
                namespace: namespace(BETA),
                ifname: "eth0".to_owned(),
                identity: Some(presenting(CLIENT)),
            },
        ))
        .await
        .expect("a request is answered");
    let crossed = refusal_in(&crossed);

    let missing = scoped
        .request(request::Kind::LookupBindingOnInterface(
            proto::LookupBindingOnInterfaceRequest {
                network: ALPHA.to_owned(),
                namespace: namespace(ALPHA),
                ifname: "eth9".to_owned(),
                identity: Some(presenting(CLIENT)),
            },
        ))
        .await
        .expect("a request is answered");
    let missing = refusal_in(&missing);

    assert_eq!(crossed.code, RefusalCode::NoSuchResource as i32);
    assert_eq!(crossed.code, missing.code);
    assert_eq!(crossed.resource_kind, missing.resource_kind);
    assert!(
        !crossed.detail.contains(ALPHA) && !crossed.detail.contains(BETA),
        "a scoped caller was told which networks disagreed: {}",
        crossed.detail
    );
    assert_eq!(crossed.held_by, None);

    // And the counter moved anyway, for the first of the two and not
    // the second. It is the store's, it fires before the refusal is
    // shaped, and an alarm a narrow scope could switch off would be
    // worth nothing.
    assert_eq!(
        counter(&mut operator).await,
        1,
        "a cross-network answer went uncounted because the caller had a scope"
    );

    drop(scoped);
    drop(operator);
    daemon.stop().await;
}

#[tokio::test]
async fn a_binding_on_a_leased_address_is_refused_naming_the_holder() {
    let directory = TempDirectory::new("conflict-names-holder");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    // The same address leased in both networks, to different clients.
    let alpha = daemon.lease(ALPHA, FREE, "11:11:11:11:11:11").await;
    let beta = daemon.lease(BETA, FREE, "22:22:22:22:22:22").await;

    let mut connection = daemon.connect(None).await;

    let answer = connection
        .request(request::Kind::CreateBinding(proto::CreateBindingRequest {
            network: ALPHA.to_owned(),
            selector: Some(mac("99:99:99:99:99:99")),
            address: wire_address(FREE),
            hostname: None,
            revoke_lease: None,
        }))
        .await
        .expect("a request is answered");
    let refusal = refusal_in(&answer);

    assert_eq!(refusal.code, RefusalCode::Conflict as i32);
    let held_by = refusal
        .held_by
        .expect("a conflict names what is in the way");
    assert!(
        held_by.contains("11:11:11:11:11:11"),
        "a conflict did not name the holder: {held_by}"
    );
    assert!(
        held_by.contains(&alpha.lease_id.to_string()),
        "a conflict did not name the lease the instruction has to carry: {held_by}"
    );
    assert!(
        !held_by.contains("22:22:22:22:22:22"),
        "a conflict named the other network's holder: {held_by}"
    );

    // The same write, carrying the holder's id, takes the lease and
    // writes the binding in one operation.
    let answer = connection
        .request(request::Kind::CreateBinding(proto::CreateBindingRequest {
            network: ALPHA.to_owned(),
            selector: Some(mac("99:99:99:99:99:99")),
            address: wire_address(FREE),
            hostname: Some("taken-over".to_owned()),
            revoke_lease: Some(wire_cursor(&alpha.lease_id)),
        }))
        .await
        .expect("a request is answered");

    assert!(
        matches!(
            answer.answer,
            Some(response::Answer::Confirmation(proto::Confirmation {}))
        ),
        "the revoking write was answered with {:?}",
        answer.answer
    );
    assert_eq!(network_of(&answer), ALPHA);

    // Both halves happened, which is what makes it one operation:
    // the binding is there and the lease is gone. There is no state in
    // which the address was taken and the binding was not written.
    let answer = connection
        .request(request::Kind::GetBinding(proto::GetBindingRequest {
            network: ALPHA.to_owned(),
            selector: Some(mac("99:99:99:99:99:99")),
        }))
        .await
        .expect("a request is answered");
    assert_eq!(binding_in(&answer).address, wire_address(FREE));

    let answer = connection
        .request(request::Kind::GetLease(proto::GetLeaseRequest {
            network: ALPHA.to_owned(),
            address: wire_address(FREE),
        }))
        .await
        .expect("a request is answered");
    assert_eq!(
        refusal_in(&answer).code,
        RefusalCode::NoSuchResource as i32,
        "the lease the write took is still there"
    );

    // And the other network's lease on the same address was not
    // touched, because it never was the same lease.
    let answer = connection
        .request(request::Kind::GetLease(proto::GetLeaseRequest {
            network: BETA.to_owned(),
            address: wire_address(FREE),
        }))
        .await
        .expect("a request is answered");
    assert_eq!(
        lease_in(&answer)
            .lease_id
            .expect("a lease carries its id")
            .sequence,
        beta.lease_id.sequence(),
        "a revocation in one network reached the other's lease"
    );

    drop(connection);
    daemon.stop().await;
}

#[tokio::test]
async fn a_revoking_write_that_names_another_lease_takes_nothing() {
    let directory = TempDirectory::new("revoke-wrong-lease");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    let alpha = daemon.lease(ALPHA, FREE, "11:11:11:11:11:11").await;
    let beta = daemon.lease(BETA, FREE, "22:22:22:22:22:22").await;

    let mut connection = daemon.connect(None).await;

    // The instruction carries the *other* network's lease id, which is
    // an id this network's lease has never had. Nothing is taken and
    // nothing is written: the two are one transaction, so a refused
    // binding rolls the revocation back with it.
    let answer = connection
        .request(request::Kind::CreateBinding(proto::CreateBindingRequest {
            network: ALPHA.to_owned(),
            selector: Some(mac("99:99:99:99:99:99")),
            address: wire_address(FREE),
            hostname: None,
            revoke_lease: Some(wire_cursor(&beta.lease_id)),
        }))
        .await
        .expect("a request is answered");

    let refusal = refusal_in(&answer);
    assert!(
        refusal.code == RefusalCode::Changed as i32 || refusal.code == RefusalCode::Conflict as i32,
        "a revocation of a lease this network does not hold was answered with {refusal:?}"
    );

    let answer = connection
        .request(request::Kind::GetLease(proto::GetLeaseRequest {
            network: ALPHA.to_owned(),
            address: wire_address(FREE),
        }))
        .await
        .expect("a request is answered");
    assert_eq!(
        lease_in(&answer)
            .lease_id
            .expect("a lease carries its id")
            .sequence,
        alpha.lease_id.sequence(),
        "a refused write took the lease anyway"
    );

    let answer = connection
        .request(request::Kind::GetBinding(proto::GetBindingRequest {
            network: ALPHA.to_owned(),
            selector: Some(mac("99:99:99:99:99:99")),
        }))
        .await
        .expect("a request is answered");
    assert_eq!(
        refusal_in(&answer).code,
        RefusalCode::NoSuchResource as i32,
        "a refused write wrote the binding anyway"
    );

    drop(connection);
    daemon.stop().await;
}

#[tokio::test]
async fn a_write_from_a_stale_position_is_refused_and_says_where_the_resource_is() {
    let directory = TempDirectory::new("stale-position");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    let mut connection = daemon.connect(None).await;

    let answer = connection
        .request(request::Kind::GetAddressRange(
            proto::GetAddressRangeRequest {
                network: ALPHA.to_owned(),
                name: "pool".to_owned(),
            },
        ))
        .await
        .expect("a range is answered");
    let stale = match resource_in(&answer).state {
        Some(proto::resource::State::AddressRange(range)) => {
            range.cursor.expect("a range carries its position")
        }
        other => panic!("a range read answered with {other:?}"),
    };

    // One resize, from the position the read reported, which is the
    // compare-and-set succeeding.
    let answer = connection
        .request(request::Kind::UpdateAddressRange(
            proto::UpdateAddressRangeRequest {
                network: ALPHA.to_owned(),
                name: "pool".to_owned(),
                expected: Some(stale.clone()),
                first: wire_address("10.0.0.1"),
                last: wire_address("10.0.0.60"),
            },
        ))
        .await
        .expect("a write is confirmed");
    let moved = position_of(&answer);

    // And a second from the same read, which is the case the
    // compare-and-set exists for: two writers resizing one pool from
    // one stale read is how a network comes to answer from addresses
    // something else was told it owned.
    let answer = connection
        .request(request::Kind::UpdateAddressRange(
            proto::UpdateAddressRangeRequest {
                network: ALPHA.to_owned(),
                name: "pool".to_owned(),
                expected: Some(stale.clone()),
                first: wire_address("10.0.0.1"),
                last: wire_address("10.0.0.70"),
            },
        ))
        .await
        .expect("a request is answered");
    let refusal = refusal_in(&answer);

    assert_eq!(refusal.code, RefusalCode::Changed as i32);
    assert_eq!(
        refusal.expected_position.as_deref(),
        Some(&*format!("{}:{}", stale.epoch, stale.sequence)),
        "the refusal did not say what the caller expected"
    );
    assert_eq!(
        refusal.found_position.as_deref(),
        Some(&*format!("{}:{}", moved.epoch, moved.sequence)),
        "the refusal did not say where the resource actually is"
    );

    // The other network's range was not moved by either write.
    let answer = connection
        .request(request::Kind::GetAddressRange(
            proto::GetAddressRangeRequest {
                network: BETA.to_owned(),
                name: "pool".to_owned(),
            },
        ))
        .await
        .expect("a range is answered");
    match resource_in(&answer).state {
        Some(proto::resource::State::AddressRange(range)) => {
            assert_eq!(range.last, wire_address("10.0.0.200"));
        }
        other => panic!("a range read answered with {other:?}"),
    }

    drop(connection);
    daemon.stop().await;
}

#[tokio::test]
async fn the_message_a_read_answers_with_is_the_message_an_event_carries() {
    let directory = TempDirectory::new("one-converter");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;
    daemon.lease(ALPHA, LEASED, "11:11:11:11:11:11").await;
    daemon.lease(BETA, LEASED, "22:22:22:22:22:22").await;

    // Read through the store rather than over the socket, because what
    // is being compared is the two ways a resource reaches the wire: a
    // typed read and the JSON an event carries. Two spellings of one
    // resource is the defect that arrangement produces, and it stays
    // invisible until a consumer applies a streamed event over a read.
    let view = daemon.store().read().await.expect("a view of the store");
    let snapshot = view.snapshot(ALPHA).await.expect("a snapshot of a network");

    // The instant the lease's expiry is evaluated at. The fixtures
    // take an hour, so both sides evaluate to "held" whenever this
    // runs; the point of the test is the other eight fields.
    let now = 0;

    let mut compared = 0;
    for event in &snapshot.events {
        let state = event.state.as_deref().expect("a snapshot event has state");
        let from_event = resource::of_event(event.kind, state, &wire_cursor(&event.cursor), now)
            .expect("an event this build wrote parses in this build");

        let from_read = match event.kind {
            EventKind::Network => {
                let found = view
                    .network(ALPHA)
                    .await
                    .expect("a network")
                    .expect("the network the snapshot named");
                resource::of_network(&found)
            }
            EventKind::Attachment => {
                let found = view
                    .attachment(ALPHA, &namespace(ALPHA), "eth0")
                    .await
                    .expect("an attachment")
                    .expect("the attachment the snapshot named");
                resource::of_attachment(&found)
            }
            EventKind::AddressRange => {
                let found = view
                    .address_range(ALPHA, "pool")
                    .await
                    .expect("a range")
                    .expect("the range the snapshot named");
                resource::of_address_range(&found)
            }
            EventKind::Binding => {
                let selector = Selector::new(SelectorKind::Mac, CLIENT).expect("a selector");
                let found = view
                    .binding(ALPHA, &selector)
                    .await
                    .expect("a binding")
                    .expect("the binding the snapshot named");
                resource::of_binding(&found)
            }
            EventKind::Lease => {
                let found = view
                    .lease(ALPHA, address(LEASED))
                    .await
                    .expect("a lease")
                    .expect("the lease the snapshot named");
                resource::of_lease(&found)
            }
        };

        // Byte for byte after encoding, because that is what a
        // consumer compares: a field set on one side and left at its
        // default on the other is a difference the encoding shows and
        // a field-by-field comparison written by hand would miss.
        assert_eq!(
            from_event.encode_to_vec(),
            from_read.encode_to_vec(),
            "a {} reaches the wire spelled two ways: {from_event:?} and {from_read:?}",
            event.kind.as_str()
        );

        compared += 1;
    }

    // Every kind the store holds, so that a kind added to the store
    // and not to the converters is a count that moved.
    assert_eq!(compared, EventKind::ALL.len());

    drop(view);
    daemon.stop().await;
}

#[tokio::test]
async fn the_diagnostics_say_what_the_daemon_is_and_the_counter_is_zero() {
    let directory = TempDirectory::new("diagnostics");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    let mut connection = daemon.connect(None).await;

    let answer = connection
        .request(request::Kind::Diagnostics(proto::DiagnosticsRequest {}))
        .await
        .expect("a request is answered");

    match answer.answer.as_ref() {
        Some(response::Answer::Diagnostics(diagnostics)) => {
            assert_eq!(diagnostics.schema_version, teneral_store::SCHEMA_VERSION);
            assert_eq!(diagnostics.protocol_version, teneral_api::PROTOCOL_VERSION);
            assert_eq!(diagnostics.max_live_views, 64);

            // The counter that should always be zero. A single
            // non-zero reading is unambiguous, which is why it is
            // worth asserting on a store two networks have been
            // reading and writing.
            assert_eq!(diagnostics.cross_network_answers, 0);

            // Nobody has replaced this store's epoch, and the
            // recorded word for that is a fact rather than an absence.
            assert!(!diagnostics.epoch_minted_by.is_empty());
            assert_eq!(diagnostics.epoch_minted_at, None);
        }
        other => panic!("the diagnostics answered with {other:?}"),
    }

    // Store-wide, so the envelope names no network, and carries the
    // position the store had reached.
    assert_eq!(answer.network, None);
    assert!(answer.position.is_some());

    drop(connection);
    daemon.stop().await;
}

#[tokio::test]
async fn the_operator_mints_credentials_and_an_epoch_over_this_wire() {
    let directory = TempDirectory::new("operator-operations");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    let mut connection = daemon.connect(None).await;

    // The first credential is minted by the peer the socket's
    // permissions already trust, which is what makes a first
    // credential possible without a secret on disk.
    let answer = connection
        .request(request::Kind::MintToken(proto::MintTokenRequest {
            name: "alpha-reader".to_owned(),
            scope: Some(proto::Scope {
                reach: Some(proto::scope::Reach::Networks(proto::NetworkNames {
                    names: vec![ALPHA.to_owned()],
                })),
            }),
            capabilities: vec![proto::Capability::Read as i32],
        }))
        .await
        .expect("a request is answered");

    let minted = match answer.answer.as_ref() {
        Some(response::Answer::MintedToken(minted)) => minted.clone(),
        other => panic!("a mint answered with {other:?}"),
    };
    let token = minted.token.expect("a mint answers with the credential");
    assert!(!minted.secret.is_empty());
    assert_eq!(token.capabilities, vec![proto::Capability::Read as i32]);

    // It is a credential that works, and only inside its own network.
    let mut scoped = daemon.connect(Some(&minted.secret)).await;
    let answer = scoped
        .request(request::Kind::GetNetwork(proto::GetNetworkRequest {
            name: BETA.to_owned(),
        }))
        .await
        .expect("a request is answered");
    assert_eq!(
        refusal_in(&answer).code,
        RefusalCode::OutsideScope as i32,
        "a credential scoped to one network reached the other"
    );
    drop(scoped);

    // The listing names it, and the removal takes it away.
    let answers = connection
        .request_collection(request::Kind::ListTokens(proto::ListTokensRequest {}))
        .await
        .expect("a collection is answered");
    assert_eq!(collection_count(&answers), 1);

    let answer = connection
        .request(request::Kind::DeleteToken(proto::DeleteTokenRequest {
            id: token.id.clone(),
        }))
        .await
        .expect("a request is answered");
    assert!(matches!(
        answer.answer,
        Some(response::Answer::Confirmation(proto::Confirmation {}))
    ));

    let answers = connection
        .request_collection(request::Kind::ListTokens(proto::ListTokensRequest {}))
        .await
        .expect("a collection is answered");
    assert_eq!(collection_count(&answers), 0);

    // And the epoch, which is the most consequential write an operator
    // can make: every position any consumer remembers stops matching.
    let before = position_of(
        &connection
            .request(request::Kind::GetNetwork(proto::GetNetworkRequest {
                name: ALPHA.to_owned(),
            }))
            .await
            .expect("a network is answered"),
    );

    let answer = connection
        .request(request::Kind::MintEpoch(proto::MintEpochRequest {}))
        .await
        .expect("a request is answered");
    let minted = match answer.answer.as_ref() {
        Some(response::Answer::MintedEpoch(minted)) => minted
            .position
            .clone()
            .expect("a mint says where the new order starts"),
        other => panic!("minting an epoch answered with {other:?}"),
    };

    assert_ne!(
        minted.epoch, before.epoch,
        "minting an epoch left the store in the one it was in"
    );
    assert!(
        minted.sequence >= before.sequence,
        "the sequence counter restarted, so a position from the old order can be issued again"
    );
    assert_eq!(answer.network, None);

    drop(connection);
    daemon.stop().await;
}

/// The same question, asked about an interface nothing has claimed.
///
/// Built from a request aimed at a network's own namespace by moving
/// it to an interface name no network has attached, so that the two
/// requests differ in nothing else.
fn unclaimed(kind: request::Kind) -> request::Kind {
    match kind {
        request::Kind::InterfaceOwner(mut request) => {
            request.ifname = "eth9".to_owned();
            request::Kind::InterfaceOwner(request)
        }
        request::Kind::Explain(mut request) => {
            request.ifname = "eth9".to_owned();
            request::Kind::Explain(request)
        }
        other => panic!("that is not a question whose answer is a network: {other:?}"),
    }
}

/// The counter that should always be zero, read over the wire.
///
/// Through the diagnostics rather than off the store, because what is
/// being checked is that a request made on this wire moved it: a test
/// that read the store's own counter would pass even if no request
/// could reach the check.
async fn counter(connection: &mut teneral_api::client::Connection) -> u64 {
    let answer = connection
        .request(request::Kind::Diagnostics(proto::DiagnosticsRequest {}))
        .await
        .expect("the diagnostics are answered");

    match answer.answer.as_ref() {
        Some(response::Answer::Diagnostics(diagnostics)) => diagnostics.cross_network_answers,
        other => panic!("the diagnostics answered with {other:?}"),
    }
}
