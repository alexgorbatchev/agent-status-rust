//! Shared, fallible rendering for human and agent output.
mod events;
mod records;
mod replay;
mod text;
mod validation;
mod view;
use cli_command_kit::Mode;
pub use events::{event_label, event_summary, intermediate_tool_turn};
pub use replay::SessionSummary;
use serde::Serialize;
use std::{
    io::{self, Write},
    path::PathBuf,
};
pub use text::{sanitize, trim_project_prefix, truncate, truncate_left, visual_width};
pub use view::{activity_status, completed, stale_idle};

#[derive(Clone, Debug)]
pub struct Format {
    pub mode: Mode,
    pub width: usize,
    pub color: bool,
    pub home: Option<PathBuf>,
}
impl Format {
    pub fn capture() -> Self {
        use std::io::IsTerminal;
        Self {
            mode: Mode::from_env(),
            width: cli_command_kit::terminal_width(),
            color: std::io::stdout().is_terminal()
                && std::env::var_os("NO_COLOR").is_none_or(|s| s.is_empty()),
            home: dirs::home_dir(),
        }
    }
    pub fn text(&self, value: &str) -> String {
        let clean = sanitize(value);
        self.home.as_ref().map_or_else(
            || clean.clone(),
            |home| cli_command_kit::fold_message(&clean, home),
        )
    }
    pub fn line(&self, out: &mut impl Write, value: &str) -> io::Result<()> {
        let value = if self.color && self.mode == Mode::Human {
            value.into()
        } else {
            text::strip_ansi(value)
        };
        let value = self.home.as_ref().map_or_else(
            || value.clone(),
            |home| cli_command_kit::fold_message(&value, home),
        );
        writeln!(out, "{value}")
    }
    pub fn divider(&self, out: &mut impl Write, character: char) -> io::Result<()> {
        if self.mode == Mode::Human {
            self.line(out, &character.to_string().repeat(self.width))
        } else {
            Ok(())
        }
    }
    pub fn json(&self, out: &mut impl Write, data: &impl Serialize) -> crate::Result<()> {
        let mut value = serde_json::to_value(data)?;
        fn fold(value: &mut serde_json::Value, home: &std::path::Path) {
            match value {
                serde_json::Value::String(s) => *s = cli_command_kit::fold_message(s, home),
                serde_json::Value::Array(items) => {
                    items.iter_mut().for_each(|item| fold(item, home))
                }
                serde_json::Value::Object(items) => {
                    items.values_mut().for_each(|item| fold(item, home))
                }
                _ => (),
            }
        }
        if let Some(home) = &self.home {
            fold(&mut value, home);
        }
        serde_json::to_writer_pretty(&mut *out, &value)?;
        writeln!(out)?;
        Ok(())
    }
    fn table(
        &self,
        out: &mut impl Write,
        headers: &[&str],
        rows: impl IntoIterator<Item = Vec<String>>,
    ) -> io::Result<()> {
        use comfy_table::{ContentArrangement, Table, presets::ASCII_FULL};
        let mut table = Table::new();
        table
            .load_style(ASCII_FULL)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_width(u16::try_from(self.width).unwrap_or(u16::MAX))
            .set_header(headers);
        for row in rows {
            table.add_row(row.into_iter().map(|v| self.text(&v)).collect::<Vec<_>>());
        }
        self.line(out, &table.to_string())
    }
}
pub(crate) fn quote(value: &str) -> String {
    serde_json::to_string(value).expect("strings serialize as JSON")
}
pub(crate) fn timestamp(ms: i64, pattern: &str) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map_or_else(|| "-".into(), |time| time.format(pattern).to_string())
}
pub(crate) fn number(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut result = String::new();
    if value < 0 {
        result.push('-');
    }
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            result.push(',');
        }
        result.push(ch);
    }
    result
}
