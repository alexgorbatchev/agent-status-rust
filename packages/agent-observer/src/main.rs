use agent_common::logging::{self, Logging};
use agent_observer::cli::{self, Action};
use cli_command_kit::Mode;
use std::io;
use tokio_util::sync::CancellationToken;

fn main() {
    let mode = Mode::from_env();
    let log_mode = if mode == Mode::Agent {
        logging::Mode::Agent
    } else {
        logging::Mode::Human
    };
    let code = logging::run_command(
        cli::Console(io::stderr()),
        "observer",
        log_mode,
        |logging| -> Result<(), cli::Error> {
            let Action::Observe(options) =
                cli::parse(std::env::args_os(), mode, &mut io::stdout().lock())?
            else {
                return Ok(());
            };
            let (log_options, file) = cli::logging_options(&options, mode);
            *logging = Logging::setup(cli::Console(io::stderr()), log_options, file)?;
            // This executable owns its one process-wide subscriber, including discovery workers.
            tracing::dispatcher::set_global_default(logging.dispatch().clone())
                .map_err(io::Error::other)?;
            tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(async {
            let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
            let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            let cancel = CancellationToken::new();
            let observation = cli::observe(&cancel, *options);
            tokio::pin!(observation);
            tokio::select! {
                result = &mut observation => result,
                _ = async { tokio::select! { _ = interrupt.recv() => {}, _ = terminate.recv() => {} } } => {
                    cancel.cancel();
                    observation.await
                }
            }
        })
        },
    );
    std::process::exit(code);
}
