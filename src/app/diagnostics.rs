// SPDX-License-Identifier: MIT

//! The diagnostics report (spec §12): started by the user (View →
//! Diagnostics), shown in full before it is saved, and saved only as shown.
//!
//! It holds versions, the dependency check, the app's settings, a count
//! per account, and the end of the app's log. No tokens, codes, cookies,
//! connection files or feed responses: the log never has them (`log.rs`
//! redacts every line before it is written), and each line is redacted
//! again here. Account names, display names, desktop titles and workspace
//! names the app knows are replaced by placeholders (spec §12: sensitive
//! metadata is redacted by default).

use super::*;

/// Log lines at the end of the report.
const LOG_LINES: usize = 300;
/// Names shorter than this aren't replaced (they'd match ordinary words).
const MIN_NAME: usize = 3;

/// What the report is built from, gathered on the UI thread.
pub(super) struct Inputs {
    summary: Vec<String>,
    /// Sensitive names and their placeholders, longest first.
    names: Vec<(String, String)>,
}

impl AppModel {
    pub(super) fn diagnostics_inputs(&self) -> Inputs {
        let mut summary = vec![
            format!("Deskfeed {}", env!("CARGO_PKG_VERSION")),
            format!("Created {}", crate::app::diagnostics::now_label()),
            String::new(),
            "Dependencies:".to_owned(),
        ];
        match self.deps.as_deref() {
            None => summary.push("  (still checking)".to_owned()),
            Some(checks) => {
                for c in checks {
                    summary.push(format!("  {:?}: {:?}", c.dependency, c.status));
                }
            }
        }
        let app = settings::effective(Some(&self.config.app_settings), None, None);
        summary.push(String::new());
        summary.push(format!("App settings: {app:?}"));
        summary.push(format!(
            "Overrides: {} accounts, {} desktops; {} approved certificates; {} remembered passwords; {} favorites",
            self.config.account_settings.len(),
            self.config.desktop_settings.len(),
            self.config.certificate_pins.len(),
            self.config.saved_passwords.len(),
            self.config.favorites.len(),
        ));
        summary.push(String::new());
        let mut names = Vec::new();
        for (n, a) in self.accounts.iter().enumerate() {
            let label = format!("<account {}>", n + 1);
            for name in [a.identity.username.clone(), a.identity.display_name.clone()]
                .into_iter()
                .flatten()
            {
                names.push((name, label.clone()));
            }
            let state = match &a.state {
                AccountState::Restoring => "restoring",
                AccountState::NeedsSignIn { .. } => "needs sign-in",
                AccountState::SigningIn { .. } => "signing in",
                AccountState::Ready { .. } => "signed in",
            };
            let workspaces = a.workspaces();
            let desktops: usize = workspaces.iter().map(|w| w.resources.len()).sum();
            summary.push(format!(
                "{label}: {state}; {} workspaces, {desktops} desktops; offline list: {}",
                workspaces.len(),
                if a.cached.is_some() { "shown" } else { "no" },
            ));
            for (w, ws) in workspaces.iter().enumerate() {
                names.push((ws.name.clone(), format!("<workspace {}.{}>", n + 1, w + 1)));
                for (d, r) in ws.resources.iter().enumerate() {
                    names.push((
                        r.title.clone(),
                        format!("<desktop {}.{}.{}>", n + 1, w + 1, d + 1),
                    ));
                    if let Some(c) = self.connections.get(&r.key) {
                        summary.push(format!("  desktop {}.{}.{}: {c:?}", n + 1, w + 1, d + 1));
                    }
                }
            }
        }
        names.retain(|(name, _)| name.chars().count() >= MIN_NAME);
        // Longest first, so a title containing another name is replaced whole.
        names.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
        names.dedup_by(|a, b| a.0 == b.0);
        Inputs { summary, names }
    }
}

/// Builds the report. Blocking (reads the log); call from `spawn_blocking`.
pub(super) fn build(inputs: &Inputs) -> String {
    use std::fmt::Write as _;
    let mut out = inputs.summary.join("\n");
    out.push_str("\n\nSystem:\n");
    if let Ok(release) = std::fs::read_to_string("/etc/os-release")
        && let Some(name) = release.lines().find_map(|l| l.strip_prefix("PRETTY_NAME="))
    {
        let _ = writeln!(out, "  {}", name.trim_matches('"'));
    }
    if let Ok(kernel) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
        let _ = writeln!(out, "  Linux {}", kernel.trim());
    }
    let _ = write!(out, "\nLast {LOG_LINES} log lines:\n");
    for line in log_tail() {
        out.push_str("  ");
        out.push_str(&crate::log::redact(&line));
        out.push('\n');
    }
    redact_names(&out, &inputs.names)
}

/// The end of the app's log (the older file first if the current one is
/// short).
fn log_tail() -> Vec<String> {
    let Some(dir) = crate::log::dir() else {
        return Vec::new();
    };
    let mut lines: Vec<String> = Vec::new();
    for file in ["app.log.1", "app.log"] {
        if let Ok(text) = std::fs::read_to_string(dir.join(file)) {
            lines.extend(text.lines().map(str::to_owned));
        }
    }
    let skip = lines.len().saturating_sub(LOG_LINES);
    lines.split_off(skip)
}

/// Replaces each known name with its placeholder.
fn redact_names(text: &str, names: &[(String, String)]) -> String {
    let mut out = text.to_owned();
    for (name, placeholder) in names {
        out = out.replace(name.as_str(), placeholder);
    }
    out
}

/// Local date and time for the report's header.
pub(super) fn now_label() -> String {
    super::views::date_time_label(std::time::SystemTime::now())
}

/// Writes the report to `path`, private to the user.
pub(super) fn save(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_replaced_longest_first() {
        let names = vec![
            ("Finance Desktop".to_owned(), "<desktop 1.1.1>".to_owned()),
            ("Finance".to_owned(), "<workspace 1.1>".to_owned()),
        ];
        let text = "connect Finance Desktop in Finance";
        assert_eq!(
            redact_names(text, &names),
            "connect <desktop 1.1.1> in <workspace 1.1>"
        );
    }

    #[test]
    fn saved_privately() {
        use std::os::unix::fs::PermissionsExt as _;
        let path = std::env::temp_dir().join(format!("deskfeed-diag-{}.txt", std::process::id()));
        save(&path, "report").unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "report");
        std::fs::remove_file(path).unwrap();
    }

    /// A report from the real log: `cargo test --release real_report --
    /// --ignored --nocapture`.
    #[test]
    #[ignore = "reads the user's real log"]
    fn real_report() {
        println!(
            "{}",
            build(&Inputs {
                summary: vec!["(summary)".into()],
                names: Vec::new(),
            })
        );
    }
}
