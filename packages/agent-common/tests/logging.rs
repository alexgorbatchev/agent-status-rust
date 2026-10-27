use agent_common::logging::{self, FileConfig, Logging, Mode, Options};
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);
impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Buffer {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}
fn options(mode: Mode) -> Options {
    Options {
        service: "observer".into(),
        level: "debug".into(),
        mode,
    }
}
fn console(buffer: Buffer, mode: Mode) -> Logging {
    Logging::setup(
        buffer,
        options(mode),
        FileConfig {
            disabled: true,
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn native_events_format_fields_and_suppress_redundant_identifiers() {
    let buf = Buffer::default();
    let log = console(buf.clone(), Mode::Human);
    log.with_default(|| {
        tracing::info!(
            env = "dev",
            envId = "e1",
            project = "project",
            projectId = "p1",
            count = 2_i64,
            unsigned = 3_u64,
            ratio = 0.5,
            enabled = true,
            empty = "",
            quoted = "has spaces=and\nnewline",
            "connected"
        );
        tracing::warn!(env = "", envId = "e1", projectId = "p1", "no_names");
        tracing::trace!("filtered");
        tracing::error!(error=?vec![1,2], "failed");
    });
    log.finish().unwrap();
    let text = buf.text();
    assert!(text.contains("[INFO] connected env=dev project=project count=2 unsigned=3 ratio=0.5 enabled=true empty=\"\""), "{text}");
    assert!(text.contains("quoted=\"has spaces=and\\nnewline\""));
    assert!(!text.lines().next().unwrap().contains("envId="));
    assert!(!text.contains("service="));
    assert!(text.contains("envId=e1 projectId=p1"));
    assert!(!text.contains("filtered"));
    assert!(text.contains("error=\"[1, 2]\""));
}

#[test]
fn agent_logfmt_preserves_service_ids_and_native_span_scopes() {
    let buf = Buffer::default();
    let log = console(buf.clone(), Mode::Agent);
    log.with_default(|| {
        let span = tracing::info_span!("request", id = 1, status = tracing::field::Empty);
        let _entered = span.enter();
        span.record("status", "ready");
        tracing::info!(env = "dev", envId = "e1", "connected now");
        logging::http_error("connection reset by peer");
    });
    let text = buf.text();
    assert!(text.contains("level=INFO action=\"connected now\" service=observer request.id=1 request.status=ready env=dev envId=e1"), "{text}");
    assert!(text.contains("action=http_server_error"));
    assert!(text.contains("detail=\"connection reset by peer\""));
    assert!(text.starts_with("time="));
    log.finish().unwrap();
}

#[test]
fn configuration_resolves_paths_and_preserves_explicit_unlimited_retention() {
    let vars = [
        ("AGENT_LOG_MAX_SIZE_MB", "2"),
        ("AGENT_LOG_MAX_BACKUPS", "0"),
        ("AGENT_LOG_MAX_AGE_DAYS", "0"),
        ("AGENT_LOG_COMPRESS", "off"),
        ("XDG_STATE_HOME", "/state"),
    ];
    let get = |key: &str| {
        vars.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).into())
    };
    let cfg = FileConfig::from_lookup(get);
    assert_eq!(
        cfg.path("observer").unwrap(),
        std::path::PathBuf::from("/state/agent-status/logs/observer.log")
    );
    assert_eq!(cfg.max_size_bytes, 2 * 1024 * 1024);
    assert_eq!(cfg.max_backups, 0);
    assert_eq!(cfg.max_age_days, 0);
    assert!(!cfg.compress);
    assert!(cfg.path("../escape").is_err());
    for value in ["1", " TRUE ", "yes"] {
        assert!(logging::is_agent_mode(value));
    }
    for value in ["0", "on", "false", ""] {
        assert!(!logging::is_agent_mode(value));
    }
    for (value, expected) in [
        ("", tracing::level_filters::LevelFilter::INFO),
        ("warning", tracing::level_filters::LevelFilter::WARN),
        ("ERROR", tracing::level_filters::LevelFilter::ERROR),
        ("debug", tracing::level_filters::LevelFilter::DEBUG),
    ] {
        assert_eq!(logging::parse_level(value).unwrap(), expected);
    }
    assert!(logging::parse_level("verbose").is_err());
}

#[test]
fn command_errors_are_reported_once_before_disk_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let buf = Buffer::default();
    let code = logging::run_command(buf.clone(), "command", Mode::Agent, |active| {
        *active = Logging::setup(
            buf.clone(),
            options(Mode::Agent),
            FileConfig {
                filename: Some(dir.path().join("command.log")),
                compress: false,
                ..Default::default()
            },
        )
        .unwrap();
        active.with_default(|| tracing::info!("started"));
        Err::<(), _>(io::Error::other("execution failed"))
    });
    assert_eq!(code, 1);
    assert_eq!(buf.text().matches("action=command_failed").count(), 1);
    let disk = std::fs::read_to_string(dir.path().join("command_rCURRENT.log")).unwrap();
    assert!(disk.contains("started"));
    assert!(disk.contains("execution failed"));
    assert_eq!(
        logging::run_command(
            Buffer::default(),
            "ok",
            Mode::Human,
            |_| Ok::<(), io::Error>(())
        ),
        0
    );
}

#[test]
fn initialization_failure_warns_and_keeps_console_usable() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("file");
    std::fs::write(&blocked, b"file").unwrap();
    let buf = Buffer::default();
    let log = Logging::setup(
        buf.clone(),
        options(Mode::Agent),
        FileConfig {
            filename: Some(blocked.join("log")),
            ..Default::default()
        },
    )
    .unwrap();
    log.with_default(|| tracing::info!("still_working"));
    log.finish().unwrap();
    assert!(buf.text().contains("action=log_rotator_init_failed"));
    assert!(buf.text().contains("action=still_working"));
    assert!(
        Logging::setup(
            Buffer::default(),
            Options {
                level: "invalid".into(),
                ..options(Mode::Agent)
            },
            FileConfig::default()
        )
        .is_err()
    );
}

#[test]
fn rotations_compress_records_and_enforce_count_and_age() {
    use std::io::Read;
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("disk_r2000-01-01_00-00-00.log.gz");
    std::fs::write(&old, b"expired").unwrap();
    let unrelated = dir.path().join("other_r2000-01-01_00-00-00.log");
    std::fs::write(&unrelated, b"keep").unwrap();
    let log = Logging::setup(
        Buffer::default(),
        options(Mode::Agent),
        FileConfig {
            filename: Some(dir.path().join("disk.log")),
            max_size_bytes: 1,
            max_backups: 2,
            max_age_days: 1,
            compress: true,
            ..Default::default()
        },
    )
    .unwrap();
    log.with_default(|| {
        for index in 0..8 {
            tracing::info!(index, "record");
        }
    });
    log.finish().unwrap();
    assert!(!old.exists());
    assert!(unrelated.exists());
    let archives: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "gz"))
        .collect();
    assert_eq!(archives.len(), 2);
    for path in archives {
        let mut decoded = String::new();
        flate2::read::GzDecoder::new(std::fs::File::open(path).unwrap())
            .read_to_string(&mut decoded)
            .unwrap();
        assert!(decoded.contains("action=record"));
        assert!(decoded.ends_with('\n'));
    }
}

#[test]
fn zero_limits_keep_archives_and_explicit_disk_disable_creates_no_files() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("disk_r2000-01-01_00-00-00.log");
    std::fs::write(&old, b"old").unwrap();
    let cfg = FileConfig {
        filename: Some(dir.path().join("disk.log")),
        max_size_bytes: 1,
        max_backups: 0,
        max_age_days: 0,
        compress: false,
        ..Default::default()
    };
    let log = Logging::setup(Buffer::default(), options(Mode::Human), cfg.clone()).unwrap();
    log.with_default(|| {
        for _ in 0..7 {
            tracing::info!("record");
        }
    });
    log.finish().unwrap();
    assert!(old.exists());
    assert!(std::fs::read_dir(dir.path()).unwrap().count() >= 7);
    let disabled = dir.path().join("absent.log");
    let log = Logging::setup(
        Buffer::default(),
        options(Mode::Human),
        FileConfig {
            filename: Some(disabled.clone()),
            disabled: true,
            ..cfg
        },
    )
    .unwrap();
    log.with_default(|| tracing::info!("console_only"));
    log.finish().unwrap();
    assert!(!disabled.exists());
    assert!(!dir.path().join("absent_rCURRENT.log").exists());
}

struct FailedWriter;
impl Write for FailedWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("write failure"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("flush failure"))
    }
}
#[test]
fn write_and_flush_failures_remain_observable_at_finish() {
    let log = Logging::setup(
        FailedWriter,
        options(Mode::Agent),
        FileConfig {
            disabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    log.with_default(|| tracing::error!("error"));
    assert!(log.finish().is_err());
    let code = logging::run_command(FailedWriter, "broken", Mode::Human, |_| {
        Ok::<(), io::Error>(())
    });
    assert_eq!(code, 1);
}
