use agent_common::logging::{self, Logging};
use agent_ingester::{
    cli::{self, Action},
    runtime,
};
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
        "ingester",
        log_mode,
        |logging| -> Result<(), cli::Error> {
            let Action::Serve(options) =
                cli::parse(std::env::args_os(), mode, &mut io::stdout().lock())?
            else {
                return Ok(());
            };
            let (config, file) = cli::logging_options(&options, mode);
            *logging = Logging::setup(cli::Console(io::stderr()), config, file)?;
            tracing::dispatcher::set_global_default(logging.dispatch().clone())
                .map_err(io::Error::other)?;
            tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(async {
            let mut interrupt=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
            let mut terminate=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            let cancel=CancellationToken::new();let server=runtime::run(&cancel,*options);tokio::pin!(server);
            tokio::select! {
                result=&mut server=>result.map_err(Into::into),
                _=async {tokio::select! {_=interrupt.recv()=>{},_=terminate.recv()=>{}}}=>{cancel.cancel();server.await.map_err(Into::into)}
            }
        })
        },
    );
    std::process::exit(code);
}
