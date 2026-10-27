//! Owned telemetry daemon lifecycle, exact deadlines, and periodic maintenance.
mod server;
use crate::{
    cli::Options,
    herdr,
    ingestion::Ingestion,
    notify::Publisher,
    owner::{self, Owner},
    receiver::{Receiver, wall_time},
    reconciler::ReconcileConfig,
    summarizer::Summarizer,
};
use agent_watcher::scanner::ProcessSnapshot;
use std::{io, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    net::TcpListener,
    time::{Instant, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Ingestion(#[from] crate::ingestion::Error),
    #[error(transparent)]
    Owner(#[from] owner::Error),
    #[error("initializing summarizer transport: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("HTTP server task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("daemon shutdown exceeded its deadline")]
    ShutdownTimeout,
}
#[derive(Clone, Copy)]
pub struct Schedule {
    pub reconcile: Duration,
    pub prune: Duration,
    pub header_timeout: Duration,
    pub shutdown_timeout: Duration,
}
impl Default for Schedule {
    fn default() -> Self {
        Self {
            reconcile: Duration::from_secs(30),
            prune: Duration::from_secs(3600),
            header_timeout: Duration::from_secs(10),
            shutdown_timeout: Duration::from_secs(5),
        }
    }
}
pub struct Daemon {
    listener: TcpListener,
    address: SocketAddr,
    owner: Owner,
    publisher: Arc<Publisher>,
    receiver: Receiver,
    reconcile: ReconcileConfig,
}
impl Daemon {
    pub async fn bind(mut options: Options) -> Result<Self, Error> {
        // Bind first so a conflicting daemon cannot mutate the shared database or notification socket.
        let listener = bind_listener(&options.addr).await?;
        let address = listener.local_addr()?;
        let summary = Arc::new(Summarizer::new(options.summarizer).await?);
        startup(&summary);
        options.classifier.summarize = !summary.is_disabled();
        let ingestion = Ingestion::open(&options.database, options.classifier)?;
        let owner = Owner::start(ingestion)?;
        let publisher = match Publisher::bind(&options.notify_socket).await {
            Ok(publisher) => Arc::new(publisher),
            Err(error) => {
                owner.shutdown().await?;
                return Err(error.into());
            }
        };
        let receiver = Receiver::new(
            owner.clone(),
            publisher.clone(),
            summary,
            options.token,
            Arc::new(herdr::Client::new(options.herdr_socket)),
        );
        tracing::info!(addr=%address,db=%options.database.display(),notifySock=%options.notify_socket.display(),"server_starting");
        Ok(Self {
            listener,
            address,
            owner,
            publisher,
            receiver,
            reconcile: ReconcileConfig {
                hostname: hostname::get()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                ..Default::default()
            },
        })
    }
    pub fn address(&self) -> SocketAddr {
        self.address
    }
    pub fn owner(&self) -> &Owner {
        &self.owner
    }
    pub fn receiver(&self) -> &Receiver {
        &self.receiver
    }
    pub async fn run(self, cancel: &CancellationToken, schedule: Schedule) -> Result<(), Error> {
        let Self {
            listener,
            owner,
            publisher,
            receiver,
            reconcile,
            ..
        } = self;
        let stop = receiver.cancellation_token();
        let serving = tokio::spawn(server::serve(
            listener,
            receiver.router(),
            stop.clone(),
            schedule.header_timeout,
        ));
        let mut serving = tokio_util::task::AbortOnDropHandle::new(serving);
        let mut deadlines = owner.deadlines();
        let interval = |duration: Duration| {
            let duration = duration.max(Duration::from_millis(1));
            let mut interval = tokio::time::interval_at(Instant::now() + duration, duration);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            interval
        };
        let mut reconciliation = interval(schedule.reconcile);
        let mut pruning = interval(schedule.prune);
        reconcile_once(&owner, &receiver, &reconcile, cancel).await;
        let mut server_result = None;
        loop {
            let deadline = *deadlines.borrow_and_update();
            let due = async {
                match deadline {
                    Some(at) => {
                        tokio::time::sleep(Duration::from_millis(
                            at.saturating_sub(wall_time()).max(0) as u64,
                        ))
                        .await
                    }
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                _=cancel.cancelled()=>break,
                _=stop.cancelled()=>break,
                result=&mut serving=>{server_result=Some(result);break;},
                changed=deadlines.changed()=>{if changed.is_err() {break;}},
                _=due=>{match owner.call(|ingestion|ingestion.tick(wall_time())).await {Ok(batch)=>{receiver.publish_batch(batch);},Err(error)=>{tracing::warn!(%error,"classifier_timer_failed");tokio::select! {biased;_=cancel.cancelled()=>break,_=tokio::time::sleep(Duration::from_millis(100))=>{}}}}},
                _=reconciliation.tick()=>reconcile_once(&owner,&receiver,&reconcile,cancel).await,
                _=pruning.tick()=>{if let Err(error)=owner.call(|ingestion|ingestion.prune(wall_time())).await {tracing::warn!(%error,"prune_cleanup_failed");}},
            }
        }
        tracing::info!("server_shutting_down");
        stop.cancel();
        let closing = async {
            let ((), http) = tokio::join!(receiver.shutdown(), async {
                if let Some(result) = server_result {
                    result
                } else {
                    serving.await
                }
            });
            let published = publisher.shutdown().await;
            let database = owner.shutdown().await;
            http??;
            published?;
            database?;
            Ok::<_, Error>(())
        };
        tokio::time::timeout(schedule.shutdown_timeout, closing)
            .await
            .map_err(|_| Error::ShutdownTimeout)?
    }
}
async fn bind_listener(addr: &str) -> io::Result<TcpListener> {
    if addr.starts_with(':') {
        let address: SocketAddr = format!("[::]{addr}")
            .parse()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        // Go's wildcard listener accepts IPv4-mapped connections when the host supports them.
        // Probe that capability before binding; binding failures must remain errors.
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
        return TcpListener::bind(format!("0.0.0.0{addr}")).await;
    }
    TcpListener::bind(addr).await
}
async fn reconcile_once(
    owner: &Owner,
    receiver: &Receiver,
    config: &ReconcileConfig,
    cancel: &CancellationToken,
) {
    if cancel.is_cancelled() {
        return;
    }
    let token = cancel.clone();
    let snapshot = tokio::task::spawn_blocking(move || ProcessSnapshot::capture(&token)).await;
    let snapshot = match snapshot {
        Ok(Ok(snapshot)) => snapshot,
        Ok(Err(error)) => {
            tracing::warn!(%error,"process_reconciliation_failed");
            return;
        }
        Err(error) => {
            tracing::warn!(%error,"process_snapshot_task_failed");
            return;
        }
    };
    let config = config.clone();
    match owner
        .call(move |ingestion| ingestion.reconcile(&snapshot, &config, wall_time()))
        .await
    {
        Ok(batch) => {
            receiver.publish_batch(batch);
        }
        Err(error) => tracing::warn!(%error,"process_reconciliation_failed"),
    }
}
fn startup(summary: &Summarizer) {
    if let Some(source) = summary.disabled_source() {
        tracing::info!(source=?source,"summarizer_disabled");
        return;
    }
    for (provider, error) in summary.init_errors() {
        tracing::error!(%provider,%error,"summarizer_provider_init_failed");
    }
    let providers = summary.active_providers();
    if providers.is_empty() && summary.init_errors().is_empty() {
        tracing::warn!(
            reason = "no_credentials",
            checked_env =
                "LITELLM_BASE_URL,LITELLM_API_KEY,OPENAI_API_KEY,GEMINI_API_KEY,ANTHROPIC_API_KEY",
            "summarizer_unconfigured"
        );
    }
    for provider in providers {
        tracing::info!(provider=%provider.name,model=%provider.model,endpoint=%provider.endpoint,"summarizer_ready");
    }
}
pub async fn run(cancel: &CancellationToken, options: Options) -> Result<(), Error> {
    if cancel.is_cancelled() {
        return Ok(());
    }
    Daemon::bind(options)
        .await?
        .run(cancel, Schedule::default())
        .await
}
