//! A single SQLite owner: acknowledged telemetry, classifier state, and usage share a commit boundary.
use crate::{
    classifier::{Classifier, Config, SummaryJob},
    summarizer::SummaryResult,
};
use agent_common::types::*;
use agent_status_store::{read, write};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Store(#[from] agent_status_store::Error),
    #[error("SQLite transaction failed")]
    Sql(#[from] rusqlite::Error),
    #[error("unsupported telemetry version")]
    Version,
    #[error("telemetry environment does not match this observer connection")]
    Environment,
    #[error("telemetry agent identity is empty or belongs to another environment")]
    Identity,
    #[error("telemetry sequence must be positive")]
    Sequence,
    #[error("state change contains no supported fields")]
    EmptyState,
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StateChange {
    pub is_hidden: Option<bool>,
    pub dismiss_completion: Option<bool>,
    pub acknowledge_completion: Option<bool>,
}
#[derive(Debug, Default)]
pub struct Batch {
    pub up_to: i64,
    pub updates: Vec<AgentState>,
    pub events: Vec<TelemetryFrame>,
    pub summaries: Vec<SummaryJob>,
}
pub struct Ingestion {
    db: Connection,
    classifier: Classifier,
}
impl Ingestion {
    pub fn open(path: impl AsRef<Path>, config: Config) -> Result<Self> {
        Ok(Self::from_connection(
            agent_status_store::open_writer(path)?,
            config,
        ))
    }
    pub fn from_connection(db: Connection, config: Config) -> Self {
        Self {
            db,
            classifier: Classifier::new(config),
        }
    }
    pub fn connection(&self) -> &Connection {
        &self.db
    }
    pub fn classifier(&self) -> &Classifier {
        &self.classifier
    }
    pub fn classifier_mut(&mut self) -> &mut Classifier {
        &mut self.classifier
    }

    pub fn hello(
        &mut self,
        mut environment: EnvironmentInfo,
        since: i64,
        now: i64,
    ) -> Result<EnvironmentInfo> {
        if environment.id.is_empty() {
            return Err(Error::Environment);
        }
        environment.last_heartbeat_at = now;
        environment.is_stale = false;
        let tx = self.db.transaction()?;
        write::upsert_environment(&tx, &environment)?;
        if since == 0 {
            write::save_cursor(&tx, &environment.id, 0)?;
        }
        tx.commit()?;
        Ok(environment)
    }
    pub fn heartbeat(&mut self, environment: &str, now: i64, dropped: i64) -> Result<()> {
        let tx = self.db.transaction()?;
        write::heartbeat(&tx, environment, now)?;
        if dropped > 0 {
            write::dropped_frames(&tx, environment, dropped)?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn accept_hello(
        &mut self,
        environment: Option<EnvironmentInfo>,
        id: &str,
        since: i64,
        dropped: i64,
        now: i64,
    ) -> Result<Option<EnvironmentInfo>> {
        if id.is_empty()
            || environment
                .as_ref()
                .is_some_and(|env| !env.id.is_empty() && env.id != id)
        {
            return Err(Error::Environment);
        }
        let mut environment = environment;
        let tx = self.db.transaction()?;
        if let Some(environment) = environment.as_mut() {
            environment.id = id.into();
            environment.last_heartbeat_at = now;
            environment.is_stale = false;
            write::upsert_environment(&tx, environment)?;
            if dropped > 0 {
                write::dropped_frames(&tx, id, dropped)?;
            }
        }
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM environments WHERE id=?)",
            [id],
            |row| row.get(0),
        )?;
        if since == 0 && exists {
            write::save_cursor(&tx, id, 0)?;
        }
        tx.commit()?;
        Ok(environment)
    }
    pub fn process_frames(
        &mut self,
        environment: &str,
        frames: &[TelemetryFrame],
        clock: i64,
    ) -> Result<Batch> {
        if environment.is_empty() {
            return Err(Error::Environment);
        }
        for frame in frames {
            if frame.version != "1.0" {
                return Err(Error::Version);
            }
            if frame.env_id != environment
                || (!frame.environment.id.is_empty() && frame.environment.id != environment)
            {
                return Err(Error::Environment);
            }
            if frame.agent_id.is_empty() {
                return Err(Error::Identity);
            }
            if frame.seq <= 0 {
                return Err(Error::Sequence);
            }
        }
        let checkpoints: BTreeMap<_, _> = frames
            .iter()
            .map(|frame| {
                (
                    frame.agent_id.clone(),
                    self.classifier.checkpoint(&frame.agent_id),
                )
            })
            .collect();
        let result = (|| {
            let tx = self.db.transaction()?;
            let mut batch = Batch {
                up_to: read::last_sequence(&tx, environment)?,
                ..Default::default()
            };
            for frame in frames {
                if frame.seq <= batch.up_to {
                    continue;
                }
                let mut frame = frame.clone();
                if frame.timestamp == 0 {
                    frame.timestamp = clock;
                }
                frame.environment.id = environment.into();
                // The ingestion connection's heartbeat is authoritative, not a transcript's event time.
                frame.environment.last_heartbeat_at = clock;
                frame.environment.is_stale = false;
                let existing = match read::agent(&tx, &frame.agent_id, clock) {
                    Ok(agent) => Some(agent),
                    Err(agent_status_store::Error::NotFound) => None,
                    Err(error) => return Err(error.into()),
                };
                if existing
                    .as_ref()
                    .is_some_and(|agent| agent.environment.id != environment)
                {
                    return Err(Error::Identity);
                }
                let seen = read::distinct_subagents(&tx, &frame.agent_id)?;
                let outcome = self.classifier.process(existing, &frame, &seen, clock);
                write::save_agent(&tx, &outcome.agent)?;
                write::record_event(&tx, &frame)?;
                batch.up_to = frame.seq;
                batch.updates.push(outcome.agent);
                batch.events.push(frame);
                batch.summaries.extend(outcome.summary);
            }
            write::save_cursor(&tx, environment, batch.up_to)?;
            tx.commit()?;
            Ok(batch)
        })();
        if result.is_err() {
            for (id, checkpoint) in checkpoints {
                self.classifier.rollback(&id, checkpoint);
            }
        }
        result
    }
    pub fn change_state(&mut self, id: &str, change: StateChange, now: i64) -> Result<AgentState> {
        if change.is_hidden.is_none()
            && change.dismiss_completion.is_none()
            && change.acknowledge_completion.is_none()
        {
            return Err(Error::EmptyState);
        }
        let dismiss = change.dismiss_completion.unwrap_or(false)
            || change.acknowledge_completion.unwrap_or(false);
        let tx = self.db.transaction()?;
        // Check existence even when a supplied boolean is false and has no mutation side effect.
        read::agent(&tx, id, now)?;
        if dismiss {
            write::dismiss_completion(&tx, id)?;
        }
        if let Some(hidden) = change.is_hidden {
            write::set_visibility(&tx, id, hidden, hidden.then_some(now), now)?;
        }
        let result = read::agent(&tx, id, now)?;
        tx.commit()?;
        if dismiss {
            self.classifier.dismiss_completion(id);
        }
        Ok(result)
    }
    pub fn apply_summary(
        &mut self,
        id: &str,
        job: &SummaryJob,
        result: SummaryResult,
        now: i64,
    ) -> Result<Option<AgentState>> {
        let checkpoint = self.classifier.checkpoint(id);
        let result = (|| {
            let tx = self.db.transaction()?;
            let mut agent = match read::agent(&tx, id, now) {
                Ok(agent) => agent,
                Err(agent_status_store::Error::NotFound) => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            if !self.classifier.apply_summary(&mut agent, job, result, now) {
                return Ok(None);
            }
            write::save_agent(&tx, &agent)?;
            tx.commit()?;
            Ok(Some(agent))
        })();
        if result.is_err() {
            self.classifier.rollback(id, checkpoint);
        }
        result
    }
    pub fn tick(&mut self, now: i64) -> Result<Batch> {
        let ids = self.classifier.due_agents(now);
        let checkpoints: Vec<_> = ids
            .iter()
            .map(|id| (id.clone(), self.classifier.checkpoint(id)))
            .collect();
        let result = (|| {
            let tx = self.db.transaction()?;
            let mut batch = Batch::default();
            for id in ids {
                let agent = match read::agent(&tx, &id, now) {
                    Ok(agent) => agent,
                    Err(agent_status_store::Error::NotFound) => {
                        self.classifier.remove(&id);
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                };
                let Some(outcome) = self.classifier.tick(agent, now) else {
                    continue;
                };
                write::save_agent(&tx, &outcome.agent)?;
                batch.updates.push(outcome.agent);
                batch.summaries.extend(outcome.summary);
            }
            tx.commit()?;
            Ok(batch)
        })();
        if result.is_err() {
            for (id, checkpoint) in checkpoints {
                self.classifier.rollback(&id, checkpoint);
            }
        }
        result
    }
    pub fn prune(&mut self, now: i64) -> Result<()> {
        let tx = self.db.transaction()?;
        write::prune(
            &tx,
            now.saturating_sub(48 * 3600 * 1000),
            now.saturating_sub(30 * 24 * 3600 * 1000),
        )?;
        tx.commit()?;
        self.db.execute_batch("PRAGMA incremental_vacuum(1000)")?;
        Ok(())
    }
    pub fn reconcile(
        &mut self,
        snapshot: &agent_watcher::scanner::ProcessSnapshot,
        config: &crate::reconciler::ReconcileConfig,
        now: i64,
    ) -> Result<Batch> {
        let tx = self.db.transaction()?;
        let agents = read::agents(
            &tx,
            &read::AgentFilter {
                environment: config.environment.clone(),
                lifecycle: Some(LifecycleState::Running),
                ..Default::default()
            },
            now,
        )?;
        let mut batch = Batch::default();
        for agent in agents {
            if agent.activity.lifecycle == LifecycleState::Exited
                || (config.environment.as_deref().unwrap_or_default().is_empty()
                    && !config.hostname.is_empty()
                    && !agent.environment.hostname.is_empty()
                    && agent.environment.hostname != config.hostname)
            {
                continue;
            }
            let (pid, started) = agent
                .activity
                .current_run
                .as_ref()
                .map_or((0, 0), |run| (run.pid, run.started_at));
            let terminated = if pid > 0 {
                !crate::reconciler::process_alive(snapshot, pid, started)
            } else {
                now.saturating_sub(agent.activity.updated_at)
                    >= i64::try_from(config.abandoned_after.as_millis()).unwrap_or(i64::MAX)
            };
            if terminated {
                write::end_current_run(&tx, &agent.id, now)?;
                batch.updates.push(read::agent(&tx, &agent.id, now)?);
            }
        }
        tx.commit()?;
        for agent in &batch.updates {
            self.classifier.remove(&agent.id);
        }
        Ok(batch)
    }
}
