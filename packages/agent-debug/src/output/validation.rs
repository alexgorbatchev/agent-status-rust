use super::{Format, quote};
use cli_command_kit::Mode;
use std::io::{self, Write};
impl Format {
    pub fn validation(
        &self,
        out: &mut impl Write,
        report: &crate::validation::Report,
    ) -> io::Result<()> {
        if self.mode == Mode::Agent {
            self.line(
                out,
                &format!(
                    "validation target:{} harness:{} passed:{}",
                    self.text(&report.target.display().to_string()),
                    report.harness.as_str(),
                    report.passed
                ),
            )?;
            for check in &report.checks {
                self.line(
                    out,
                    &format!(
                        "check:{} status:{} details:{}",
                        quote(&check.name),
                        if check.passed { "PASS" } else { "FAIL" },
                        quote(&self.text(&check.details))
                    ),
                )?;
            }
        } else {
            self.divider(out, '-')?;
            self.line(
                out,
                &format!(
                    "Session Validation: {}",
                    self.text(&report.target.display().to_string())
                ),
            )?;
            self.table(
                out,
                &["Check", "Status", "Details"],
                report.checks.iter().map(|c| {
                    vec![
                        c.name.clone(),
                        if c.passed { "PASS" } else { "FAIL" }.into(),
                        c.details.clone(),
                    ]
                }),
            )?;
            self.line(
                out,
                if report.passed {
                    "[OK] All validation checks passed"
                } else {
                    "[ERROR] Session validation failed"
                },
            )?;
        }
        Ok(())
    }
    pub fn comparison(
        &self,
        out: &mut impl Write,
        comparison: &crate::comparison::Comparison,
    ) -> io::Result<()> {
        if self.mode == Mode::Agent {
            self.line(
                out,
                &format!(
                    "comparison db:{} session_id:{} agent_id:{} has_drift:{}",
                    self.text(&comparison.db_path.display().to_string()),
                    self.text(&comparison.session_id),
                    self.text(&comparison.agent_id),
                    comparison.has_drift
                ),
            )?;
            for field in &comparison.fields {
                self.line(
                    out,
                    &format!(
                        "field:{} replayed:{} persisted:{} status:{}",
                        quote(&field.name),
                        quote(&self.text(&field.replayed_value)),
                        quote(&self.text(&field.persisted_value)),
                        quote(&field.status)
                    ),
                )?;
            }
        } else {
            self.divider(out, '-')?;
            self.line(
                out,
                &format!(
                    "Database Comparison: {}",
                    self.text(&comparison.db_path.display().to_string())
                ),
            )?;
            self.table(
                out,
                &["Field", "Replayed", "Persisted", "Status"],
                comparison.fields.iter().map(|f| {
                    vec![
                        f.name.clone(),
                        f.replayed_value.clone(),
                        f.persisted_value.clone(),
                        f.status.clone(),
                    ]
                }),
            )?;
        }
        for warning in &comparison.warnings {
            self.line(out, &format!("WARN: {}", self.text(warning)))?;
        }
        Ok(())
    }
}
