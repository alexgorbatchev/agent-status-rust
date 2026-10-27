use super::{blocking, db_path, flag, value};
use crate::{
    Result,
    audit::SessionPaths,
    client::ServerClient,
    database::{self, DatabaseSearch},
    discovery,
    output::Format,
};
use agent_observer::config::Identity;
use agent_watcher::scanner::ProcessSnapshot;
use clap::ArgMatches;
use std::{io::Write, time::Duration};
use tokio_util::sync::CancellationToken;
fn client(m: &ArgMatches) -> std::result::Result<ServerClient, crate::client::Error> {
    let url = if value(m, "server-url").is_empty() {
        std::env::var("AGENT_SERVER_HTTP_URL")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "http://127.0.0.1:4502".into())
    } else {
        value(m, "server-url").into()
    };
    ServerClient::new(
        &url,
        &std::env::var("AGENT_STATUS_TOKEN").unwrap_or_default(),
        Duration::from_secs(10),
    )
}
pub(super) async fn run(
    verb: &str,
    m: &ArgMatches,
    cancel: &CancellationToken,
    f: &Format,
    out: &mut impl Write,
) -> Result<()> {
    if verb == "usage" {
        let report = client(m)?.usage(cancel, value(m, "agent-id")).await?;
        return if flag(m, "json") {
            f.json(out, &report)
        } else {
            f.usage(out, &report).map_err(Into::into)
        };
    }
    let token = cancel.clone();
    let (identity, snapshot) =
        blocking(move || Ok((Identity::capture(), ProcessSnapshot::capture(&token)?))).await?;
    let agents = if flag(m, "all") {
        discovery::monitored(
            client(m)?.agents(cancel).await?,
            &identity.environment_id(),
            &snapshot,
        )
    } else if m.contains_id("db") {
        let path = db_path(value(m, "db"), DatabaseSearch::Sessions)?;
        let token = cancel.clone();
        let id = identity.environment_id();
        let agents = blocking(move || {
            database::list_agents(&token, &path, chrono::Utc::now().timestamp_millis())
        })
        .await?
        .into_iter()
        .filter(|a| a.environment.id == id)
        .collect();
        discovery::monitored(agents, &identity.environment_id(), &snapshot)
    } else {
        let token = cancel.clone();
        blocking(move || {
            discovery::local(
                &token,
                &SessionPaths::default(),
                &snapshot,
                &identity.hostname,
            )
        })
        .await?
    };
    for agent in agents {
        f.agent_row(out, &agent)?;
    }
    Ok(())
}
