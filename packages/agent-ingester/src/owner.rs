//! One dedicated thread owns SQLite and classifier state; all callers submit
//! ordered operations and await typed results without blocking Tokio workers.
use crate::ingestion::{self, Ingestion};
use std::{io, sync::Arc, thread};
use tokio::sync::{Mutex, mpsc, oneshot, watch};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Ingestion(#[from] ingestion::Error),
    #[error("database worker is closed")]
    Closed,
    #[error("database worker panicked")]
    Panicked,
    #[error("joining database worker: {0}")]
    Join(#[from] tokio::task::JoinError),
}
type Operation = Box<dyn FnOnce(&mut Ingestion) + Send>;
enum Work {
    Run(Operation),
    Stop,
}
#[derive(Clone)]
pub struct Owner {
    sender: mpsc::Sender<Work>,
    thread: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
    deadline: watch::Receiver<Option<i64>>,
}
impl Owner {
    pub fn start(mut ingestion: Ingestion) -> io::Result<Self> {
        let (sender, mut receiver) = mpsc::channel(64);
        let (deadline_sender, deadline) = watch::channel(ingestion.classifier().next_deadline());
        let thread = thread::Builder::new()
            .name("agent-ingester-database".into())
            .spawn(move || {
                while let Some(Work::Run(operation)) = receiver.blocking_recv() {
                    operation(&mut ingestion);
                    let next = ingestion.classifier().next_deadline();
                    deadline_sender.send_if_modified(|deadline| {
                        if *deadline == next {
                            false
                        } else {
                            *deadline = next;
                            true
                        }
                    });
                }
            })?;
        Ok(Self {
            sender,
            thread: Arc::new(Mutex::new(Some(thread))),
            deadline,
        })
    }
    pub fn deadlines(&self) -> watch::Receiver<Option<i64>> {
        self.deadline.clone()
    }
    pub async fn call<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Ingestion) -> ingestion::Result<T> + Send + 'static,
    ) -> Result<T, Error> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(Work::Run(Box::new(move |ingestion| {
                let _ = sender.send(operation(ingestion));
            })))
            .await
            .map_err(|_| Error::Closed)?;
        receiver
            .await
            .map_err(|_| Error::Closed)?
            .map_err(Error::Ingestion)
    }
    pub async fn shutdown(&self) -> Result<(), Error> {
        let mut guard = self.thread.lock().await;
        if guard.is_some() {
            let _ = self.sender.send(Work::Stop).await;
            let thread = guard
                .take()
                .expect("worker handle is present while shutdown lock is held");
            tokio::task::spawn_blocking(move || thread.join())
                .await?
                .map_err(|_| Error::Panicked)?;
        }
        Ok(())
    }
}
