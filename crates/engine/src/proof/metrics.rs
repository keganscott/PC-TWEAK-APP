//! Frame-time metrics from a PresentMon CSV.
//!
//! Lows are computed from **frame times**, never from averaged FPS: averaging
//! per-second FPS hides exactly the stutter that lows exist to show. The
//! definitions are written down here and echoed in every `FrameStats` so a
//! number can be reproduced by hand:
//!
//! * **average FPS** = frames / total frame time.
//! * **1% low FPS** = 1000 / mean of the slowest 1% of frame times (at least one frame).
//! * **0.1% low FPS** = the same over the slowest 0.1% (at least one frame).
//! * **percentiles** use the nearest-rank method.
//! * **stutter** = a frame slower than twice the median of the previous 60
//!   frames *and* at least 20 ms, so a 2 ms -> 5 ms blip at 400 FPS is not
//!   counted.
//!
//! Frame time is `MsBetweenPresents`, present-to-present, including frames the
//! display never showed (so tearing/dropping cannot hide a hitch).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{EngineError, Result};

/// Fewer frames than this and the numbers are noise; refuse to compute them.
pub const MIN_FRAMES: usize = 60;

const STUTTER_WINDOW: usize = 60;
const STUTTER_FACTOR: f64 = 2.0;
const STUTTER_MIN_MS: f64 = 20.0;

pub const DEFINITIONS: &str = "average = frames / total frame time; 1% low = 1000 / mean of the slowest 1% of frame \
     times; 0.1% low likewise; percentiles nearest-rank; stutter = frame > 2x the median of the previous 60 frames \
     and >= 20 ms; frame time = MsBetweenPresents";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct FrameStats {
    pub frames: u32,
    pub duration_seconds: f64,
    pub avg_fps: f64,
    pub one_percent_low_fps: f64,
    pub point_one_percent_low_fps: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub p999_ms: f64,
    pub max_ms: f64,
    pub stutters: u32,
    /// How these numbers were computed. Same text for every run.
    pub definitions: String,
}

/// Frame times pulled from a CSV, plus what was set aside.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameTimes {
    pub ms: Vec<f64>,
    /// Rows from swap chains other than the busiest one (overlays, launchers).
    pub ignored_rows: usize,
    /// Rows whose frame time was missing (`NA`), zero or negative.
    pub unusable_rows: usize,
}

/// Split one CSV line, honouring double quotes (an exe name may contain a comma).
fn split_csv(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn column(header: &[String], name: &str) -> Option<usize> {
    header.iter().position(|h| h.trim().eq_ignore_ascii_case(name))
}

/// Parse frame times from PresentMon CSV text.
///
/// If a process presents on several swap chains (a game plus an overlay), only
/// the swap chain with the most frames is used: that is the game's render loop.
pub fn parse_frame_times(csv: &str) -> Result<FrameTimes> {
    let bad = |detail: String| EngineError::Command {
        what: "Read PresentMon CSV".into(),
        exit_code: None,
        detail,
    };
    let mut lines = csv.trim_start_matches('\u{feff}').lines();
    let header = split_csv(lines.next().ok_or_else(|| bad("the file is empty".into()))?);
    let time_col = column(&header, "MsBetweenPresents").ok_or_else(|| {
        bad(format!(
            "no MsBetweenPresents column (found: {})",
            header.iter().take(12).cloned().collect::<Vec<_>>().join(", ")
        ))
    })?;
    let chain_col = column(&header, "SwapChainAddress");

    let mut by_chain: std::collections::HashMap<String, Vec<f64>> = std::collections::HashMap::new();
    let mut unusable = 0usize;
    for line in lines.filter(|l| !l.trim().is_empty()) {
        let fields = split_csv(line);
        let chain = chain_col.and_then(|c| fields.get(c)).cloned().unwrap_or_default();
        match fields.get(time_col).and_then(|v| v.trim().parse::<f64>().ok()) {
            Some(ms) if ms.is_finite() && ms > 0.0 => by_chain.entry(chain).or_default().push(ms),
            _ => unusable += 1,
        }
    }
    let total: usize = by_chain.values().map(Vec::len).sum();
    let (_, ms) = by_chain
        .into_iter()
        .max_by(|a, b| a.1.len().cmp(&b.1.len()).then_with(|| b.0.cmp(&a.0)))
        .ok_or_else(|| bad("the file has no usable frames; was the game running and presenting?".into()))?;
    let ignored_rows = total - ms.len();
    Ok(FrameTimes {
        ms,
        ignored_rows,
        unusable_rows: unusable,
    })
}

fn nearest_rank(sorted: &[f64], p: f64) -> f64 {
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

fn low_fps(sorted_desc: &[f64], fraction: f64) -> f64 {
    let n = ((fraction * sorted_desc.len() as f64).ceil() as usize).clamp(1, sorted_desc.len());
    let mean: f64 = sorted_desc[..n].iter().sum::<f64>() / n as f64;
    1000.0 / mean
}

fn median(window: &[f64]) -> f64 {
    let mut w = window.to_vec();
    w.sort_by(|a, b| a.total_cmp(b));
    let mid = w.len() / 2;
    if w.len() % 2 == 1 {
        w[mid]
    } else {
        (w[mid - 1] + w[mid]) / 2.0
    }
}

pub fn count_stutters(ms: &[f64]) -> u32 {
    let mut count = 0;
    for i in STUTTER_WINDOW..ms.len() {
        let base = median(&ms[i - STUTTER_WINDOW..i]);
        if ms[i] > STUTTER_FACTOR * base && ms[i] >= STUTTER_MIN_MS {
            count += 1;
        }
    }
    count
}

pub fn compute_stats(ms: &[f64]) -> Result<FrameStats> {
    if ms.len() < MIN_FRAMES {
        return Err(EngineError::Command {
            what: "Compute frame statistics".into(),
            exit_code: None,
            detail: format!(
                "only {} usable frames were captured; at least {MIN_FRAMES} are needed. Was the game running, \
                 in the foreground, and drawing frames?",
                ms.len()
            ),
        });
    }
    let total_ms: f64 = ms.iter().sum();
    let mut desc = ms.to_vec();
    desc.sort_by(|a, b| b.total_cmp(a));
    let mut asc = desc.clone();
    asc.reverse();

    Ok(FrameStats {
        frames: ms.len() as u32,
        duration_seconds: total_ms / 1000.0,
        avg_fps: 1000.0 * ms.len() as f64 / total_ms,
        one_percent_low_fps: low_fps(&desc, 0.01),
        point_one_percent_low_fps: low_fps(&desc, 0.001),
        p50_ms: nearest_rank(&asc, 0.50),
        p95_ms: nearest_rank(&asc, 0.95),
        p99_ms: nearest_rank(&asc, 0.99),
        p999_ms: nearest_rank(&asc, 0.999),
        max_ms: desc[0],
        stutters: count_stutters(ms),
        definitions: DEFINITIONS.to_owned(),
    })
}

/// CSV text to statistics in one step.
pub fn stats_from_csv(csv: &str) -> Result<FrameStats> {
    compute_stats(&parse_frame_times(csv)?.ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csv(rows: &[(&str, f64)]) -> String {
        let mut s = String::from("Application,ProcessID,SwapChainAddress,MsBetweenPresents,DisplayedTime\n");
        for (chain, ms) in rows {
            s.push_str(&format!("game.exe,100,{chain},{ms},{ms}\n"));
        }
        s
    }

    fn steady(n: usize, ms: f64) -> Vec<f64> {
        vec![ms; n]
    }

    #[test]
    fn a_perfectly_steady_60_fps_has_no_lows_below_the_average_and_no_stutter() {
        let s = compute_stats(&steady(600, 1000.0 / 60.0)).unwrap();
        assert!((s.avg_fps - 60.0).abs() < 1e-9);
        assert!((s.one_percent_low_fps - 60.0).abs() < 1e-9);
        assert!((s.point_one_percent_low_fps - 60.0).abs() < 1e-9);
        assert_eq!(s.stutters, 0);
        assert!((s.p50_ms - 16.666_666).abs() < 1e-3);
        assert_eq!(s.frames, 600);
        assert!((s.duration_seconds - 10.0).abs() < 1e-9);
    }

    #[test]
    fn lows_come_from_frame_times_not_from_the_average() {
        // 990 frames at 10 ms (100 FPS) and 10 frames at 50 ms (20 FPS).
        let mut ms = steady(990, 10.0);
        ms.extend(steady(10, 50.0));
        let s = compute_stats(&ms).unwrap();
        // average FPS = 1000 * 1000 / (9900 + 500) = 96.15
        assert!((s.avg_fps - 96.153_846).abs() < 1e-3, "{}", s.avg_fps);
        // slowest 1% = 10 frames, all 50 ms -> 20 FPS
        assert!((s.one_percent_low_fps - 20.0).abs() < 1e-9, "{}", s.one_percent_low_fps);
        // slowest 0.1% = 1 frame of 50 ms -> 20 FPS
        assert!((s.point_one_percent_low_fps - 20.0).abs() < 1e-9);
        assert!((s.p99_ms - 10.0).abs() < 1e-9, "p99 is the 990th frame: {}", s.p99_ms);
        assert!((s.p999_ms - 50.0).abs() < 1e-9);
        assert!((s.max_ms - 50.0).abs() < 1e-9);
    }

    #[test]
    fn the_worst_frames_are_averaged_for_the_1_percent_low() {
        // 1000 frames: 10 slow ones of 30..48 ms (mean 39), the rest 10 ms.
        let mut ms = steady(990, 10.0);
        ms.extend((0..10).map(|i| 30.0 + 2.0 * i as f64));
        let s = compute_stats(&ms).unwrap();
        assert!(
            (s.one_percent_low_fps - 1000.0 / 39.0).abs() < 1e-9,
            "{}",
            s.one_percent_low_fps
        );
    }

    #[test]
    fn stutters_need_to_be_both_relative_and_absolute() {
        // At 400 FPS (2.5 ms) a 6 ms frame is 2.4x the median but far under 20 ms: not a stutter.
        let mut fast = steady(200, 2.5);
        fast[100] = 6.0;
        assert_eq!(compute_stats(&fast).unwrap().stutters, 0);

        // At 60 FPS a 45 ms frame is 2.7x and above 20 ms: a stutter.
        let mut slow = steady(200, 16.7);
        slow[100] = 45.0;
        assert_eq!(compute_stats(&slow).unwrap().stutters, 1);

        // A frame at 30 ms in a 16.7 ms stream is only 1.8x: not a stutter.
        let mut mild = steady(200, 16.7);
        mild[100] = 30.0;
        assert_eq!(compute_stats(&mild).unwrap().stutters, 0);

        // The first 60 frames have no history to compare against.
        let mut early = steady(200, 16.7);
        early[10] = 90.0;
        assert_eq!(compute_stats(&early).unwrap().stutters, 0);
    }

    #[test]
    fn too_few_frames_is_an_error_not_a_number() {
        let e = compute_stats(&steady(59, 16.0)).unwrap_err();
        assert!(
            matches!(&e, EngineError::Command { detail, .. } if detail.contains("59")),
            "{e:?}"
        );
        assert!(compute_stats(&steady(60, 16.0)).is_ok());
    }

    #[test]
    fn csv_parsing_skips_na_zero_and_negative_and_reports_them() {
        let text = "Application,SwapChainAddress,MsBetweenPresents\n\
                    game.exe,0x1,16.6\ngame.exe,0x1,NA\ngame.exe,0x1,0\ngame.exe,0x1,-3\ngame.exe,0x1,17.0\n";
        let f = parse_frame_times(text).unwrap();
        assert_eq!(f.ms, vec![16.6, 17.0]);
        assert_eq!(f.unusable_rows, 3);
    }

    #[test]
    fn only_the_busiest_swap_chain_counts() {
        let mut rows: Vec<(&str, f64)> = (0..100).map(|_| ("0xAAA", 16.6)).collect();
        rows.extend((0..30).map(|_| ("0xBBB", 100.0))); // an overlay
        let f = parse_frame_times(&csv(&rows)).unwrap();
        assert_eq!(f.ms.len(), 100);
        assert_eq!(f.ignored_rows, 30);
    }

    #[test]
    fn header_matching_is_case_insensitive_and_tolerates_a_bom_and_quotes() {
        let text = "\u{feff}\"Application\",\"msbetweenpresents\"\n\"my,game.exe\",16.0\n\"other.exe\",17.0\n";
        let f = parse_frame_times(text).unwrap();
        assert_eq!(f.ms, vec![16.0, 17.0]);
    }

    #[test]
    fn a_csv_without_the_frame_time_column_names_what_it_found() {
        let e = parse_frame_times("Application,Other\ngame.exe,1\n").unwrap_err();
        assert!(
            matches!(&e, EngineError::Command { detail, .. } if detail.contains("MsBetweenPresents") && detail.contains("Other")),
            "{e:?}"
        );
        assert!(parse_frame_times("").is_err());
        assert!(parse_frame_times("MsBetweenPresents\n").is_err());
    }

    #[test]
    fn stats_serialize_with_stable_camel_case_keys_and_the_definitions() {
        let s = compute_stats(&steady(100, 10.0)).unwrap();
        let j = serde_json::to_value(&s).unwrap();
        for key in [
            "frames",
            "durationSeconds",
            "avgFps",
            "onePercentLowFps",
            "pointOnePercentLowFps",
            "p50Ms",
            "p95Ms",
            "p99Ms",
            "p999Ms",
            "maxMs",
            "stutters",
            "definitions",
        ] {
            assert!(j.get(key).is_some(), "missing key {key}: {j}");
        }
        assert!(j["definitions"].as_str().unwrap().contains("MsBetweenPresents"));
    }
    /// The one test that uses PresentMon's real output. The file is captured on
    /// a real desktop by `scripts/capture-presentmon-fixture.ps1` and committed;
    /// until then this only says so (NOTES.md N39), it does not pass silently.
    #[test]
    fn a_real_presentmon_file_parses_and_gives_plausible_numbers() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/presentmon-real.csv");
        let Ok(text) = std::fs::read_to_string(&path) else {
            println!("NOT VERIFIED: {} is not committed yet (NOTES.md N39)", path.display());
            return;
        };
        println!("header: {}", text.lines().next().unwrap_or(""));
        let frames = parse_frame_times(&text).expect("a real PresentMon file must parse");
        println!(
            "frames {} (ignored {}, unusable {})",
            frames.ms.len(),
            frames.ignored_rows,
            frames.unusable_rows
        );
        assert!(
            frames.ms.len() >= MIN_FRAMES,
            "the fixture must hold at least {MIN_FRAMES} frames"
        );
        assert!(
            frames.unusable_rows * 10 <= frames.ms.len(),
            "more than 10% of a real file was unusable: the column we read is probably wrong"
        );
        let stats = compute_stats(&frames.ms).unwrap();
        println!("{stats:#?}");
        // A screen shows something between about 1 and 1000 frames a second.
        assert!(
            (1.0..=1000.0).contains(&stats.avg_fps),
            "implausible average: {}",
            stats.avg_fps
        );
        assert!(
            stats.p50_ms > 0.5 && stats.p50_ms < 1000.0,
            "implausible median: {}",
            stats.p50_ms
        );
        assert!(stats.one_percent_low_fps <= stats.avg_fps + 1e-9);
    }
}
