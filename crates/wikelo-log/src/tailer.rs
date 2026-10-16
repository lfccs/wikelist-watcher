//! Polling tailer for the live `Game.log` with rotation detection.
//!
//! Star Citizen recreates `Game.log` on every launch (the previous one moves
//! to `logbackups\`). Polling every few hundred milliseconds is simpler and
//! more reliable on Windows than filesystem notifications.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::import::scan_file;
use crate::parser::LineParser;
use crate::tracker::{MissionTracker, TrackerOutput};

#[derive(Debug, Clone)]
pub struct TailerConfig {
    pub path: PathBuf,
    /// Replay the whole current file on start (recovers active missions).
    pub from_start: bool,
    pub poll_interval: Duration,
}

pub struct Tailer {
    cfg: TailerConfig,
    parser: LineParser,
    tracker: MissionTracker,
    offset: u64,
    /// First bytes of the file; a change means the game recreated Game.log.
    signature: Option<Vec<u8>>,
}

const SIGNATURE_LEN: usize = 160;

fn read_signature(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; SIGNATURE_LEN];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);
    Some(buf)
}

impl Tailer {
    pub fn new(cfg: TailerConfig) -> Self {
        Self { cfg, parser: LineParser::new(), tracker: MissionTracker::new(), offset: 0, signature: None }
    }

    pub fn tracker(&self) -> &MissionTracker {
        &self.tracker
    }

    /// Run until `stop` is set. `sink` receives (is_replay, output).
    pub fn run(&mut self, stop: Arc<AtomicBool>, sink: &mut dyn FnMut(bool, TrackerOutput)) {
        self.run_batched(stop, &mut |batch| {
            for (replay, out) in batch {
                sink(replay, out);
            }
        });
    }

    /// Like [`run`](Self::run), but `sink` is called once per poll with everything that
    /// poll produced (possibly nothing), which lets a remote watcher batch and heartbeat.
    pub fn run_batched(&mut self, stop: Arc<AtomicBool>, sink: &mut dyn FnMut(Vec<(bool, TrackerOutput)>)) {
        let mut replaying = true;
        if !self.cfg.from_start {
            self.offset = std::fs::metadata(&self.cfg.path).map(|m| m.len()).unwrap_or(0);
            replaying = false;
        }
        while !stop.load(Ordering::Relaxed) {
            let mut batch: Vec<(bool, TrackerOutput)> = Vec::new();
            match std::fs::metadata(&self.cfg.path) {
                Ok(meta) => {
                    let len = meta.len();
                    let sig = if len as usize >= SIGNATURE_LEN { read_signature(&self.cfg.path) } else { None };
                    let header_changed = matches!((&self.signature, &sig), (Some(a), Some(b)) if a != b);
                    if len < self.offset || header_changed {
                        // Rotation: file was recreated.
                        tracing::info!("Game.log rotated; restarting from the beginning");
                        self.offset = 0;
                        self.tracker.reset_session();
                        replaying = false;
                    }
                    if sig.is_some() {
                        self.signature = sig;
                    }
                    if len > self.offset {
                        let is_replay = replaying;
                        let mut forward = |_: &Path, out: TrackerOutput| batch.push((is_replay, out));
                        match scan_file(&self.cfg.path, self.offset, &self.parser, &mut self.tracker, &mut forward) {
                            Ok((new_offset, _, _)) => self.offset = new_offset,
                            Err(e) => tracing::warn!("tail read error: {e}"),
                        }
                    }
                    if replaying && self.offset >= len {
                        replaying = false;
                    }
                }
                Err(_) => {
                    // Game not running yet; keep waiting.
                    self.offset = 0;
                }
            }
            sink(batch);
            std::thread::sleep(self.cfg.poll_interval);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn tails_and_detects_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Game.log");
        let marker = "<2026-10-12T18:44:32.784Z> [Notice] <CLocalMissionPhaseMarker::CreateMarker> Creating objective marker: missionId [c8c64cab-0eda-49eb-8f2b-538e2d4db547], generator name [TheCollector], contract [TheCollector_Intro], contractDefinitionId[028d7a46-bd97-4e32-a5bf-0b1775f05e1c], objectiveId [o], markerEntityId [1], zoneHostId [1], position [x: 0, y: 0, z: 0] [Team_MissionFeatures][Missions]\n";
        let header = "<2026-10-13T21:34:02.356Z> BackupNameAttachment=\" Build(12913470) 13 Oct 26 (18 33 55)\"  -- used by backup system\n";
        std::fs::write(&path, format!("{header}{marker}")).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let p2 = path.clone();
        let handle = std::thread::spawn(move || {
            let mut t = Tailer::new(TailerConfig { path: p2, from_start: true, poll_interval: Duration::from_millis(20) });
            let mut got: Vec<(bool, TrackerOutput)> = Vec::new();
            t.run(stop2, &mut |r, o| got.push((r, o)));
            got
        });
        std::thread::sleep(Duration::from_millis(150));
        // Rotate: the game recreates the file with a new session header (new timestamp).
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(header.replace("21:34:02.356Z", "23:10:00.000Z").replace("18 33 55", "20 09 59").as_bytes()).unwrap();
            f.write_all(marker.replace("c8c64cab", "deadbeef").as_bytes()).unwrap();
        }
        std::thread::sleep(Duration::from_millis(200));
        stop.store(true, Ordering::Relaxed);
        let got = handle.join().unwrap();
        let linked: Vec<&(bool, TrackerOutput)> = got.iter().filter(|(_, o)| matches!(o, TrackerOutput::MissionLinked { .. })).collect();
        assert_eq!(linked.len(), 2, "{got:?}");
        assert!(linked[0].0, "first marker is a replay");
        assert!(!linked[1].0, "marker after rotation is live");
    }
}
