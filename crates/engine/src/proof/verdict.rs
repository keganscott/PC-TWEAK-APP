//! Deciding whether a change made a difference.
//!
//! The rule the plan asks for: a difference is only "Better" or "Worse" when it
//! is **larger than the run-to-run spread actually measured on this PC**. Run the
//! same scene several times before and several times after; the spread of the
//! repeats is how much the number moves with no change at all. A difference
//! inside that is noise and is reported as such.
//!
//! All user-facing verdict text comes from `compare`, so no screen can word a
//! result more strongly than the data allows.

use serde::Serialize;
use ts_rs::TS;

/// Each side needs at least this many runs, or there is no spread to compare against.
pub const MIN_RUNS_PER_SIDE: usize = 2;

/// Even with perfectly repeatable runs, a difference under this share of the
/// baseline is not called a change (0.3 FPS on a 100 FPS game is not a result).
pub const MIN_RELATIVE_DIFFERENCE: f64 = 0.01;

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub run_id: String,
    pub avg_fps: f64,
    pub one_percent_low_fps: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Better,
    NoMeasurableChange,
    Worse,
    NotEnoughData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    AvgFps,
    OnePercentLowFps,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct MetricComparison {
    pub metric: Metric,
    pub before_median: f64,
    pub after_median: f64,
    /// `after - before`; positive is faster.
    pub difference: f64,
    pub spread_before: f64,
    pub spread_after: f64,
    /// The difference must exceed this to count.
    pub threshold: f64,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub average: MetricComparison,
    pub lows: MetricComparison,
    pub overall: Verdict,
    /// The only sentence a screen should show about this result.
    pub headline: String,
    pub before_run_ids: Vec<String>,
    pub after_run_ids: Vec<String>,
    /// Reasons to trust these numbers less (for example a run where the GPU was
    /// held back by heat). Filled in by the service from stored run data.
    pub warnings: Vec<String>,
}

fn median(values: &[f64]) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let mid = v.len() / 2;
    if v.len() % 2 == 1 {
        v[mid]
    } else {
        (v[mid - 1] + v[mid]) / 2.0
    }
}

fn spread(values: &[f64]) -> f64 {
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    max - min
}

fn compare_metric(metric: Metric, before: &[f64], after: &[f64]) -> MetricComparison {
    if before.len() < MIN_RUNS_PER_SIDE || after.len() < MIN_RUNS_PER_SIDE {
        return MetricComparison {
            metric,
            before_median: before.first().map_or(0.0, |_| median(before)),
            after_median: after.first().map_or(0.0, |_| median(after)),
            difference: 0.0,
            spread_before: 0.0,
            spread_after: 0.0,
            threshold: 0.0,
            verdict: Verdict::NotEnoughData,
        };
    }
    let (before_median, after_median) = (median(before), median(after));
    let (spread_before, spread_after) = (spread(before), spread(after));
    let difference = after_median - before_median;
    let threshold = spread_before
        .max(spread_after)
        .max(MIN_RELATIVE_DIFFERENCE * before_median.abs());
    let verdict = if difference > threshold {
        Verdict::Better
    } else if difference < -threshold {
        Verdict::Worse
    } else {
        Verdict::NoMeasurableChange
    };
    MetricComparison {
        metric,
        before_median,
        after_median,
        difference,
        spread_before,
        spread_after,
        threshold,
        verdict,
    }
}

/// Compare the runs taken before a change with the runs taken after.
///
/// A run with a non-finite number is ignored rather than trusted.
pub fn compare(before: &[RunSummary], after: &[RunSummary]) -> Comparison {
    let usable = |runs: &[RunSummary]| -> Vec<RunSummary> {
        runs.iter()
            .filter(|r| r.avg_fps.is_finite() && r.one_percent_low_fps.is_finite())
            .cloned()
            .collect()
    };
    let (before, after) = (usable(before), usable(after));
    let col = |runs: &[RunSummary], f: fn(&RunSummary) -> f64| runs.iter().map(f).collect::<Vec<_>>();

    let average = compare_metric(
        Metric::AvgFps,
        &col(&before, |r| r.avg_fps),
        &col(&after, |r| r.avg_fps),
    );
    let lows = compare_metric(
        Metric::OnePercentLowFps,
        &col(&before, |r| r.one_percent_low_fps),
        &col(&after, |r| r.one_percent_low_fps),
    );

    // A change that hurts either number is not called an improvement.
    let overall = match (average.verdict, lows.verdict) {
        (Verdict::NotEnoughData, _) | (_, Verdict::NotEnoughData) => Verdict::NotEnoughData,
        (Verdict::Worse, _) | (_, Verdict::Worse) => Verdict::Worse,
        (Verdict::Better, _) | (_, Verdict::Better) => Verdict::Better,
        _ => Verdict::NoMeasurableChange,
    };

    let headline = headline(overall, &average, &lows, before.len(), after.len());
    Comparison {
        average,
        lows,
        overall,
        headline,
        before_run_ids: before.iter().map(|r| r.run_id.clone()).collect(),
        after_run_ids: after.iter().map(|r| r.run_id.clone()).collect(),
        warnings: Vec::new(),
    }
}

fn headline(
    overall: Verdict,
    avg: &MetricComparison,
    lows: &MetricComparison,
    n_before: usize,
    n_after: usize,
) -> String {
    if overall == Verdict::NotEnoughData {
        return format!(
            "Not enough data: each side needs at least {MIN_RUNS_PER_SIDE} runs to tell a change from noise \
             (before has {n_before}, after has {n_after})."
        );
    }
    let label = match overall {
        Verdict::Better => "Better",
        Verdict::Worse => "Worse",
        _ => "No measurable change",
    };
    let line = |name: &str, m: &MetricComparison| {
        format!(
            "{name} {:.1} \u{2192} {:.1} FPS ({:+.1}; run-to-run spread {:.1})",
            m.before_median, m.after_median, m.difference, m.threshold
        )
    };
    let tail = if overall == Verdict::NoMeasurableChange {
        " The difference is within the run-to-run spread measured on this PC."
    } else {
        ""
    };
    format!("{label}: {}; {}.{tail}", line("average", avg), line("1% low", lows))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(values: &[(f64, f64)]) -> Vec<RunSummary> {
        values
            .iter()
            .enumerate()
            .map(|(i, (a, l))| RunSummary {
                run_id: format!("r{i}"),
                avg_fps: *a,
                one_percent_low_fps: *l,
            })
            .collect()
    }

    #[test]
    fn a_difference_larger_than_the_spread_is_better() {
        // Before: 60, 61, 62 (spread 2). After: 70, 71, 72. Difference 10 > 2.
        let c = compare(
            &runs(&[(60.0, 40.0), (61.0, 41.0), (62.0, 42.0)]),
            &runs(&[(70.0, 50.0), (71.0, 51.0), (72.0, 52.0)]),
        );
        assert_eq!(c.average.verdict, Verdict::Better);
        assert_eq!(c.lows.verdict, Verdict::Better);
        assert_eq!(c.overall, Verdict::Better);
        assert!((c.average.difference - 10.0).abs() < 1e-9);
        assert!(c.headline.starts_with("Better:"), "{}", c.headline);
        assert_eq!(c.before_run_ids, vec!["r0", "r1", "r2"]);
    }

    #[test]
    fn a_difference_inside_the_spread_is_not_called_anything() {
        // Runs jump around by 6 FPS with no change; the "improvement" of 2 is noise.
        let c = compare(
            &runs(&[(60.0, 40.0), (66.0, 44.0), (63.0, 42.0)]),
            &runs(&[(62.0, 41.0), (68.0, 45.0), (65.0, 43.0)]),
        );
        assert!((c.average.difference - 2.0).abs() < 1e-9);
        assert!((c.average.threshold - 6.0).abs() < 1e-9);
        assert_eq!(c.average.verdict, Verdict::NoMeasurableChange);
        assert_eq!(c.overall, Verdict::NoMeasurableChange);
        assert!(c.headline.starts_with("No measurable change:"), "{}", c.headline);
        assert!(c.headline.contains("within the run-to-run spread"));
    }

    #[test]
    fn exactly_the_spread_is_not_enough() {
        // spread 4 on both sides, difference exactly 4.
        let c = compare(
            &runs(&[(60.0, 40.0), (64.0, 44.0)]),
            &runs(&[(66.0, 46.0), (70.0, 50.0)]),
        );
        assert!((c.average.difference - 6.0).abs() < 1e-9);
        // 6 > 4: better. Now make it exactly 4 by shifting the after side.
        let c = compare(
            &runs(&[(60.0, 40.0), (64.0, 44.0)]),
            &runs(&[(64.0, 44.0), (68.0, 48.0)]),
        );
        assert!((c.average.difference - 4.0).abs() < 1e-9);
        assert_eq!(
            c.average.verdict,
            Verdict::NoMeasurableChange,
            "must strictly exceed the spread"
        );
    }

    #[test]
    fn a_regression_larger_than_the_spread_is_worse() {
        let c = compare(
            &runs(&[(100.0, 80.0), (101.0, 81.0)]),
            &runs(&[(90.0, 70.0), (91.0, 71.0)]),
        );
        assert_eq!(c.overall, Verdict::Worse);
        assert!(c.headline.starts_with("Worse:"));
    }

    #[test]
    fn better_average_but_worse_lows_is_worse_overall() {
        let c = compare(
            &runs(&[(60.0, 50.0), (60.5, 50.5)]),
            &runs(&[(75.0, 30.0), (75.5, 30.5)]),
        );
        assert_eq!(c.average.verdict, Verdict::Better);
        assert_eq!(c.lows.verdict, Verdict::Worse);
        assert_eq!(
            c.overall,
            Verdict::Worse,
            "a change that hurts the lows is not an improvement"
        );
    }

    #[test]
    fn perfectly_repeatable_runs_still_need_a_one_percent_difference() {
        // Zero spread, difference 0.4 on a 100 FPS baseline (0.4%): not a result.
        let c = compare(
            &runs(&[(100.0, 90.0), (100.0, 90.0)]),
            &runs(&[(100.4, 90.4), (100.4, 90.4)]),
        );
        assert_eq!(c.overall, Verdict::NoMeasurableChange);
        // 2% is.
        let c = compare(
            &runs(&[(100.0, 90.0), (100.0, 90.0)]),
            &runs(&[(102.0, 92.0), (102.0, 92.0)]),
        );
        assert_eq!(c.overall, Verdict::Better);
    }

    #[test]
    fn one_run_a_side_is_not_enough_and_says_so() {
        let c = compare(&runs(&[(60.0, 40.0)]), &runs(&[(90.0, 70.0), (91.0, 71.0)]));
        assert_eq!(c.overall, Verdict::NotEnoughData);
        assert!(c.headline.contains("before has 1"), "{}", c.headline);
        assert!(compare(&[], &[]).overall == Verdict::NotEnoughData);
    }

    #[test]
    fn non_finite_runs_are_ignored_not_trusted() {
        let mut before = runs(&[(60.0, 40.0), (61.0, 41.0)]);
        before.push(RunSummary {
            run_id: "bad".into(),
            avg_fps: f64::NAN,
            one_percent_low_fps: 1.0,
        });
        let c = compare(&before, &runs(&[(80.0, 60.0), (81.0, 61.0)]));
        assert_eq!(c.before_run_ids, vec!["r0", "r1"]);
        assert_eq!(c.overall, Verdict::Better);
    }

    #[test]
    fn median_is_used_so_one_outlier_run_does_not_decide_the_result() {
        // Before has one fluke fast run (90). The median stays 60.
        let c = compare(
            &runs(&[(60.0, 40.0), (61.0, 41.0), (90.0, 70.0)]),
            &runs(&[(75.0, 55.0), (76.0, 56.0), (77.0, 57.0)]),
        );
        assert!((c.average.before_median - 61.0).abs() < 1e-9);
        // But the fluke widens the spread (30), so the +15 is not called a result.
        assert_eq!(c.average.verdict, Verdict::NoMeasurableChange);
    }
}
