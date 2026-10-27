use agent_watcher::scanner::ProcessSnapshot;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct ReconcileConfig {
    pub hostname: String,
    pub environment: Option<String>,
    pub abandoned_after: Duration,
}
impl Default for ReconcileConfig {
    fn default() -> Self {
        Self {
            hostname: String::new(),
            environment: None,
            abandoned_after: Duration::from_secs(300),
        }
    }
}
pub fn process_alive(snapshot: &ProcessSnapshot, pid: i64, started_at: i64) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    pid > 0
        && snapshot.process(pid).is_some_and(|process| {
            process.alive
                && (started_at <= 0
                    || process.create_time <= 0
                    || process.create_time.saturating_sub(started_at) <= 60_000)
        })
}
