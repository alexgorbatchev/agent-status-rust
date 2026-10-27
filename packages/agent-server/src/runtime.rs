//! Owned read-only service lifecycle with native HTTP header and shutdown deadlines.
mod server;
use crate::{
    api::Api,
    cli::Options,
    hub::Hub,
    store::ReadStore,
    subscriber::{Subscriber, SubscriberConfig},
};
use std::{io, net::SocketAddr, time::Duration};
use tokio::net::TcpListener;
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("opening read-only database: {0}")]
    Store(#[from] crate::store::Error),
    #[error("initializing mutation transport: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("service task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("service shutdown exceeded its deadline")]
    ShutdownTimeout,
}

#[derive(Clone, Copy)]
pub struct Schedule {
    pub header_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub reconnect_wait: Duration,
}
impl Default for Schedule {
    fn default() -> Self {
        Self {
            header_timeout: Duration::from_secs(10),
            shutdown_timeout: Duration::from_secs(5),
            reconnect_wait: Duration::from_secs(1),
        }
    }
}

pub struct Daemon {
    listener: TcpListener,
    address: SocketAddr,
    api: Api,
    store: ReadStore,
    hub: Hub,
    socket: std::path::PathBuf,
    stop: CancellationToken,
}
impl Daemon {
    pub async fn bind(options: Options, cancel: &CancellationToken) -> Result<Self, Error> {
        let listener = bind_listener(&options.addr).await?;
        let address = listener.local_addr()?;
        let store = ReadStore::open(&options.database, cancel).await?;
        let hub = Hub::default();
        let stop = cancel.child_token();
        let api = Api::new(store.clone(), hub.clone(), options.api, stop.clone())?;
        tracing::info!(path=%cli_command_kit::display_path(&options.database), "db_opened");
        Ok(Self {
            listener,
            address,
            api,
            store,
            hub,
            socket: options.socket,
            stop,
        })
    }
    pub fn address(&self) -> SocketAddr {
        self.address
    }
    pub fn hub(&self) -> &Hub {
        &self.hub
    }

    /// Cancellation stops admission and joins HTTP, upgraded sockets, notifications, and reads.
    pub async fn run(self, cancel: &CancellationToken, schedule: Schedule) -> Result<(), Error> {
        let Self {
            listener,
            address,
            api,
            store,
            hub,
            socket,
            stop,
        } = self;
        let _stop_on_drop = stop.clone().drop_guard();
        let mut http = AbortOnDropHandle::new(tokio::spawn(server::serve(
            listener,
            api.router(),
            stop.clone(),
            schedule.header_timeout,
        )));
        let subscriber = Subscriber::new(
            SubscriberConfig {
                socket_path: socket,
                reconnect_wait: schedule.reconnect_wait,
            },
            store.clone(),
            hub.clone(),
        );
        let mut notifications = AbortOnDropHandle::new(tokio::spawn(subscriber.run(stop.clone())));
        tracing::info!(addr=%address, "server_starting");
        let mut http_result = None;
        let mut notify_result = None;
        tokio::select! { biased;
            _ = cancel.cancelled() => {},
            _ = stop.cancelled() => {},
            result = &mut http => { http_result = Some(result); },
            result = &mut notifications => { notify_result = Some(result); },
        }
        tracing::info!("server_shutting_down");
        stop.cancel();
        hub.close();
        let closing = async {
            // Drain HTTP concurrently: failed/pending upgrades release their tracker tokens
            // only after the native HTTP connection finishes.
            let ((), (), (), ()) = tokio::join!(
                api.shutdown(),
                async {
                    if http_result.is_none() {
                        http_result = Some((&mut http).await);
                    }
                },
                async {
                    if notify_result.is_none() {
                        notify_result = Some((&mut notifications).await);
                    }
                },
                store.shutdown()
            );
            http_result.take().expect("HTTP task joined")??;
            notify_result.take().expect("notification task joined")??;
            Ok::<_, Error>(())
        };
        match tokio::time::timeout(schedule.shutdown_timeout, closing).await {
            Ok(result) => result,
            Err(_) => {
                // A completed JoinHandle must not be polled twice.
                if http_result.is_none() {
                    http.abort();
                    let _ = (&mut http).await;
                }
                if notify_result.is_none() {
                    notifications.abort();
                    let _ = (&mut notifications).await;
                }
                Err(Error::ShutdownTimeout)
            }
        }
    }
}

async fn bind_listener(address: &str) -> io::Result<TcpListener> {
    if address.starts_with(':') {
        let address: SocketAddr = format!("[::]{address}")
            .parse()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let socket = socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .and_then(|socket| {
            socket.set_only_v6(false)?;
            Ok(socket)
        });
        if let Ok(socket) = socket {
            socket.set_reuse_address(true)?;
            socket.set_nonblocking(true)?;
            socket.bind(&address.into())?;
            socket.listen(i32::MAX)?;
            return TcpListener::from_std(socket.into());
        }
        return TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], address.port()))).await;
    }
    TcpListener::bind(address).await
}

pub async fn run(cancel: &CancellationToken, options: Options) -> Result<(), Error> {
    if cancel.is_cancelled() {
        return Ok(());
    }
    Daemon::bind(options, cancel)
        .await?
        .run(cancel, Schedule::default())
        .await
}
