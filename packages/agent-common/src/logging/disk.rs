use super::FileConfig;
use chrono::{NaiveDateTime, Utc};
use flexi_logger::{
    Cleanup, Criterion, FileSpec, LogfileSelector, Naming,
    writers::{ArcFileLogWriter, FileLogWriter, FileLogWriterHandle},
};
use std::io::{self, Write};

pub(super) struct Disk {
    writer: ArcFileLogWriter,
    handle: FileLogWriterHandle,
    stem: String,
    max_age_days: u32,
}
impl Disk {
    pub(super) fn open(service: &str, cfg: &FileConfig) -> io::Result<Self> {
        let path = cfg.path(service)?;
        let stem = path
            .file_stem()
            .and_then(|v| v.to_str())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "log file name must be valid UTF-8",
                )
            })?
            .to_owned();
        let spec = FileSpec::try_from(path).map_err(io::Error::other)?;
        let count = if cfg.max_backups == 0 {
            usize::MAX
        } else {
            cfg.max_backups
        };
        let cleanup = if cfg.compress {
            Cleanup::KeepCompressedFiles(count)
        } else if cfg.max_backups > 0 {
            Cleanup::KeepLogFiles(count)
        } else {
            Cleanup::Never
        };
        let (writer, handle) = FileLogWriter::builder(spec)
            .append()
            .use_utc()
            .cleanup_in_background_thread(false)
            .rotate(
                Criterion::Size(if cfg.max_size_bytes == 0 {
                    10 * 1024 * 1024
                } else {
                    cfg.max_size_bytes
                }),
                Naming::Timestamps,
                cleanup,
            )
            .try_build_with_handle()
            .map_err(io::Error::other)?;
        let disk = Self {
            writer,
            handle,
            stem,
            max_age_days: cfg.max_age_days,
        };
        disk.purge_expired()?;
        Ok(disk)
    }
    // flexi_logger handles rotation, compression and count. Its cleanup enum cannot
    // combine age and count, so apply the missing age policy only to its own archives.
    fn purge_expired(&self) -> io::Result<()> {
        if self.max_age_days == 0 {
            return Ok(());
        }
        let cutoff = Utc::now() - chrono::Duration::days(i64::from(self.max_age_days));
        let archives = self
            .handle
            .existing_log_files(&LogfileSelector::default().with_compressed_files())
            .map_err(io::Error::other)?;
        let prefix = format!("{}_r", self.stem);
        for path in archives {
            let timestamp = path
                .file_name()
                .and_then(|v| v.to_str())
                .and_then(|v| v.strip_prefix(&prefix))
                .and_then(|v| v.get(..19));
            if let Some(date) =
                timestamp.and_then(|s| NaiveDateTime::parse_from_str(s, "%Y-%m-%d_%H-%M-%S").ok())
                && date.and_utc() < cutoff
            {
                std::fs::remove_file(path)?;
            }
        }
        Ok(())
    }
    pub(super) fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.purge_expired()
    }
    pub(super) fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}
