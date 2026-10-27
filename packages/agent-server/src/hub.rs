//! Bounded, immutable fanout. Missing authoritative state forces a reconnect.
use agent_common::types::{ServerEvent, TelemetryFrame};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::mpsc;

pub const CLIENT_BUFFER_SIZE: usize = 64;

#[derive(Default)]
struct State {
    closed: bool,
    sequence: u64,
    global: HashMap<u64, mpsc::Sender<Arc<ServerEvent>>>,
    tails: HashMap<String, HashMap<u64, mpsc::Sender<Arc<TelemetryFrame>>>>,
}

/// Clones share registrations. Subscriptions unregister when dropped.
#[derive(Clone, Default)]
pub struct Hub(Arc<Mutex<State>>);

enum RegistrationKind {
    Global,
    Tail(String),
}
struct Registration {
    hub: Weak<Mutex<State>>,
    id: u64,
    kind: RegistrationKind,
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(hub) = self.hub.upgrade() {
            let mut state = hub
                .lock()
                .expect("hub operations never panic while holding their lock");
            match &self.kind {
                RegistrationKind::Global => {
                    state.global.remove(&self.id);
                }
                RegistrationKind::Tail(agent) => {
                    if let Some(subscribers) = state.tails.get_mut(agent) {
                        subscribers.remove(&self.id);
                        if subscribers.is_empty() {
                            state.tails.remove(agent);
                        }
                    }
                }
            }
        }
    }
}

pub struct GlobalSubscription {
    receiver: mpsc::Receiver<Arc<ServerEvent>>,
    _registration: Registration,
}
impl GlobalSubscription {
    pub async fn recv(&mut self) -> Option<Arc<ServerEvent>> {
        self.receiver.recv().await
    }
    pub fn try_recv(&mut self) -> Result<Arc<ServerEvent>, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }
}

pub struct TailSubscription {
    receiver: mpsc::Receiver<Arc<TelemetryFrame>>,
    _registration: Registration,
}
impl TailSubscription {
    pub async fn recv(&mut self) -> Option<Arc<TelemetryFrame>> {
        self.receiver.recv().await
    }
    pub fn try_recv(&mut self) -> Result<Arc<TelemetryFrame>, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }
}

impl Hub {
    pub fn subscribe_global(&self) -> GlobalSubscription {
        let (sender, receiver) = mpsc::channel(CLIENT_BUFFER_SIZE);
        let mut state = self
            .0
            .lock()
            .expect("hub operations never panic while holding their lock");
        state.sequence += 1;
        let id = state.sequence;
        if !state.closed {
            state.global.insert(id, sender);
        }
        GlobalSubscription {
            receiver,
            _registration: Registration {
                hub: Arc::downgrade(&self.0),
                id,
                kind: RegistrationKind::Global,
            },
        }
    }

    pub fn subscribe_tail(&self, agent: impl Into<String>) -> TailSubscription {
        let agent = agent.into();
        let (sender, receiver) = mpsc::channel(CLIENT_BUFFER_SIZE);
        let mut state = self
            .0
            .lock()
            .expect("hub operations never panic while holding their lock");
        state.sequence += 1;
        let id = state.sequence;
        if !state.closed {
            state
                .tails
                .entry(agent.clone())
                .or_default()
                .insert(id, sender);
        }
        TailSubscription {
            receiver,
            _registration: Registration {
                hub: Arc::downgrade(&self.0),
                id,
                kind: RegistrationKind::Tail(agent),
            },
        }
    }

    pub fn broadcast(&self, event: ServerEvent) {
        let mut state = self
            .0
            .lock()
            .expect("hub operations never panic while holding their lock");
        if state.closed {
            return;
        }
        let event = Arc::new(event);
        state
            .global
            .retain(|id, sender| match sender.try_send(event.clone()) {
                Ok(()) => true,
                Err(mpsc::error::TrySendError::Closed(_)) => false,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    let retain = event.kind == "log_append";
                    if retain {
                        tracing::warn!(
                            subscriber_id = id,
                            event_type = event.kind,
                            "subscriber_dropped"
                        );
                    } else {
                        tracing::warn!(
                            subscriber_id = id,
                            event_type = event.kind,
                            "subscriber_evicted"
                        );
                    }
                    retain
                }
            });
        if event.kind == "log_append"
            && !event.agent_id.is_empty()
            && let Some(frame) = &event.event
            && let Some(subscribers) = state.tails.get_mut(&event.agent_id)
        {
            let frame = Arc::new(frame.clone());
            subscribers.retain(|id, sender| match sender.try_send(frame.clone()) {
                Ok(()) => true,
                Err(mpsc::error::TrySendError::Closed(_)) => false,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!(
                        subscriber_id = id,
                        agent_id = event.agent_id,
                        "subscriber_dropped"
                    );
                    true
                }
            });
        }
    }

    /// Closes senders; consumers may drain events already accepted into their queues.
    pub fn close(&self) {
        let mut state = self
            .0
            .lock()
            .expect("hub operations never panic while holding their lock");
        state.closed = true;
        state.global.clear();
        state.tails.clear();
    }
    pub fn global_count(&self) -> usize {
        self.0
            .lock()
            .expect("hub operations never panic while holding their lock")
            .global
            .len()
    }
    pub fn tail_count(&self, agent: &str) -> usize {
        self.0
            .lock()
            .expect("hub operations never panic while holding their lock")
            .tails
            .get(agent)
            .map_or(0, HashMap::len)
    }
}
