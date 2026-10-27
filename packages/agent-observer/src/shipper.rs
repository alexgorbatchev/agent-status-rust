//! Bounded, acknowledged telemetry delivery. Transcripts remain the durable queue.
use agent_common::types::{EnvironmentInfo, IngesterMessage, ObserverMessage, TelemetryFrame};
use agent_watcher::Acknowledgement;
use backoff::{ExponentialBackoff, ExponentialBackoffBuilder, backoff::Backoff};
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::BTreeMap,
    io,
    ops::Bound::{Excluded, Unbounded},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::TcpStream,
    sync::{Notify, OwnedSemaphorePermit, Semaphore},
    time::{Instant, interval_at, sleep},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{self, Message, client::IntoClientRequest},
};
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Clone)]
pub struct Config {
    pub ingester_url: Url,
    pub bearer_token: String,
    pub environment: EnvironmentInfo,
    pub buffer_size: usize,
    pub heartbeat_interval: Duration,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    pub sustained_health: Duration,
    pub backoff_randomization: f64,
}

impl Config {
    pub fn new(ingester_url: Url, environment: EnvironmentInfo) -> Self {
        Self {
            ingester_url,
            environment,
            bearer_token: String::new(),
            buffer_size: 100,
            heartbeat_interval: Duration::from_secs(10),
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
            sustained_health: Duration::from_secs(5),
            backoff_randomization: 0.5,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("shipper stopped")]
    Stopped,
    #[error("telemetry delivery cancelled")]
    Cancelled,
    #[error("shipper is already running")]
    AlreadyRunning,
    #[error("invalid shipper configuration: {0}")]
    Configuration(&'static str),
    #[error("invalid WebSocket request")]
    Request(#[source] tungstenite::http::header::InvalidHeaderValue),
    #[error("WebSocket transport failed")]
    Transport(#[source] Box<tungstenite::Error>),
    #[error("telemetry serialization failed")]
    Serialization(#[from] serde_json::Error),
    #[error("ingester acknowledgement exceeds transmitted sequence")]
    InvalidAcknowledgement,
    #[error("saving acknowledged transcript checkpoint failed")]
    Checkpoint(#[source] io::Error),
    #[error("telemetry sequence exhausted")]
    SequenceExhausted,
}

impl From<tungstenite::Error> for Error {
    fn from(error: tungstenite::Error) -> Self {
        Self::Transport(Box::new(error))
    }
}

struct Pending {
    payload: String,
    acknowledgement: Option<Acknowledgement>,
    _permit: OwnedSemaphorePermit,
}

#[derive(Default)]
struct State {
    last_sequence: i64,
    last_acknowledged: i64,
    pending: BTreeMap<i64, Pending>,
}

pub struct Shipper {
    config: Config,
    state: Mutex<State>,
    capacity: Arc<Semaphore>,
    incoming: Notify,
    stop: CancellationToken,
    running: AtomicBool,
}

impl Shipper {
    pub fn new(mut config: Config) -> Result<Self, Error> {
        if !matches!(config.ingester_url.scheme(), "ws" | "wss") {
            return Err(Error::Configuration("URL must use ws or wss"));
        }
        if config.heartbeat_interval.is_zero()
            || config.initial_backoff.is_zero()
            || config.max_backoff < config.initial_backoff
        {
            return Err(Error::Configuration(
                "heartbeat and backoff intervals must be positive, with maximum at least initial",
            ));
        }
        if !(0.0..=1.0).contains(&config.backoff_randomization) {
            return Err(Error::Configuration(
                "backoff randomization must be between zero and one",
            ));
        }
        if config.buffer_size == 0 {
            config.buffer_size = 100;
        }
        if config.buffer_size > Semaphore::MAX_PERMITS {
            return Err(Error::Configuration(
                "buffer size exceeds native semaphore capacity",
            ));
        }
        Ok(Self {
            capacity: Arc::new(Semaphore::new(config.buffer_size)),
            config,
            state: Mutex::new(State::default()),
            incoming: Notify::new(),
            stop: CancellationToken::new(),
            running: AtomicBool::new(false),
        })
    }

    pub async fn enqueue(
        &self,
        cancel: &CancellationToken,
        mut frame: TelemetryFrame,
        acknowledgement: Option<Acknowledgement>,
    ) -> Result<i64, Error> {
        let permit = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            _ = self.stop.cancelled() => return Err(Error::Stopped),
            permit = self.capacity.clone().acquire_owned() => permit.map_err(|_| Error::Stopped)?,
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        frame.seq = state
            .last_sequence
            .checked_add(1)
            .ok_or(Error::SequenceExhausted)?;
        let seq = frame.seq;
        let message = ObserverMessage {
            kind: "frames".into(),
            frames: vec![frame],
            ..Default::default()
        };
        let payload = serde_json::to_string(&message)?;
        state.pending.insert(
            seq,
            Pending {
                payload,
                acknowledgement,
                _permit: permit,
            },
        );
        state.last_sequence = seq;
        drop(state);
        self.incoming.notify_one();
        Ok(seq)
    }

    pub fn unacked_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending
            .len()
    }

    pub fn stop(&self) {
        self.stop.cancel();
    }

    /// Runs until cancelled or stopped. The caller owns and joins this future.
    pub async fn run(&self, cancel: &CancellationToken) -> Result<(), Error> {
        if self.running.swap(true, Ordering::AcqRel) {
            return Err(Error::AlreadyRunning);
        }
        struct Running<'a>(&'a AtomicBool);
        impl Drop for Running<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _running = Running(&self.running);
        let mut backoff = ExponentialBackoffBuilder::new()
            .with_initial_interval(self.config.initial_backoff)
            .with_max_interval(self.config.max_backoff)
            .with_multiplier(2.0)
            .with_randomization_factor(self.config.backoff_randomization)
            .with_max_elapsed_time(None)
            .build();
        let mut endpoint = self.config.ingester_url.clone();
        let _ = endpoint.set_username("");
        let _ = endpoint.set_password(None);
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        loop {
            if cancel.is_cancelled() || self.stop.is_cancelled() {
                return Ok(());
            }
            tracing::info!(action = "ingester_connecting", ingester_url = %endpoint);
            match self.connect_and_stream(cancel, &mut backoff).await {
                Err(Error::Cancelled | Error::Stopped) => return Ok(()),
                Err(error @ Error::Checkpoint(_)) => return Err(error),
                Err(error) => {
                    tracing::warn!(action = "ingester_disconnected", ingester_url = %endpoint, error = %error)
                }
                Ok(()) => return Ok(()),
            }
            let delay = backoff.next_backoff().unwrap_or(self.config.max_backoff);
            tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                _ = self.stop.cancelled() => return Ok(()),
                _ = sleep(delay) => {},
            }
        }
    }

    async fn connect_and_stream(
        &self,
        cancel: &CancellationToken,
        backoff: &mut ExponentialBackoff,
    ) -> Result<(), Error> {
        let mut request = self.config.ingester_url.as_str().into_client_request()?;
        if !self.config.bearer_token.is_empty() {
            request.headers_mut().insert(
                "Authorization",
                format!("Bearer {}", self.config.bearer_token)
                    .parse()
                    .map_err(Error::Request)?,
            );
        }
        let (mut socket, _) = tokio::select! {
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            _ = self.stop.cancelled() => return Err(Error::Stopped),
            result = connect_async(request) => result?,
        };
        self.send(cancel, &mut socket, self.hello(false)?).await?;
        tracing::info!(action = "ingester_connected");
        let mut heartbeat = interval_at(
            Instant::now() + self.config.heartbeat_interval,
            self.config.heartbeat_interval,
        );
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let healthy_at = sleep(self.config.sustained_health);
        tokio::pin!(healthy_at);
        let mut healthy = false;
        let mut sent_up_to = 0;
        loop {
            let next = self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pending
                .range((Excluded(sent_up_to), Unbounded))
                .next()
                .map(|(&seq, item)| (seq, item.payload.clone()));
            if let Some((seq, payload)) = next {
                self.send(cancel, &mut socket, payload).await?;
                sent_up_to = seq;
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(Error::Cancelled),
                _ = self.stop.cancelled() => return Err(Error::Stopped),
                message = socket.next() => match message {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(ack) = serde_json::from_str::<IngesterMessage>(&text)
                            && ack.kind == "ack" {
                                if ack.up_to > sent_up_to { return Err(Error::InvalidAcknowledgement); }
                                if !healthy { backoff.reset(); healthy = true; }
                                self.acknowledge(ack.up_to)?;
                        }
                    },
                    Some(Ok(Message::Close(_))) | None => return Err(tungstenite::Error::ConnectionClosed.into()),
                    Some(Err(error)) => return Err(error.into()),
                    Some(Ok(_)) => {},
                },
                _ = &mut healthy_at, if !healthy => { backoff.reset(); healthy = true; },
                _ = heartbeat.tick() => self.send(cancel, &mut socket, self.hello(true)?).await?,
                _ = self.incoming.notified() => {},
                _ = std::future::ready(()), if self.state.lock().unwrap_or_else(|e| e.into_inner()).pending.range((Excluded(sent_up_to), Unbounded)).next().is_some() => {},
            }
        }
    }

    fn hello(&self, heartbeat: bool) -> Result<String, Error> {
        let mut environment = self.config.environment.clone();
        if heartbeat {
            environment.last_heartbeat_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .try_into()
                .unwrap_or(i64::MAX);
        }
        let since = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last_acknowledged;
        Ok(serde_json::to_string(&ObserverMessage {
            kind: "hello".into(),
            env_id: environment.id.clone(),
            environment: Some(environment),
            since,
            ..Default::default()
        })?)
    }

    async fn send(
        &self,
        cancel: &CancellationToken,
        socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
        payload: String,
    ) -> Result<(), Error> {
        tokio::select! {
            _ = cancel.cancelled() => Err(Error::Cancelled),
            _ = self.stop.cancelled() => Err(Error::Stopped),
            result = socket.send(Message::Text(payload.into())) => result.map_err(Into::into),
        }
    }

    fn acknowledge(&self, up_to: i64) -> Result<(), Error> {
        loop {
            let next = self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pending
                .first_key_value()
                .filter(|(seq, _)| **seq <= up_to)
                .map(|(&seq, item)| (seq, item.acknowledgement.clone()));
            let Some((seq, acknowledgement)) = next else {
                break;
            };
            if let Some(acknowledgement) = acknowledgement {
                acknowledgement.acknowledge().map_err(Error::Checkpoint)?;
            }
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.pending.remove(&seq);
            state.last_acknowledged = seq;
        }
        Ok(())
    }
}
