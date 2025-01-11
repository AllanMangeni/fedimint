//! Implements a connection manager for communication with other federation
//! members
//!
//! The main interface is [`fedimint_core::net::peers::IP2PConnections`] and
//! its main implementation is [`ReconnectP2PConnections`], see these for
//! details.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::time::Duration;

use async_channel::{bounded, Receiver, Sender};
use async_trait::async_trait;
use fedimint_api_client::api::P2PConnectionStatus;
use fedimint_core::net::peers::{IP2PConnections, Recipient};
use fedimint_core::task::TaskGroup;
use fedimint_core::util::backoff_util::{api_networking_backoff, FibonacciBackoff};
use fedimint_core::util::FmtCompactAnyhow;
use fedimint_core::PeerId;
use fedimint_logging::{LOG_CONSENSUS, LOG_NET_PEER};
use futures::future::select_all;
use futures::{FutureExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::time::sleep;
use tracing::{info, info_span, warn, Instrument};

use crate::metrics::{PEER_CONNECT_COUNT, PEER_DISCONNECT_COUNT, PEER_MESSAGES_COUNT};
use crate::net::connect::DynConnector;
use crate::net::framed::DynFramedTransport;

#[derive(Clone)]
pub struct ReconnectP2PConnections<M> {
    connections: BTreeMap<PeerId, P2PConnection<M>>,
}

/// Internal message type for [`ReconnectP2PConnections`], just public because
/// it appears in the public interface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum P2PMessage<M> {
    Message(M),
    Ping,
}

impl<M: Send + 'static> ReconnectP2PConnections<M> {
    pub async fn new(
        identity: PeerId,
        connector: DynConnector<P2PMessage<M>>,
        task_group: &TaskGroup,
        mut status_channels: Option<BTreeMap<PeerId, watch::Sender<P2PConnectionStatus>>>,
    ) -> Self {
        let mut connection_senders = BTreeMap::new();
        let mut connections = BTreeMap::new();

        for peer_id in connector.peers() {
            assert_ne!(peer_id, identity);

            let (connection_sender, connection_receiver) = bounded(4);

            let connection = P2PConnection::new(
                identity,
                peer_id,
                connector.clone(),
                connection_receiver,
                status_channels.as_mut().map(|channels| {
                    channels
                        .remove(&peer_id)
                        .expect("No p2p status sender for peer {peer}")
                }),
                task_group,
            );

            connection_senders.insert(peer_id, connection_sender);
            connections.insert(peer_id, connection);
        }

        let mut listener = connector.listen().await.expect("Could not bind to port");

        task_group.spawn_cancellable("handle-incoming-p2p-connections", async move {
            info!(target: LOG_NET_PEER, "Shutting down listening task for p2p connections");

            loop {
                match listener.next().await.expect("Listener closed") {
                    Ok((peer, connection)) => {
                        if connection_senders
                            .get_mut(&peer)
                            .expect("Authenticating connectors dont return unknown peers")
                            .send(connection)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    },
                    Err(err) => {
                        warn!(target: LOG_NET_PEER, our_id = %identity, err = %err.fmt_compact_anyhow(), "Error while opening incoming connection");
                    }
                }
            }

            info!(target: LOG_NET_PEER, "Shutting down listening task for p2p connections");
        });

        ReconnectP2PConnections { connections }
    }
}

#[async_trait]
impl<M: Clone + Send + 'static> IP2PConnections<M> for ReconnectP2PConnections<M> {
    async fn send(&self, recipient: Recipient, message: M) {
        match recipient {
            Recipient::Everyone => {
                for connection in self.connections.values() {
                    connection.send(message.clone()).await;
                }
            }
            Recipient::Peer(peer) => {
                if let Some(connection) = self.connections.get(&peer) {
                    connection.send(message).await;
                } else {
                    warn!(target: LOG_NET_PEER, "No connection for peer {peer}");
                }
            }
        }
    }

    fn try_send(&self, recipient: Recipient, message: M) {
        match recipient {
            Recipient::Everyone => {
                for connection in self.connections.values() {
                    connection.try_send(message.clone());
                }
            }
            Recipient::Peer(peer) => {
                if let Some(connection) = self.connections.get(&peer) {
                    connection.try_send(message);
                } else {
                    warn!(target: LOG_NET_PEER, "No connection for peer {peer}");
                }
            }
        }
    }

    async fn receive(&self) -> Option<(PeerId, M)> {
        select_all(self.connections.iter().map(|(&peer, connection)| {
            Box::pin(connection.receive().map(move |m| m.map(|m| (peer, m))))
        }))
        .await
        .0
    }
}

#[derive(Clone)]
struct P2PConnection<M> {
    outgoing: Sender<M>,
    incoming: Receiver<M>,
}

impl<M: Send + 'static> P2PConnection<M> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        our_id: PeerId,
        peer_id: PeerId,
        connector: DynConnector<P2PMessage<M>>,
        incoming_connections: Receiver<DynFramedTransport<P2PMessage<M>>>,
        status_channel: Option<watch::Sender<P2PConnectionStatus>>,
        task_group: &TaskGroup,
    ) -> P2PConnection<M> {
        let (outgoing_sender, outgoing_receiver) = bounded(1024);
        let (incoming_sender, incoming_receiver) = bounded(1024);

        task_group.spawn_cancellable(
            format!("io-state-machine-{peer_id}"),
            async move {
                info!(target: LOG_NET_PEER, "Starting peer connection state machine");

                let mut state_machine = P2PConnectionStateMachine {
                    common: P2PConnectionSMCommon {
                        incoming_sender,
                        outgoing_receiver,
                        our_id_str: our_id.to_string(),
                        our_id,
                        peer_id_str: peer_id.to_string(),
                        peer_id,
                        connector,
                        incoming_connections,
                        status_channel,
                    },
                    state: P2PConnectionSMState::Disconnected(api_networking_backoff()),
                };

                while let Some(sm) = state_machine.state_transition().await {
                    state_machine = sm;
                }

                info!(target: LOG_NET_PEER, "Shutting down peer connection state machine");
            }
            .instrument(info_span!("io-state-machine", ?peer_id)),
        );

        P2PConnection {
            outgoing: outgoing_sender,
            incoming: incoming_receiver,
        }
    }

    async fn send(&self, message: M) {
        self.outgoing.send(message).await.ok();
    }

    fn try_send(&self, message: M) {
        self.outgoing.try_send(message).ok();
    }

    async fn receive(&self) -> Option<M> {
        self.incoming.recv().await.ok()
    }
}

struct P2PConnectionStateMachine<M> {
    state: P2PConnectionSMState<M>,
    common: P2PConnectionSMCommon<M>,
}

struct P2PConnectionSMCommon<M> {
    incoming_sender: async_channel::Sender<M>,
    outgoing_receiver: async_channel::Receiver<M>,
    our_id: PeerId,
    our_id_str: String,
    peer_id: PeerId,
    peer_id_str: String,
    connector: DynConnector<P2PMessage<M>>,
    incoming_connections: Receiver<DynFramedTransport<P2PMessage<M>>>,
    status_channel: Option<watch::Sender<P2PConnectionStatus>>,
}

enum P2PConnectionSMState<M> {
    Disconnected(FibonacciBackoff),
    Connected(DynFramedTransport<P2PMessage<M>>),
}

impl<M: Send + 'static> P2PConnectionStateMachine<M> {
    async fn state_transition(mut self) -> Option<Self> {
        match self.state {
            P2PConnectionSMState::Disconnected(disconnected) => {
                if let Some(channel) = &self.common.status_channel {
                    channel.send(P2PConnectionStatus::Connected).ok();
                }

                self.common.transition_disconnected(disconnected).await
            }
            P2PConnectionSMState::Connected(connected) => {
                if let Some(channel) = &self.common.status_channel {
                    channel.send(P2PConnectionStatus::Disconnected).ok();
                }

                self.common.transition_connected(connected).await
            }
        }
        .map(|state| P2PConnectionStateMachine {
            common: self.common,
            state,
        })
    }
}

impl<M: Send + 'static> P2PConnectionSMCommon<M> {
    async fn transition_connected(
        &mut self,
        mut connection: DynFramedTransport<P2PMessage<M>>,
    ) -> Option<P2PConnectionSMState<M>> {
        tokio::select! {
            message = self.outgoing_receiver.recv() => {
                Some(self.send_message(connection, P2PMessage::Message(message.ok()?)).await)
            },
            connection = self.incoming_connections.recv() => {
                Some(self.connect(connection.ok()?).await)
            },
            message = connection.receive() => {
                match message {
                    Ok(message) => {
                        if let P2PMessage::Message(message) = message {
                            PEER_MESSAGES_COUNT.with_label_values(&[&self.our_id_str, &self.peer_id_str, "incoming"]).inc();

                            self.incoming_sender.send(message).await.ok()?;
                        }

                    },
                    Err(e) => return Some(self.disconnect(e)),
                };

                Some(P2PConnectionSMState::Connected(connection))
            },
            () = sleep(Duration::from_secs(10)) => {
                Some(self.send_message(connection, P2PMessage::Ping).await)
            },
        }
    }

    async fn connect(
        &mut self,
        mut connection: DynFramedTransport<P2PMessage<M>>,
    ) -> P2PConnectionSMState<M> {
        info!(target: LOG_NET_PEER, "Connected to peer");

        match connection.send(P2PMessage::Ping).await {
            Ok(()) => P2PConnectionSMState::Connected(connection),
            Err(e) => self.disconnect(e),
        }
    }

    fn disconnect(&self, error: anyhow::Error) -> P2PConnectionSMState<M> {
        info!(target: LOG_NET_PEER, "Disconnected from peer: {}",  error);

        PEER_DISCONNECT_COUNT
            .with_label_values(&[&self.our_id_str, &self.peer_id_str])
            .inc();

        P2PConnectionSMState::Disconnected(api_networking_backoff())
    }

    async fn send_message(
        &mut self,
        mut connection: DynFramedTransport<P2PMessage<M>>,
        peer_message: P2PMessage<M>,
    ) -> P2PConnectionSMState<M> {
        PEER_MESSAGES_COUNT
            .with_label_values(&[&self.our_id_str, &self.peer_id_str, "outgoing"])
            .inc();

        if let Err(e) = connection.send(peer_message).await {
            return self.disconnect(e);
        }

        P2PConnectionSMState::Connected(connection)
    }

    async fn transition_disconnected(
        &mut self,
        mut backoff: FibonacciBackoff,
    ) -> Option<P2PConnectionSMState<M>> {
        tokio::select! {
            connection = self.incoming_connections.recv() => {
                PEER_CONNECT_COUNT.with_label_values(&[&self.our_id_str, &self.peer_id_str, "incoming"]).inc();

                Some(self.connect(connection.ok()?).await)
            },
            () = sleep(backoff.next().expect("Unlimited retries")), if self.our_id < self.peer_id => {
                // to prevent "reconnection ping-pongs", only the side with lower PeerId is responsible for reconnecting

                info!(target: LOG_NET_PEER, "Attempting to reconnect to peer");

                match  self.connector.connect(self.peer_id).await {
                    Ok(connection) => {
                        PEER_CONNECT_COUNT
                            .with_label_values(&[&self.our_id_str, &self.peer_id_str, "outgoing"])
                            .inc();

                        return Some(self.connect(connection).await);
                    }
                    Err(e) => warn!(target: LOG_CONSENSUS, "Failed to connect to peer: {e}")
                }

                Some(P2PConnectionSMState::Disconnected(backoff))
            },
        }
    }
}
