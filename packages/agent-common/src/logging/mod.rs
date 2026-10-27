//! Native tracing integration with atomic records and an explicitly owned disk lifecycle.
mod config;
mod disk;
use chrono::{Local, SecondsFormat};
pub use config::{FileConfig, Mode, Options, default_log_dir, is_agent_mode, parse_level};
use std::{
    fmt,
    io::{self, Write},
    sync::{Arc, Mutex, MutexGuard},
};
use tracing::{
    Dispatch, Event, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};
use tracing_subscriber::{
    Layer,
    layer::{Context, SubscriberExt},
    registry::LookupSpan,
};

#[derive(Clone, Debug, Default)]
struct Fields(Vec<(String, String)>);
fn quote(value: &str) -> String {
    if value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\\' | '='))
    {
        serde_json::to_string(value).expect("string serialization is infallible")
    } else {
        value.into()
    }
}
impl Fields {
    fn put(&mut self, key: &str, value: String) {
        if let Some((_, old)) = self.0.iter_mut().find(|(name, _)| name == key) {
            *old = value;
        } else {
            self.0.push((key.into(), value));
        }
    }
}
impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.put(field.name(), quote(value));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.put(field.name(), value.to_string());
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.put(field.name(), value.to_string());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.put(field.name(), value.to_string());
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.put(field.name(), value.to_string());
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.put(field.name(), quote(&format!("{value:?}")));
    }
    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.put(field.name(), quote(&value.to_string()));
    }
}
struct Sink {
    console: Box<dyn Write + Send>,
    disk: Option<disk::Disk>,
    error: Option<io::Error>,
}
impl Sink {
    fn record_error(&mut self, result: io::Result<()>) {
        if self.error.is_none() {
            self.error = result.err();
        }
    }
    fn write(&mut self, record: &[u8]) {
        let result = self.console.write_all(record);
        self.record_error(result);
        if let Some(disk) = self.disk.as_mut() {
            let result = disk.write(record);
            self.record_error(result);
        }
    }
}
fn lock(sink: &Mutex<Sink>) -> MutexGuard<'_, Sink> {
    sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
#[derive(Clone)]
struct LoggingLayer {
    sink: Arc<Mutex<Sink>>,
    mode: Mode,
    service: String,
}
impl<S> Layer<S> for LoggingLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let mut fields = Fields::default();
            attrs.record(&mut fields);
            span.extensions_mut().insert(fields);
        }
    }
    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && let Some(fields) = span.extensions_mut().get_mut::<Fields>()
        {
            values.record(fields);
        }
    }
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let action = fields
            .0
            .iter()
            .find(|(k, _)| k == "action")
            .or_else(|| fields.0.iter().find(|(k, _)| k == "message"))
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        fields
            .0
            .retain(|(key, _)| key != "message" && key != "action");
        let mut attrs = Vec::new();
        if !self.service.is_empty() {
            attrs.push(("service".into(), quote(&self.service)));
        }
        if let Some(scope) = ctx.event_scope(event) {
            let mut prefix = String::new();
            for span in scope.from_root() {
                if !prefix.is_empty() {
                    prefix.push('.');
                }
                prefix.push_str(span.name());
                if let Some(fields) = span.extensions().get::<Fields>() {
                    attrs.extend(
                        fields
                            .0
                            .iter()
                            .map(|(key, value)| (format!("{prefix}.{key}"), value.clone())),
                    );
                }
            }
        }
        attrs.extend(fields.0);
        let now = Local::now();
        let level = event.metadata().level();
        let mut line = match self.mode {
            Mode::Agent => format!(
                "time={} level={level} action={}",
                now.to_rfc3339_opts(SecondsFormat::Millis, true),
                if action.is_empty() { "\"\"" } else { &action }
            ),
            Mode::Human => format!(
                "{} [{level}]{}",
                now.format("%H:%M:%S"),
                if action.is_empty() {
                    String::new()
                } else {
                    format!(
                        " {}",
                        serde_json::from_str::<String>(&action).unwrap_or(action)
                    )
                }
            ),
        };
        let has_env = attrs
            .iter()
            .any(|(key, value)| key == "env" && value != "\"\"");
        let has_project = attrs
            .iter()
            .any(|(key, value)| key == "project" && value != "\"\"");
        for (key, value) in attrs {
            if self.mode == Mode::Human
                && (key == "service"
                    || (has_env && key == "envId")
                    || (has_project && key == "projectId"))
            {
                continue;
            }
            line.push_str(&format!(" {key}={value}"));
        }
        line.push('\n');
        lock(&self.sink).write(line.as_bytes());
    }
}

/// Own a subscriber and its writers. Install this dispatch explicitly in each task.
pub struct Logging {
    dispatch: Dispatch,
    sink: Arc<Mutex<Sink>>,
}
impl Logging {
    pub fn setup(
        console: impl Write + Send + 'static,
        options: Options,
        file: FileConfig,
    ) -> io::Result<Self> {
        let level = parse_level(&options.level)?;
        let (disk, warning) = if file.disabled {
            (None, None)
        } else {
            match disk::Disk::open(&options.service, &file) {
                Ok(disk) => (Some(disk), None),
                Err(error) => (None, Some(error)),
            }
        };
        let sink = Arc::new(Mutex::new(Sink {
            console: Box::new(console),
            disk,
            error: None,
        }));
        let layer = LoggingLayer {
            sink: sink.clone(),
            mode: options.mode,
            service: options.service,
        };
        let dispatch = Dispatch::new(tracing_subscriber::registry().with(layer.with_filter(level)));
        let logging = Self { dispatch, sink };
        if let Some(error) = warning {
            logging.with_default(|| tracing::warn!(error=%error,"log_rotator_init_failed"));
        }
        Ok(logging)
    }
    pub fn dispatch(&self) -> &Dispatch {
        &self.dispatch
    }
    pub fn with_default<T>(&self, operation: impl FnOnce() -> T) -> T {
        tracing::dispatcher::with_default(&self.dispatch, operation)
    }
    /// Flush, close disk output and return any recorded write/flush failure.
    pub fn finish(&self) -> io::Result<()> {
        let mut sink = lock(&self.sink);
        if let Some(mut disk) = sink.disk.take() {
            let result = disk.flush();
            sink.record_error(result);
        }
        let result = sink.console.flush();
        sink.record_error(result);
        sink.error.take().map_or(Ok(()), Err)
    }
}
impl Drop for Logging {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

/// The closure can replace the active logger after parsing command options.
pub fn run_command<E: fmt::Display>(
    console: impl Write + Send + 'static,
    service: &str,
    mode: Mode,
    run: impl FnOnce(&mut Logging) -> Result<(), E>,
) -> i32 {
    let mut logging = Logging::setup(
        console,
        Options {
            service: service.into(),
            level: "warn".into(),
            mode,
        },
        FileConfig {
            disabled: true,
            ..Default::default()
        },
    )
    .expect("warn is a supported level");
    let mut code = 0;
    if let Err(error) = run(&mut logging) {
        logging.with_default(|| tracing::error!(error=%error,"command_failed"));
        code = 1;
    }
    if let Err(error) = logging.finish() {
        logging.with_default(|| tracing::error!(error=%error,"log_close_failed"));
        code = 1;
    }
    code
}
pub fn http_error(detail: impl fmt::Display) {
    tracing::error!(detail=%detail,"http_server_error");
}
