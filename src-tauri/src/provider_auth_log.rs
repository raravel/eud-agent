//! Diagnostic log of provider sign-in and status decisions, so a "로그인 필요" the user saw can
//! be explained afterwards.
//!
//! One line per event in `%LOCALAPPDATA%\eud-agent\logs\provider-auth.log` (UTF-8 without BOM),
//! rotated to `provider-auth.log.1` past `MAX_LOG_BYTES`. Callers pass facts only (exit codes,
//! status fields, file presence, CLI output lines); tokens and email addresses are never handed
//! to it, and every line is passed through `redact` before it is written.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::DataDirs;

const LOG_FILENAME: &str = "provider-auth.log";
const ROTATED_FILENAME: &str = "provider-auth.log.1";
const MAX_LOG_BYTES: u64 = 512 * 1024;
const MAX_LINE_CHARS: usize = 400;

static WRITE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Append one event. A failure to log never changes the caller's outcome.
pub fn record(dirs: &DataDirs, provider: &str, event: &str) {
    let _guard = WRITE_LOCK.lock();
    let dir = dirs.logs_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(LOG_FILENAME);
    rotate_if_full(&path, &dir.join(ROTATED_FILENAME));
    let line = format!(
        "{} [{provider}] {}\n",
        timestamp(SystemTime::now()),
        redact(event)
    );
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(line.as_bytes());
    }
}

fn rotate_if_full(path: &Path, rotated: &Path) {
    if fs::metadata(path).is_ok_and(|metadata| metadata.len() >= MAX_LOG_BYTES) {
        let _ = fs::remove_file(rotated);
        let _ = fs::rename(path, rotated);
    }
}

/// One line, bounded, with URL query strings and email addresses removed and control
/// characters escaped.
pub(crate) fn redact(text: &str) -> String {
    let mut words = Vec::new();
    for word in text.split(' ') {
        let word = if word.contains("://") {
            word.split(['?', '#'])
                .next()
                .unwrap_or_default()
                .to_string()
                + "?…"
        } else if word.contains('@') && word.contains('.') {
            "<email>".to_string()
        } else {
            word.to_string()
        };
        words.push(word);
    }
    let joined = words.join(" ");
    let mut out = String::new();
    for ch in joined.chars() {
        if out.chars().count() >= MAX_LINE_CHARS {
            out.push('…');
            break;
        }
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push(' '),
            ch if ch.is_control() => out.push_str(&format!("\\u{{{:04x}}}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ`, UTC.
fn timestamp(now: SystemTime) -> String {
    let millis = now
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default();
    let days = millis.div_euclid(86_400_000);
    let of_day = millis.rem_euclid(86_400_000);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        of_day / 3_600_000,
        of_day / 60_000 % 60,
        of_day / 1000 % 60,
        of_day % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn redact_strips_query_strings_and_emails() {
        let line = redact(
            "visit https://claude.ai/oauth/authorize?code=true&state=secret as someone@example.com",
        );
        assert_eq!(line, "visit https://claude.ai/oauth/authorize?… as <email>");
        assert!(!line.contains("secret"));
        assert_eq!(redact("a\nb\u{1b}"), "a\\nb\\u{001b}");
        assert!(redact(&"x".repeat(1000)).chars().count() <= MAX_LINE_CHARS + 1);
    }

    #[test]
    fn timestamp_is_utc_iso() {
        let at = UNIX_EPOCH + Duration::from_millis(1_794_406_345_232);
        assert_eq!(timestamp(at), "2026-11-11T14:12:25.232Z");
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn record_appends_and_rotates() {
        let base = std::env::temp_dir().join(format!("eud-auth-log-{}", uuid::Uuid::new_v4()));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        record(&dirs, "claude-code", "first");
        record(&dirs, "claude-code", "second");
        let path = dirs.logs_dir().join(LOG_FILENAME);
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.lines().all(|line| line.contains("[claude-code]")));
        fs::write(&path, vec![b'x'; MAX_LOG_BYTES as usize]).unwrap();
        record(&dirs, "claude-code", "after rotation");
        assert!(dirs.logs_dir().join(ROTATED_FILENAME).is_file());
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 1);
        fs::remove_dir_all(base).ok();
    }
}
