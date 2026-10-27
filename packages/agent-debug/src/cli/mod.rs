//! Native Clap entrypoint. Errors are printed once at this boundary.
mod agents;
mod command;
mod session;
use crate::{
    Error, Result,
    database::{self, DatabaseSearch},
    output::Format,
};
use clap::ArgMatches;
pub use command::{SKILL_POINTER, command, help_options};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};
use tokio_util::sync::CancellationToken;
pub(super) fn value<'a>(m: &'a ArgMatches, name: &str) -> &'a str {
    m.try_get_one::<String>(name)
        .ok()
        .flatten()
        .map_or("", String::as_str)
}
pub(super) fn flag(m: &ArgMatches, name: &str) -> bool {
    m.try_get_one::<bool>(name)
        .ok()
        .flatten()
        .copied()
        .unwrap_or_default()
}
/// Shared logging writes complete UTF-8 records. Fold and sanitize only console output.
pub struct Console<W> {
    writer: W,
    format: Format,
}
impl<W> Console<W> {
    pub fn new(writer: W, format: Format) -> Self {
        Self { writer, format }
    }
}
impl<W: Write> Write for Console<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let record = std::str::from_utf8(bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        self.writer.write_all(self.format.text(record).as_bytes())?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}
pub(super) fn harness(m: &ArgMatches) -> Option<agent_common::types::HarnessType> {
    let name = value(m, "harness");
    (!name.is_empty()).then(|| {
        serde_json::from_value(serde_json::json!(if name == "claude" {
            "claude-code"
        } else {
            name
        }))
        .expect("string harness deserializes")
    })
}
pub(super) async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(Error::Task)?
}
pub(super) fn db_path(custom: &str, mode: DatabaseSearch) -> Result<PathBuf> {
    database::resolve_path(
        custom,
        mode,
        &dirs::home_dir().unwrap_or_default(),
        &std::env::current_dir()?,
        |key| std::env::var(key).ok(),
    )
}
/// Execute the complete CLI, including framework help and one error report.
pub async fn run(
    args: impl IntoIterator<Item = impl Into<OsString> + Clone>,
    cancel: &CancellationToken,
    format: &Format,
    out: &mut impl Write,
    err: &mut impl Write,
) -> i32 {
    let result = match parse(args, format, out) {
        Ok(Some(matches)) => execute(matches, cancel, format, out, err).await,
        Ok(None) => Ok(()),
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => 0,
        Err(error) => {
            let _ = format.line(
                err,
                &format!(
                    "ERR: {}",
                    format.text(&error.to_string()).replace(['\r', '\n'], " ")
                ),
            );
            1
        }
    }
}
/// Parse with Clap and print informational commands without starting runtime services.
pub fn parse(
    args: impl IntoIterator<Item = impl Into<OsString> + Clone>,
    f: &Format,
    out: &mut impl Write,
) -> Result<Option<ArgMatches>> {
    let mut root = command(f.mode);
    let matches = match root.clone().try_get_matches_from(args) {
        Ok(m) => m,
        Err(e) if e.kind() == clap::error::ErrorKind::DisplayVersion => {
            writeln!(out, "{}", env!("CARGO_PKG_VERSION"))?;
            return Ok(None);
        }
        Err(e) if e.kind() == clap::error::ErrorKind::DisplayHelp => {
            write!(out, "{e}")?;
            return Ok(None);
        }
        Err(mut e) => {
            e.remove(clap::error::ContextKind::Usage);
            return Err(Error::Arguments(e));
        }
    };
    if let Some(("agent", agent)) = matches.subcommand()
        && let Some(("list", list)) = agent.subcommand()
        && !value(list, "server-url").is_empty()
        && !flag(list, "all")
    {
        return Err(Error::InvalidInput(
            "--server-url requires --all=true".into(),
        ));
    }
    if cli_command_kit::write_skill(&matches, &help_options(), out)? {
        return Ok(None);
    }
    if let Some(("completion", m)) = matches.subcommand() {
        if let Some((name, _)) = m.subcommand() {
            let shell = match name {
                "bash" => clap_complete::Shell::Bash,
                "zsh" => clap_complete::Shell::Zsh,
                "fish" => clap_complete::Shell::Fish,
                _ => clap_complete::Shell::PowerShell,
            };
            let mut bytes = Vec::new();
            clap_complete::generate(shell, &mut root, "agent-debug", &mut bytes);
            out.write_all(&bytes)?;
        } else {
            write!(
                out,
                "{}",
                root.find_subcommand_mut("completion")
                    .expect("completion exists")
                    .render_help()
            )?;
        }
        return Ok(None);
    }
    Ok(Some(matches))
}

/// Execute parsed runtime commands. The caller owns logging and the error-report boundary.
pub async fn execute(
    matches: ArgMatches,
    cancel: &CancellationToken,
    f: &Format,
    out: &mut impl Write,
    err: &mut impl Write,
) -> Result<()> {
    let mut root = command(f.mode);
    let Some((subject, m)) = matches.subcommand() else {
        write!(out, "{}", root.render_help())?;
        return Ok(());
    };
    if [
        "session", "db", "scanner", "process", "agent", "fleet", "event",
    ]
    .contains(&subject)
        && m.subcommand().is_none()
    {
        write!(
            out,
            "{}",
            root.find_subcommand_mut(subject)
                .expect("parsed subject exists")
                .render_help()
        )?;
        return Ok(());
    }
    crate::check_cancel(cancel)?;
    if flag(&matches, "debug") {
        f.line(err, &format!("DEBUG: action:command subject:{subject}"))?;
    }
    match subject {
        "session" => {
            let (verb, m) = m.subcommand().expect("session verb exists");
            session::run(verb, m, cancel, f, out).await
        }
        "fleet" => {
            session::fleet(m.subcommand().expect("fleet verb exists").1, cancel, f, out).await
        }
        "event" => {
            let (_, m) = m.subcommand().expect("event verb exists");
            let path = db_path(value(m, "db"), DatabaseSearch::Events)?;
            let token = cancel.clone();
            let id = m.try_get_one::<i64>("id").ok().flatten().copied();
            let sid = value(m, "session-id").to_owned();
            let options = database::EventQuery {
                limit: m
                    .try_get_one::<i64>("limit")
                    .ok()
                    .flatten()
                    .copied()
                    .unwrap_or(1),
                event_types: value(m, "type").split(',').map(str::to_owned).collect(),
                subagent_id: (!value(m, "subagent-id").is_empty())
                    .then(|| value(m, "subagent-id").into()),
                include_subagents: id.is_some() || flag(m, "include-subagents"),
                descending: !flag(m, "asc") && flag(m, "desc"),
                event_id: id,
            };
            let events = blocking(move || database::events(&token, &path, &sid, &options)).await?;
            if let Some(id) = id
                && events.is_empty()
            {
                return Err(Error::InvalidInput(format!(
                    "telemetry event with ID {id} not found"
                )));
            }
            if flag(m, "json") {
                f.events_json(out, &events, id.is_some())
            } else if id.is_some() {
                f.event_detail(out, &events[0])
            } else {
                f.events(out, &events).map_err(Into::into)
            }
        }
        "db" => {
            let (_, m) = m.subcommand().expect("db verb exists");
            let path = db_path(value(m, "db"), DatabaseSearch::Sessions)?;
            let sid = value(m, "session-id").to_owned();
            let token = cancel.clone();
            let record = blocking(move || {
                database::inspect(&token, &path, &sid, chrono::Utc::now().timestamp_millis())
            })
            .await?;
            if flag(m, "json") {
                f.json(out, &record)
            } else {
                f.inspect(out, &record).map_err(Into::into)
            }
        }
        "scanner" | "process" => {
            let (_, m) = m.subcommand().expect("audit verb exists");
            let filter = crate::audit::AuditFilter {
                harness: harness(m),
                cwd: (!value(m, "cwd").is_empty()).then(|| value(m, "cwd").into()),
            };
            let token = cancel.clone();
            let (records, diagnostics) = blocking(move || {
                let now = chrono::Utc::now().timestamp_millis();
                let processes = crate::audit::capture(&token, now)?;
                let sessions = crate::audit::sessions(
                    &token,
                    &crate::audit::SessionPaths::default(),
                    filter.harness.as_ref(),
                )?;
                let records =
                    crate::audit::audit(&processes.groups, &sessions.sessions, &filter, now);
                Ok((records, sessions.diagnostics))
            })
            .await?;
            for diagnostic in diagnostics {
                f.line(
                    err,
                    &format!(
                        "WARN: {}: {}",
                        f.text(&diagnostic.path.display().to_string()),
                        f.text(&diagnostic.error)
                    ),
                )?;
            }
            if flag(m, "json") {
                f.json(out, &records)
            } else {
                f.audit(out, &records).map_err(Into::into)
            }
        }
        "agent" => {
            let (verb, m) = m.subcommand().expect("agent verb exists");
            agents::run(verb, m, cancel, f, out).await
        }
        _ => unreachable!("Clap accepted only declared subcommands"),
    }
}
