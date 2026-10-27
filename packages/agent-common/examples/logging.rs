use agent_common::{
    logging::{FileConfig, Logging, Mode, Options},
    models,
};

fn main() -> std::io::Result<()> {
    let logger = Logging::setup(
        std::io::stderr(),
        Options {
            service: "observer".into(),
            level: "info".into(),
            mode: Mode::Agent,
        },
        FileConfig {
            disabled: true,
            ..Default::default()
        },
    )?;
    logger.with_default(|| {
        tracing::info!(
            capacity = models::resolve_context_window("gpt-5.4"),
            "started"
        );
    });
    logger.finish()
}
