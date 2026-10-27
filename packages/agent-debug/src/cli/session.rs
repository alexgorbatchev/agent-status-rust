use super::{blocking, db_path, flag, harness, value};
use crate::{
    Error, Result, comparison,
    database::{self, DatabaseSearch},
    output::{Format, SessionSummary},
    replay::{self, Options},
    transcript::{self, LoadOptions},
    validation,
};
use clap::ArgMatches;
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

async fn resolve(
    target: &str,
    hint: Option<agent_common::types::HarnessType>,
    cancel: &CancellationToken,
) -> Result<transcript::ResolvedTarget> {
    let target = PathBuf::from(target);
    let token = cancel.clone();
    blocking(move || {
        transcript::resolve_target(
            &token,
            &target,
            hint,
            &transcript::search_roots(&dirs::home_dir().unwrap_or_default(), |key| {
                std::env::var(key).ok()
            }),
        )
    })
    .await
}
async fn options(
    m: &ArgMatches,
    target: &str,
    cancel: &CancellationToken,
) -> Result<(PathBuf, Options)> {
    let resolved = resolve(target, harness(m), cancel).await?;
    let options = Options {
        load: LoadOptions {
            harness: Some(resolved.harness),
            session_id: if value(m, "session-id").is_empty() {
                resolved.session_id
            } else {
                Some(value(m, "session-id").into())
            },
            cwd: (!value(m, "cwd").is_empty()).then(|| value(m, "cwd").into()),
            strict_lifecycle: flag(m, "strict-lifecycle"),
        },
        environment: value(m, "env").into(),
        diff: flag(m, "diff"),
        until_line: m
            .try_get_one::<usize>("until-line")
            .ok()
            .flatten()
            .copied()
            .filter(|n| *n > 0),
        until_event: (!value(m, "until-event").is_empty()).then(|| value(m, "until-event").into()),
        stop_on_error: flag(m, "stop-on-error"),
        export_db: (!value(m, "export-db").is_empty()).then(|| value(m, "export-db").into()),
        summarize: flag(m, "summarize"),
        ..Default::default()
    };
    Ok((resolved.file_path, options))
}
async fn compare(
    m: &ArgMatches,
    cancel: &CancellationToken,
    result: &mut replay::ReplayResult,
) -> Result<()> {
    if value(m, "db").is_empty() {
        return Ok(());
    }
    let path = db_path(value(m, "db"), DatabaseSearch::Sessions)?;
    let token = cancel.clone();
    let sid = result.session_id.clone();
    let record = blocking(move || {
        database::inspect(&token, &path, &sid, chrono::Utc::now().timestamp_millis())
    })
    .await?;
    let agent = result
        .agent
        .as_ref()
        .ok_or_else(|| Error::InvalidInput("no agent state produced from session".into()))?;
    result.comparison = Some(comparison::compare(
        agent,
        result.last_event_timestamp,
        &record,
    ));
    Ok(())
}
pub(super) async fn run(
    verb: &str,
    m: &ArgMatches,
    cancel: &CancellationToken,
    f: &Format,
    out: &mut impl Write,
) -> Result<()> {
    if verb == "list" {
        let target = resolve(
            value(m, "target"),
            Some(agent_common::types::HarnessType::Opencode),
            cancel,
        )
        .await?;
        let token = cancel.clone();
        let sessions = blocking(move || {
            let db = agent_parser::OpencodeDb::open(&target.file_path)?;
            let sessions = crate::opencode::query(&token, &db, |db| db.get_sessions())?;
            sessions
                .into_iter()
                .map(|s| {
                    let tokens = if s.tokens.total != 0 {
                        s.tokens.total
                    } else {
                        [
                            s.tokens.input,
                            s.tokens.output,
                            s.tokens.reasoning,
                            s.tokens.cache.read,
                            s.tokens.cache.write,
                        ]
                        .into_iter()
                        .try_fold(0i64, |sum, n| {
                            sum.checked_add(n).ok_or_else(|| {
                                Error::InvalidInput(format!(
                                    "session {} token total overflows",
                                    s.id
                                ))
                            })
                        })?
                    };
                    Ok(SessionSummary {
                        id: s.id,
                        title: s.title,
                        directory: s.directory,
                        created_at: chrono::DateTime::from_timestamp_millis(s.time_created)
                            .ok_or_else(|| {
                                Error::InvalidInput(
                                    "OpenCode creation timestamp is out of range".into(),
                                )
                            })?,
                        cost_usd: s.cost,
                        tokens,
                    })
                })
                .collect::<Result<Vec<_>>>()
        })
        .await?;
        return f.session_list(out, &sessions).map_err(Into::into);
    }
    let (path, options) = options(m, value(m, "target"), cancel).await?;
    if verb == "validate" {
        let result = replay::run(cancel, &path, &options).await;
        let (report, comparison) = match result {
            Ok(mut result) => {
                let mut report = validation::Report::replay(path.clone(), &result);
                if !value(m, "db").is_empty() {
                    match compare(m, cancel, &mut result).await {
                        Ok(()) => {
                            let comparison =
                                result.comparison.as_ref().expect("comparison requested");
                            let count = comparison.fields.iter().filter(|f| f.is_drift).count();
                            report.push(
                                "Database Discrepancy Check",
                                !comparison.has_drift,
                                if comparison.has_drift {
                                    format!("drift detected across {count} field(s)")
                                } else {
                                    "persisted database state matches replayed transcript".into()
                                },
                            );
                        }
                        Err(e) => report.push("Database Discrepancy Check", false, e.to_string()),
                    }
                }
                (report, result.comparison)
            }
            Err(e) => (validation::Report::failed(path.clone(), &e), None),
        };
        if flag(m, "json") {
            f.json(out, &report)?;
        } else {
            f.validation(out, &report)?;
            if let Some(comparison) = comparison {
                f.comparison(out, &comparison)?;
            }
        }
        return if report.passed {
            Ok(())
        } else {
            Err(Error::InvalidInput(format!(
                "session validation failed for {}",
                path.display()
            )))
        };
    }
    let mut result = replay::run_observed(cancel, &path, &options, |_, _, diff| {
        if !flag(m, "json")
            && let Some(diff) = diff
        {
            f.diff(out, diff)?;
        }
        Ok(())
    })
    .await?;
    if verb == "view" {
        let agent = result
            .agent
            .ok_or_else(|| Error::InvalidInput("no agent state produced from session".into()))?;
        return if flag(m, "json") {
            f.json(out, &agent)
        } else {
            f.card(out, &agent, chrono::Utc::now().timestamp_millis())
                .map_err(Into::into)
        };
    }
    compare(m, cancel, &mut result).await?;
    if flag(m, "json") {
        f.json(out, &result)?;
    } else {
        f.replay_summary(out, &result)?;
        if let Some(comparison) = &result.comparison {
            f.comparison(out, comparison)?;
        }
    }
    // Open-ended transcripts are a supported replay result. Validation remains strict.
    if !result.verification.passes_replay() {
        return Err(Error::InvalidInput(format!(
            "read-side verification failed: {}",
            result
                .verification
                .last_error
                .as_deref()
                .unwrap_or("query verification failed")
        )));
    }
    Ok(())
}
async fn stored(
    path: PathBuf,
    cancel: &CancellationToken,
) -> Result<Vec<agent_common::types::AgentState>> {
    let token = cancel.clone();
    blocking(move || database::list_agents(&token, &path, chrono::Utc::now().timestamp_millis()))
        .await
}
pub(super) async fn fleet(
    m: &ArgMatches,
    cancel: &CancellationToken,
    f: &Format,
    out: &mut impl Write,
) -> Result<()> {
    let targets = m
        .get_many::<String>("targets")
        .map(|values| values.map(String::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let mut agents = Vec::new();
    if targets.is_empty() {
        agents = stored(db_path(value(m, "db"), DatabaseSearch::Sessions)?, cancel).await?;
    }
    for target in targets {
        if Path::new(target).extension().is_some_and(|e| e == "db") {
            agents.extend(stored(db_path(target, DatabaseSearch::Sessions)?, cancel).await?);
        } else {
            let (path, options) = options(m, target, cancel).await?;
            if let Some(agent) = replay::run(cancel, &path, &options).await?.agent {
                agents.push(agent);
            }
        }
    }
    if flag(m, "json") {
        f.json(out, &agents)
    } else {
        f.fleet(out, &agents, chrono::Utc::now().timestamp_millis())
            .map_err(Into::into)
    }
}
