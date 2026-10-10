//! "Record while I play" (audit 2026-10-04, section 6, "Proof, made one
//! click"): the game watcher records one side of a comparison by itself while
//! that comparison's game runs, so a before/after needs no clicks in a game.
//!
//! The schedule is PeakTweaks' own choice, not a measured one: a first
//! 30-second sample two minutes after the game starts (past loading and
//! shader compiling), then one every three minutes, each only while the
//! comparison's program is the window in front, until the side has three
//! runs. The game may close in between; the next time it runs, recording
//! carries on. Samples land wherever the player happens to be (a menu or a
//! match), so the spread between them is wider than with scenes picked by
//! hand, and the verdict, which only calls a difference real when it is
//! bigger than that spread, says "no measurable change" more often. That is
//! the honest cost of no clicks, and the Proof page says so.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::service::ProofService;
use super::store::Side;

/// `EngineError::Command::what` when a comparison cannot be recorded this way.
pub const AUTO_WHAT: &str = "Record while I play";

/// Runs a side gets before recording stops by itself (the guide's three).
pub const AUTO_RUNS: u32 = 3;
/// The length of each sample.
pub const AUTO_SECONDS: u32 = 30;
/// The first sample waits this long after the game starts.
pub const AUTO_FIRST_AFTER_MS: u64 = 120_000;
/// Later samples wait this long after the one before ended.
pub const AUTO_GAP_MS: u64 = 180_000;

/// Which side of which comparison the watcher records. Kept in the proof
/// folder so it outlasts a restart; one at a time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoRecord {
    pub session_id: String,
    pub side: Side,
}

/// What the app shows about it (`PlayStatus::auto_record`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AutoRecordStatus {
    pub session_id: String,
    pub side: Side,
    /// The comparison's game (`env::KNOWN_GAMES` id) and its program.
    pub game_id: String,
    pub exe: String,
    /// Runs on that side so far, and how many recording stops at.
    pub recorded: u32,
    pub wanted: u32,
    /// A sample is being recorded now.
    pub recording_now: bool,
    /// While the game runs and no sample is being recorded: when the next
    /// one is due (Unix ms).
    #[ts(type = "number | null")]
    pub next_unix_ms: Option<u64>,
    /// Why the next sample waits (the game not in front, other long work), or
    /// why the last one did not record, in plain words.
    pub problem: Option<String>,
}

/// When the next sample is due: two minutes after the game started, and at
/// least three minutes after the last one ended.
pub fn next_sample_at(game_started_ms: u64, last_ended_ms: Option<u64>) -> u64 {
    let first = game_started_ms.saturating_add(AUTO_FIRST_AFTER_MS);
    match last_ended_ms {
        Some(ended) => first.max(ended.saturating_add(AUTO_GAP_MS)),
        None => first,
    }
}

/// The comparison's program is the window in front (any case).
pub fn in_front(foreground: Option<&str>, exe: &str) -> bool {
    foreground.is_some_and(|f| f.eq_ignore_ascii_case(exe))
}

/// The watcher's side of recording while playing, between looks: when the
/// comparison's game started, when the last sample ended, whether one is being
/// recorded and why the last one failed. The app's watcher takes the samples
/// (on a thread of their own, as the Proof page's Record button does) and
/// tells this when one starts and ends.
#[derive(Debug, Default)]
pub struct AutoSchedule {
    /// The record the times belong to; another one starts afresh.
    record: Option<AutoRecord>,
    game_started_ms: Option<u64>,
    last_ended_ms: Option<u64>,
    recording: bool,
    failed: Option<String>,
}

impl AutoSchedule {
    /// One look: what to show, and whether a sample is due now. Stops
    /// recording once the side has its runs or the comparison cannot be read.
    /// `game` is the game the watcher sees running (`env::KNOWN_GAMES` id).
    pub fn look(&mut self, svc: &ProofService, game: Option<&str>, now_ms: u64) -> (Option<AutoRecordStatus>, bool) {
        let record = svc.auto_record();
        if record != self.record {
            *self = Self {
                record: record.clone(),
                recording: self.recording,
                ..Self::default()
            };
        }
        if record.is_none() {
            return (None, false);
        }
        let mut status = match svc.auto_record_status() {
            Ok(Some(status)) => status,
            Ok(None) => return (None, false),
            Err(_) => {
                // The comparison can no longer be read: nothing to record into.
                let _ = svc.stop_auto_record();
                return (None, false);
            }
        };
        if status.recorded >= status.wanted && !self.recording {
            // The side has its runs; the Proof page shows them.
            let _ = svc.stop_auto_record();
            *self = Self::default();
            return (None, false);
        }
        status.recording_now = self.recording;
        status.problem = self.failed.clone();
        let mut due = false;
        if game == Some(status.game_id.as_str()) {
            let started = *self.game_started_ms.get_or_insert(now_ms);
            if !self.recording {
                let next = next_sample_at(started, self.last_ended_ms);
                status.next_unix_ms = Some(next);
                due = now_ms >= next;
            }
        } else {
            self.game_started_ms = None;
            self.last_ended_ms = None;
        }
        (Some(status), due)
    }

    /// The watcher started the sample `look` said was due.
    pub fn sample_started(&mut self) {
        self.recording = true;
        self.failed = None;
    }

    /// The sample ended: `Err` carries why it did not record. One the game's
    /// closing cut short says nothing new, so it is not shown.
    pub fn sample_ended(&mut self, result: std::result::Result<(), String>, game_still_running: bool, now_ms: u64) {
        self.recording = false;
        self.last_ended_ms = Some(now_ms);
        self.failed = match result {
            Ok(()) => None,
            Err(_) if !game_still_running => None,
            Err(why) => Some(format!("The last sample did not record: {why}")),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_sample_waits_two_minutes_and_later_ones_three_after_the_last() {
        let start = 1_000_000;
        assert_eq!(next_sample_at(start, None), start + 120_000);
        // A sample that ended at 2:30 puts the next at 5:30.
        assert_eq!(next_sample_at(start, Some(start + 150_000)), start + 330_000);
        // An end time from before this game started never brings it forward.
        assert_eq!(next_sample_at(start, Some(start - 600_000)), start + 120_000);
        assert_eq!(next_sample_at(u64::MAX - 5, Some(u64::MAX - 5)), u64::MAX);
    }

    mod schedule {
        use std::sync::Arc;

        use super::*;
        use crate::proof::capture::FakeCapture;
        use crate::proof::service::BeginSession;
        use crate::types::Tier;

        const T0: u64 = 1_800_000_000_000;

        fn fortnite(dir: &std::path::Path) -> (ProofService, String) {
            let svc = ProofService::new(dir.join("proof"), Arc::new(FakeCapture::new(|_, _| vec![10.0; 300])));
            let session = svc
                .begin_session(
                    BeginSession {
                        exe: crate::games::facts("fortnite").unwrap().programs[0].into(),
                        game_id: Some("fortnite".into()),
                        game_build: None,
                    },
                    None,
                    Tier::Pro,
                )
                .unwrap();
            (svc, session.session_id)
        }

        fn sample(svc: &ProofService, id: &str, side: Side) {
            svc.capture(id, side, AUTO_SECONDS, 0, vec![], &|_, _| {}).unwrap();
        }

        #[test]
        fn nothing_set_shows_nothing_and_is_never_due() {
            let d = tempfile::tempdir().unwrap();
            let (svc, _) = fortnite(d.path());
            let mut s = AutoSchedule::default();
            assert_eq!(s.look(&svc, Some("fortnite"), T0), (None, false));
        }

        #[test]
        fn samples_wait_for_the_game_two_minutes_then_three_after_each_until_the_side_is_full() {
            let d = tempfile::tempdir().unwrap();
            let (svc, id) = fortnite(d.path());
            svc.start_auto_record(&id, Side::Before).unwrap();
            let mut s = AutoSchedule::default();

            // Not playing: shown, nothing due, no time given.
            let (status, due) = s.look(&svc, None, T0);
            let status = status.unwrap();
            assert!(!due);
            assert_eq!(
                (status.recorded, status.next_unix_ms, status.recording_now),
                (0, None, false)
            );
            // Another game running is not this comparison's.
            assert!(!s.look(&svc, Some("valorant"), T0).1);

            // The game starts: the first sample two minutes later.
            let (status, due) = s.look(&svc, Some("fortnite"), T0 + 1_000);
            assert!(!due);
            assert_eq!(status.unwrap().next_unix_ms, Some(T0 + 1_000 + AUTO_FIRST_AFTER_MS));
            assert!(!s.look(&svc, Some("fortnite"), T0 + 120_999).1);
            assert!(s.look(&svc, Some("fortnite"), T0 + 121_000).1);

            for n in 1..=AUTO_RUNS {
                s.sample_started();
                let (status, due) = s.look(&svc, Some("fortnite"), T0 + 125_000);
                let status = status.unwrap();
                assert!(!due && status.recording_now && status.next_unix_ms.is_none());
                sample(&svc, &id, Side::Before);
                let ended = T0 + 200_000 * u64::from(n);
                s.sample_ended(Ok(()), true, ended);
                if n < AUTO_RUNS {
                    let (status, due) = s.look(&svc, Some("fortnite"), ended + 1);
                    let status = status.unwrap();
                    assert!(!due);
                    assert_eq!((status.recorded, status.next_unix_ms), (n, Some(ended + AUTO_GAP_MS)));
                    assert!(s.look(&svc, Some("fortnite"), ended + AUTO_GAP_MS).1);
                }
            }
            // The side has its three runs: recording stops by itself.
            assert_eq!(s.look(&svc, Some("fortnite"), T0 + 900_000), (None, false));
            assert_eq!(svc.auto_record(), None);
        }

        #[test]
        fn a_game_closed_and_started_again_waits_two_minutes_again() {
            let d = tempfile::tempdir().unwrap();
            let (svc, id) = fortnite(d.path());
            svc.start_auto_record(&id, Side::After).unwrap();
            let mut s = AutoSchedule::default();
            s.look(&svc, Some("fortnite"), T0);
            s.sample_started();
            s.sample_ended(Ok(()), true, T0 + 150_000);
            s.look(&svc, None, T0 + 160_000);
            let (status, _) = s.look(&svc, Some("fortnite"), T0 + 400_000);
            assert_eq!(status.unwrap().next_unix_ms, Some(T0 + 400_000 + AUTO_FIRST_AFTER_MS));
        }

        #[test]
        fn a_failed_sample_is_shown_unless_the_game_closing_cut_it_short() {
            let d = tempfile::tempdir().unwrap();
            let (svc, id) = fortnite(d.path());
            svc.start_auto_record(&id, Side::Before).unwrap();
            let mut s = AutoSchedule::default();
            s.look(&svc, Some("fortnite"), T0);
            s.sample_started();
            s.sample_ended(Err("PresentMon wrote no data".into()), true, T0 + 150_000);
            let (status, due) = s.look(&svc, Some("fortnite"), T0 + 150_001);
            let status = status.unwrap();
            assert!(!due, "a failure waits like a sample does");
            assert_eq!(
                status.problem.as_deref(),
                Some("The last sample did not record: PresentMon wrote no data")
            );
            assert_eq!(status.recorded, 0);
            // The next one starting clears it.
            s.sample_started();
            assert_eq!(s.look(&svc, Some("fortnite"), T0 + 330_000).0.unwrap().problem, None);
            s.sample_ended(Err("too few frames".into()), false, T0 + 360_000);
            assert_eq!(s.look(&svc, None, T0 + 360_001).0.unwrap().problem, None);
        }

        #[test]
        fn another_comparison_starts_afresh_and_a_sample_in_flight_still_ends() {
            let d = tempfile::tempdir().unwrap();
            let (svc, first) = fortnite(d.path());
            svc.start_auto_record(&first, Side::Before).unwrap();
            let mut s = AutoSchedule::default();
            s.look(&svc, Some("fortnite"), T0);
            s.sample_started();
            let second = svc
                .begin_session(
                    BeginSession {
                        exe: crate::games::facts("fortnite").unwrap().programs[0].into(),
                        game_id: Some("fortnite".into()),
                        game_build: None,
                    },
                    None,
                    Tier::Pro,
                )
                .unwrap()
                .session_id;
            svc.start_auto_record(&second, Side::Before).unwrap();
            let (status, due) = s.look(&svc, Some("fortnite"), T0 + 200_000);
            let status = status.unwrap();
            assert_eq!(status.session_id, second);
            assert!(
                status.recording_now && !due,
                "the first comparison's sample is still running"
            );
            s.sample_ended(Ok(()), true, T0 + 210_000);
            let (status, _) = s.look(&svc, Some("fortnite"), T0 + 210_001);
            assert_eq!(status.unwrap().next_unix_ms, Some(T0 + 210_000 + AUTO_GAP_MS));

            // Stopped from the Proof page: nothing shown, nothing due.
            svc.stop_auto_record().unwrap();
            assert_eq!(s.look(&svc, Some("fortnite"), T0 + 900_000), (None, false));
        }
    }

    #[test]
    fn only_the_comparisons_own_program_in_front_counts() {
        assert!(in_front(
            Some("FortniteClient-Win64-Shipping.exe"),
            "fortniteclient-win64-shipping.exe"
        ));
        assert!(!in_front(Some("Discord.exe"), "FortniteClient-Win64-Shipping.exe"));
        assert!(!in_front(Some("r5apex_dx12.exe"), "r5apex.exe"));
        assert!(!in_front(None, "r5apex.exe"));
    }
}
