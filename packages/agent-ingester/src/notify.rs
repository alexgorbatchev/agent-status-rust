//! Bounded, ordered Unix notification fanout. Slow subscribers must reconnect and
//! reload the authoritative database snapshot rather than delaying ingestion.
use agent_common::types::ServerEvent;
use std::{
    collections::BTreeMap,
    fs, io,
    os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::AsyncWriteExt,
    net::{UnixListener, UnixStream},
    sync::mpsc,
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

struct Client {
    sender: mpsc::Sender<Arc<[u8]>>,
    cancel: CancellationToken,
}
#[derive(Default)]
struct Clients(Mutex<BTreeMap<u64, Client>>);

pub struct Publisher {
    path: PathBuf,
    clients: Arc<Clients>,
    cancel: CancellationToken,
    task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl Publisher {
    pub async fn bind(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.file_type().is_socket() {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "notification path is not a socket",
                    ));
                }
                match UnixStream::connect(&path).await {
                    Ok(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrInUse,
                            "notification socket is active",
                        ));
                    }
                    Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                        let current = fs::symlink_metadata(&path)?;
                        if identity(&current) != identity(&metadata) {
                            return Err(io::Error::new(
                                io::ErrorKind::AlreadyExists,
                                "notification socket changed during stale check",
                            ));
                        }
                        fs::remove_file(&path)?;
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(&path)?;
        let metadata = fs::symlink_metadata(&path)?;
        let cleanup = SocketCleanup {
            path: path.clone(),
            identity: identity(&metadata),
        };
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let clients = Arc::new(Clients::default());
        let cancel = CancellationToken::new();
        let task = tokio::spawn(accept(listener, clients.clone(), cancel.clone(), cleanup));
        Ok(Self {
            path,
            clients,
            cancel,
            task: tokio::sync::Mutex::new(Some(task)),
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn client_count(&self) -> usize {
        self.clients
            .0
            .lock()
            .expect("notification client lock poisoned")
            .len()
    }
    pub fn publish(&self, event: &ServerEvent) -> serde_json::Result<()> {
        let mut payload = serde_json::to_vec(event)?;
        payload.push(b'\n');
        let payload: Arc<[u8]> = payload.into();
        self.clients
            .0
            .lock()
            .expect("notification client lock poisoned")
            .retain(|_, client| {
                if client.sender.try_send(payload.clone()).is_ok() {
                    true
                } else {
                    client.cancel.cancel();
                    false
                }
            });
        Ok(())
    }
    pub async fn shutdown(&self) -> io::Result<()> {
        self.cancel.cancel();
        if let Some(task) = self.task.lock().await.take() {
            task.await.map_err(io::Error::other)?;
        }
        Ok(())
    }
}
impl Drop for Publisher {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn identity(metadata: &fs::Metadata) -> (u64, u64) {
    (metadata.dev(), metadata.ino())
}
struct SocketCleanup {
    path: PathBuf,
    identity: (u64, u64),
}
impl Drop for SocketCleanup {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path)
            .is_ok_and(|meta| identity(&meta) == self.identity && meta.file_type().is_socket())
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

async fn accept(
    listener: UnixListener,
    clients: Arc<Clients>,
    cancel: CancellationToken,
    _cleanup: SocketCleanup,
) {
    let mut tasks = JoinSet::new();
    let mut next_id = 0u64;
    loop {
        tokio::select! {biased;
            _ = cancel.cancelled() => break,
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
            connection = listener.accept() => match connection {
                Ok((stream,_)) => {
                    let id=next_id; next_id=next_id.wrapping_add(1);
                    let (sender, receiver)=mpsc::channel(100);
                    let child=cancel.child_token();
                    let hello=ServerEvent {kind:"hello".into(),at:now(),..Default::default()};
                    let mut payload=serde_json::to_vec(&hello).expect("hello event is serializable"); payload.push(b'\n');
                    sender.try_send(Arc::from(payload)).expect("new notification queue is empty");
                    clients.0.lock().expect("notification client lock poisoned").insert(id,Client {sender,cancel:child.clone()});
                    let clients=clients.clone();
                    tasks.spawn(async move {
                        write(stream,receiver,child).await;
                        clients.0.lock().expect("notification client lock poisoned").remove(&id);
                    });
                }
                Err(error) => {
                    tracing::warn!(%error,"notification accept failed");
                    tokio::select! {_=cancel.cancelled()=>break,_=tokio::time::sleep(Duration::from_millis(100))=>{}}
                }
            }
        }
    }
    while tasks.join_next().await.is_some() {}
    clients
        .0
        .lock()
        .expect("notification client lock poisoned")
        .clear();
}
async fn write(
    mut stream: UnixStream,
    mut receiver: mpsc::Receiver<Arc<[u8]>>,
    cancel: CancellationToken,
) {
    loop {
        let payload = tokio::select! {biased;_=cancel.cancelled()=>return,payload=receiver.recv()=>match payload {Some(payload)=>payload,None=>return}};
        tokio::select! {biased;_=cancel.cancelled()=>return,result=tokio::time::timeout(Duration::from_secs(2),stream.write_all(&payload))=>if !matches!(result,Ok(Ok(()))) {return;}}
    }
}
pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| i64::try_from(value.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}
