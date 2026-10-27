//! Per-agent serialized LLM workers with bounded latest-context queues.
use crate::{
    classifier::SummaryJob,
    notify::{Publisher, now},
    owner::Owner,
    summarizer::Summarizer,
};
use agent_common::types::ServerEvent;
use agent_status_store::read;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};

#[derive(Default)]
struct State {
    jobs: VecDeque<SummaryJob>,
    busy: bool,
}
#[derive(Default)]
struct Queue {
    state: Mutex<State>,
    wake: Notify,
    changed: Notify,
}
struct Inner {
    queues: Mutex<BTreeMap<String, Arc<Queue>>>,
    tasks: Mutex<Vec<AbortOnDropHandle<()>>>,
    owner: Owner,
    summarizer: Arc<Summarizer>,
    publisher: Arc<Publisher>,
    cancel: CancellationToken,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
#[derive(Clone)]
pub struct Dispatcher(Arc<Inner>);
impl Dispatcher {
    pub fn new(owner: Owner, summarizer: Arc<Summarizer>, publisher: Arc<Publisher>) -> Self {
        Self(Arc::new(Inner {
            queues: Mutex::default(),
            tasks: Mutex::default(),
            owner,
            summarizer,
            publisher,
            cancel: CancellationToken::new(),
        }))
    }
    pub fn enqueue(&self, jobs: impl IntoIterator<Item = SummaryJob>) {
        let mut queues = self.0.queues.lock().expect("summary queue lock poisoned");
        if self.0.cancel.is_cancelled() {
            return;
        }
        for job in jobs {
            let queue = queues.entry(job.input.agent_id.clone()).or_insert_with(|| {
                let queue = Arc::new(Queue::default());
                self.0
                    .tasks
                    .lock()
                    .expect("summary task lock poisoned")
                    .push(AbortOnDropHandle::new(tokio::spawn(worker(
                        queue.clone(),
                        self.0.owner.clone(),
                        self.0.summarizer.clone(),
                        self.0.publisher.clone(),
                        self.0.cancel.clone(),
                    ))));
                queue
            });
            let mut state = queue.state.lock().expect("summary queue state poisoned");
            if state.jobs.len() == 10 {
                state.jobs.pop_front();
            }
            state.jobs.push_back(job);
            drop(state);
            queue.wake.notify_one();
        }
    }
    pub async fn flush(&self) {
        let queues: Vec<_> = self
            .0
            .queues
            .lock()
            .expect("summary queue lock poisoned")
            .values()
            .cloned()
            .collect();
        for queue in queues {
            loop {
                let changed = queue.changed.notified();
                let pending = {
                    let state = queue.state.lock().expect("summary queue state poisoned");
                    state.busy || !state.jobs.is_empty()
                };
                if !pending {
                    break;
                }
                tokio::select! {biased;_=self.0.cancel.cancelled()=>return,_=changed=>{}}
            }
        }
    }
    pub async fn shutdown(&self) {
        self.0.cancel.cancel();
        let tasks = {
            let mut queues = self.0.queues.lock().expect("summary queue lock poisoned");
            queues.clear();
            std::mem::take(&mut *self.0.tasks.lock().expect("summary task lock poisoned"))
        };
        for task in tasks {
            if let Err(error) = task.await {
                tracing::error!(%error,"summary worker failed");
            }
        }
    }
}
async fn worker(
    queue: Arc<Queue>,
    owner: Owner,
    summarizer: Arc<Summarizer>,
    publisher: Arc<Publisher>,
    cancel: CancellationToken,
) {
    loop {
        if cancel.is_cancelled() {
            return;
        }
        let wake = queue.wake.notified();
        let job = {
            let mut state = queue.state.lock().expect("summary queue state poisoned");
            let job = state.jobs.pop_front();
            state.busy = job.is_some();
            job
        };
        if let Some(job) = job {
            if let Err(error) = process(&owner, &summarizer, &publisher, &cancel, job).await {
                tracing::warn!(%error,"summary update failed");
            }
            queue
                .state
                .lock()
                .expect("summary queue state poisoned")
                .busy = false;
            queue.changed.notify_waiters();
        } else {
            tokio::select! {biased;_=cancel.cancelled()=>return,_=wake=>{}}
        }
    }
}
async fn process(
    owner: &Owner,
    summarizer: &Summarizer,
    publisher: &Publisher,
    cancel: &CancellationToken,
    job: SummaryJob,
) -> Result<(), crate::owner::Error> {
    let check = job.clone();
    let current = owner
        .call(move |ingestion| {
            match read::agent(ingestion.connection(), &check.input.agent_id, now()) {
                Ok(agent) => Ok(ingestion.classifier().summary_is_current(&agent, &check)),
                Err(agent_status_store::Error::NotFound) => Ok(false),
                Err(error) => Err(error.into()),
            }
        })
        .await?;
    if !current {
        return Ok(());
    }
    let result = summarizer.summarize(cancel, &job.input).await;
    if cancel.is_cancelled() {
        return Ok(());
    }
    let agent = owner
        .call(move |ingestion| ingestion.apply_summary(&job.input.agent_id, &job, result, now()))
        .await?;
    if let Some(agent) = agent {
        let _ = publisher.publish(&ServerEvent {
            kind: "agent_upsert".into(),
            at: now(),
            agent: Some(agent),
            ..Default::default()
        });
    }
    Ok(())
}
