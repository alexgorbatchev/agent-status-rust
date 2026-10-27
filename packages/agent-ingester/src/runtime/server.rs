//! Hyper's native HTTP/1 header timeout and upgrade lifecycle.
use axum::Router;
use hyper::server::conn::http1;
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use std::{io, time::Duration};
use tokio::{net::TcpListener, task::JoinSet};
use tokio_util::sync::CancellationToken;

pub(super) async fn serve(
    listener: TcpListener,
    router: Router,
    cancel: CancellationToken,
    header_timeout: Duration,
) -> io::Result<()> {
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {biased;
            _=cancel.cancelled()=>break,
            result=tasks.join_next(),if !tasks.is_empty()=>{if let Some(Err(error))=result {tracing::error!(%error,"http_connection_task_failed");}},
            accepted=listener.accept()=>{
                let (stream,_)=accepted?;let router=router.clone();let cancel=cancel.clone();
                tasks.spawn(async move {
                    let mut builder=http1::Builder::new();builder.timer(TokioTimer::new()).header_read_timeout(header_timeout);
                    let connection=builder.serve_connection(TokioIo::new(stream),TowerToHyperService::new(router)).with_upgrades();tokio::pin!(connection);
                    let result=tokio::select! {biased;_=cancel.cancelled()=>{connection.as_mut().graceful_shutdown();connection.await},result=&mut connection=>result};
                    if let Err(error)=result {tracing::debug!(%error,"http_connection_closed");}
                });
            }
        }
    }
    drop(listener);
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            tracing::error!(%error,"http_connection_task_failed");
        }
    }
    Ok(())
}
