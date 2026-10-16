//! Arbitrary text as Game.log lines through the parser and the mission state machine (the
//! watcher feeds whatever the game writes; the server replays what the watcher sends).
#![no_main]

use std::sync::LazyLock;

use libfuzzer_sys::fuzz_target;
use wikelo_log::{LineParser, MissionTracker, timestamp_of};

/// The parser compiles its regexes once; building it per input would cap the fuzzer at a
/// few runs per second.
static PARSER: LazyLock<LineParser> = LazyLock::new(LineParser::new);

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    let mut tracker = MissionTracker::new();
    for line in text.lines().take(256) {
        let _ = timestamp_of(line);
        if let Some(ev) = PARSER.parse(line) {
            let _ = tracker.apply(ev);
        }
    }
    let _ = tracker.active_wikelo();
});
