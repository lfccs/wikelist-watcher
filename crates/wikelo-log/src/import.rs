//! One-shot history import over `logbackups\*.log` (and optionally the live
//! `Game.log`). Each file is an independent session.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::parser::LineParser;
use crate::tracker::{MissionTracker, TrackerOutput};

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub files: usize,
    pub lines: u64,
    pub events: u64,
    pub wikelo_linked: usize,
    pub wikelo_completed: usize,
    pub offers: usize,
}

/// Backup logs sorted chronologically by the timestamp in their file name:
/// `Game Build(12881860) 12 Oct 26 (13 27 10).log`.
pub fn list_logbackups(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<(i64, PathBuf)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("log")))
        .map(|p| {
            let key = p
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(parse_backup_name)
                .or_else(|| std::fs::metadata(&p).and_then(|m| m.modified()).ok().map(|t| {
                    t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
                }))
                .unwrap_or(0);
            (key, p)
        })
        .collect();
    files.sort();
    files.into_iter().map(|(_, p)| p).collect()
}

/// `Game Build(12881860) 12 Oct 26 (13 27 10).log` -> sortable timestamp (seconds).
fn parse_backup_name(name: &str) -> Option<i64> {
    let rest = name.strip_prefix("Game Build(")?;
    let close = rest.find(')')?;
    let rest = rest[close + 1..].trim_start();
    // "12 Oct 26 (13 27 10).log"
    let mut parts = rest.split_whitespace();
    let day: i64 = parts.next()?.parse().ok()?;
    let mon = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: i64 = 2000 + parts.next()?.parse::<i64>().ok()?;
    let hms = rest[rest.find('(')? + 1..rest.rfind(')')?].split_whitespace().map(|x| x.parse::<i64>().unwrap_or(0)).collect::<Vec<_>>();
    let (h, m, s) = (hms.first().copied().unwrap_or(0), hms.get(1).copied().unwrap_or(0), hms.get(2).copied().unwrap_or(0));
    Some(((year * 12 + mon) * 31 + day) * 86_400 + h * 3600 + m * 60 + s)
}

/// Scan one log file from `start_offset`, feeding events to `sink`.
/// Returns (new offset, lines read, events emitted).
pub fn scan_file(
    path: &Path,
    start_offset: u64,
    parser: &LineParser,
    tracker: &mut MissionTracker,
    sink: &mut dyn FnMut(&Path, TrackerOutput),
) -> std::io::Result<(u64, u64, u64)> {
    let mut file = File::open(path)?;
    if start_offset > 0 {
        use std::io::Seek;
        file.seek(std::io::SeekFrom::Start(start_offset))?;
    }
    let mut reader = BufReader::with_capacity(1 << 16, file);
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut offset = start_offset;
    let (mut lines, mut events) = (0u64, 0u64);
    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 {
            break;
        }
        offset += n as u64;
        lines += 1;
        if !buf.ends_with(b"\n") {
            // Partial last line (file still being written): do not consume it.
            offset -= n as u64;
            break;
        }
        if buf.len() < 30 {
            continue;
        }
        let line = String::from_utf8_lossy(&buf);
        if let Some(ev) = parser.parse(&line) {
            for out in tracker.apply(ev) {
                events += 1;
                sink(path, out);
            }
        }
    }
    Ok((offset, lines, events))
}

/// Import every backup log (and the current `Game.log` if given) in order.
pub fn import_history(
    logbackups_dir: &Path,
    current_log: Option<&Path>,
    skip: &dyn Fn(&Path) -> bool,
    progress: &mut dyn FnMut(usize, usize, &Path),
    sink: &mut dyn FnMut(&Path, TrackerOutput),
) -> ImportReport {
    let parser = LineParser::new();
    let mut files = list_logbackups(logbackups_dir);
    if let Some(cur) = current_log
        && cur.is_file()
    {
        files.push(cur.to_path_buf());
    }
    let total = files.len();
    let mut report = ImportReport::default();
    for (i, path) in files.iter().enumerate() {
        progress(i, total, path);
        if skip(path) {
            continue;
        }
        let mut tracker = MissionTracker::new();
        let mut counting = |p: &Path, out: TrackerOutput| {
            match &out {
                TrackerOutput::MissionLinked { .. } => report.wikelo_linked += 1,
                TrackerOutput::MissionEnded { mission } if mission.completion.as_ref().is_some_and(|c| c.is_complete()) => {
                    report.wikelo_completed += 1
                }
                TrackerOutput::ContractOffered { .. } => report.offers += 1,
                _ => {}
            }
            sink(p, out);
        };
        match scan_file(path, 0, &parser, &mut tracker, &mut counting) {
            Ok((_, lines, events)) => {
                report.files += 1;
                report.lines += lines;
                report.events += events;
            }
            Err(e) => tracing::warn!("skipping {}: {e}", path.display()),
        }
    }
    progress(total, total, Path::new(""));
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_names_sort_chronologically() {
        let a = parse_backup_name("Game Build(10007308) 02 Aug 25 (14 41 12).log").unwrap();
        let b = parse_backup_name("Game Build(12881860) 12 Oct 26 (13 27 10).log").unwrap();
        let c = parse_backup_name("Game Build(12881860) 12 Oct 26 (13 27 11).log").unwrap();
        assert!(a < b && b < c);
        assert!(parse_backup_name("random.log").is_none());
    }
}
