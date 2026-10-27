use agent_common::logging::{self, FileConfig, Logging, Options};
use agent_debug::{cli, output::Format};
use cli_command_kit::Mode;
use std::io;
use tokio_util::sync::CancellationToken;

fn main() {
    let format = Format::capture();
    let mode = if format.mode == Mode::Agent {
        logging::Mode::Agent
    } else {
        logging::Mode::Human
    };
    let code = logging::run_command(
        cli::Console::new(io::stderr(), format.clone()),
        "agent-debug",
        mode,
        |logging| -> agent_debug::Result<()> {
            let Some(matches) = cli::parse(std::env::args_os(), &format, &mut io::stdout().lock())?
            else {
                return Ok(());
            };
            *logging = Logging::setup(
                cli::Console::new(io::stderr(), format.clone()),
                Options {
                    service: "agent-debug".into(),
                    level: if matches
                        .get_one::<bool>("debug")
                        .copied()
                        .unwrap_or_default()
                    {
                        "debug"
                    } else {
                        "warn"
                    }
                    .into(),
                    mode,
                },
                FileConfig::from_env(),
            )?;
            tracing::dispatcher::set_global_default(logging.dispatch().clone())
                .map_err(io::Error::other)?;
            tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(async {
                let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
                let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
                let cancel = CancellationToken::new();
                let mut stdout = io::stdout().lock();
                let mut stderr = io::stderr().lock();
                let command = cli::execute(matches, &cancel, &format, &mut stdout, &mut stderr);
                tokio::pin!(command);
                tokio::select! {
                    result = &mut command => result,
                    _ = async { tokio::select! { _ = interrupt.recv() => {}, _ = terminate.recv() => {} } } => {
                        cancel.cancel();
                        command.await
                    }
                }
            })
        },
    );
    std::process::exit(code);
}
