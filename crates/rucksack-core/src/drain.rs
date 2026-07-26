use chrono::{DateTime, Duration, Utc};
use std::collections::VecDeque;

/// How many battery drops the rate is measured across.
///
/// At roughly a third of a percent per minute this spans about a quarter of an hour, which is long
/// enough to survive one burst of heavy work and short enough to follow a real change in workload.
const WINDOW_DROPS: usize = 5;

/// How long the battery has left before it reaches a floor, measured from what actually happened.
///
/// The lease clock and the battery are different limits, and on a commute the battery is almost
/// always the smaller one: a 24 hour lease on a Mac with four hours of charge has never had 24 hours
/// to give. Projecting from observed drops is what lets rucksack report the limit that binds rather
/// than the one it was configured with.
///
/// Percent readings are whole numbers, so at a realistic drain rate the gauge only moves every few
/// minutes. Differencing every heartbeat would therefore measure quantisation rather than drain,
/// which is why only the drops are recorded and the rate is taken across several of them.
///
/// Only drops are recorded, and two are needed before anything is projected. Both ends of the span
/// are then real transitions. Timing the first drop from the first heartbeat instead would measure
/// a Mac that sat on mains power all morning as a Mac that discharges one percent per hour, and
/// promise weeks of battery on the strength of it.
#[derive(Debug, Clone, Default)]
pub struct Drain {
    /// Every heartbeat, whether or not the gauge could be read, so that a Mac which stopped ticking
    /// can be told from a gauge that merely stayed quiet.
    last_tick: Option<DateTime<Utc>>,
    /// The last figure actually read.
    last_percent: Option<u8>,
    /// One entry per observed drop, oldest first.
    drops: VecDeque<(DateTime<Utc>, u8)>,
}

impl Drain {
    /// Fold in one heartbeat's reading.
    ///
    /// `max_gap` is the longest silence that still counts as continuous observation. A longer one
    /// means the Mac slept, and sleep advances the wall clock while the battery barely moves:
    /// keeping the window across that would divide a small drop by a large span and promise hours
    /// that do not exist. Such a gap therefore starts the measurement again.
    ///
    /// A heartbeat that could not read the gauge is a tunnel rather than a discontinuity, so it
    /// carries the window forward untouched — but it still counts as a tick, because the daemon was
    /// plainly awake to report it.
    ///
    /// A reading higher than the last one means charging, which has no drain to project and also
    /// invalidates the drops behind it.
    pub fn advance(mut self, at: DateTime<Utc>, percent: Option<u8>, max_gap: Duration) -> Self {
        if self.last_tick.is_some_and(|tick| at - tick > max_gap) {
            self.drops.clear();
            self.last_percent = None;
        }
        self.last_tick = Some(at);

        let Some(percent) = percent else {
            return self;
        };

        match self.last_percent {
            Some(previous) if percent > previous => self.drops.clear(),
            Some(previous) if percent < previous => {
                self.drops.push_back((at, percent));
                while self.drops.len() > WINDOW_DROPS {
                    self.drops.pop_front();
                }
            }
            _ => {}
        }
        self.last_percent = Some(percent);
        self
    }

    /// Minutes until the battery reaches `floor_percent`, or `None` when that cannot be said yet.
    ///
    /// `None` covers every case where an answer would be invented rather than measured: fewer than
    /// two drops seen, a session that has only just started, and a Mac on mains power.
    pub fn minutes_until(&self, floor_percent: u8) -> Option<u64> {
        if self.drops.len() < 2 {
            return None;
        }
        let (first_at, first_percent) = *self.drops.front()?;
        let (last_at, last_percent) = *self.drops.back()?;
        let dropped = first_percent
            .checked_sub(last_percent)
            .filter(|drop| *drop > 0)?;
        let span_minutes = (last_at - first_at).num_seconds() as f64 / 60.0;
        if span_minutes <= 0.0 {
            return None;
        }
        let rate = f64::from(dropped) / span_minutes;
        let headroom = last_percent.saturating_sub(floor_percent);
        Some((f64::from(headroom) / rate).round() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(minute: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(minute * 60, 0).unwrap()
    }

    /// Two minutes, against samples a minute apart: long enough to be quiet, short enough to catch
    /// a sleep. Production uses a small multiple of the heartbeat interval for the same reason.
    fn max_gap() -> Duration {
        Duration::minutes(2)
    }

    /// The gauge repeats itself between drops, and those repeats carry no information about rate.
    ///
    /// Differencing every reading would measure a rate of zero for two minutes and then a cliff on
    /// the third, so only the drops are recorded.
    #[test]
    fn repeated_readings_do_not_move_the_rate() {
        let drain = (0..=9).fold(Drain::default(), |drain, minute| {
            let percent = 100 - u8::try_from(minute / 3).unwrap();
            drain.advance(at(minute), Some(percent), max_gap())
        });

        // Drops at minutes 3, 6 and 9, so two percent across six minutes.
        assert_eq!(drain.minutes_until(12), Some(255));
    }

    /// The reason drops are timed from each other rather than from the first heartbeat.
    ///
    /// This Mac spent two hours on mains power before anything moved. Anchoring the rate on that
    /// first reading would measure one percent per two hours and offer weeks of charge.
    #[test]
    fn a_long_flat_period_does_not_become_a_slow_drain() {
        let flat = (0..=120).fold(Drain::default(), |drain, minute| {
            drain.advance(at(minute), Some(100), max_gap())
        });
        let unplugged =
            flat.advance(at(121), Some(99), max_gap())
                .advance(at(122), Some(98), max_gap());

        // One percent per minute from the two real drops, not one percent per two hours.
        assert_eq!(unplugged.minutes_until(15), Some(83));
    }

    /// A commute is the case this exists for: the Mac slept, and almost no charge went with it.
    ///
    /// Dividing the drops before the gap by the span across it is what would report a battery that
    /// lasts all day, which is the failure this projection replaces.
    #[test]
    fn a_sleep_gap_restarts_the_measurement_rather_than_inflating_it() {
        let before = Drain::default()
            .advance(at(0), Some(100), max_gap())
            .advance(at(1), Some(99), max_gap())
            .advance(at(2), Some(98), max_gap());
        assert_eq!(before.minutes_until(15), Some(83));

        let across = before.advance(at(600), Some(97), max_gap());
        assert_eq!(across.minutes_until(15), None);
    }

    /// Plugging in on the train invalidates the drops behind it; there is no drain left to project.
    #[test]
    fn charging_clears_the_projection() {
        let drain = Drain::default()
            .advance(at(0), Some(90), max_gap())
            .advance(at(1), Some(89), max_gap())
            .advance(at(2), Some(88), max_gap())
            .advance(at(3), Some(90), max_gap());

        assert_eq!(drain.minutes_until(15), None);
    }

    /// Nothing has dropped yet, so nothing may be claimed.
    #[test]
    fn says_nothing_before_it_has_seen_a_drop() {
        let drain = Drain::default()
            .advance(at(0), Some(80), max_gap())
            .advance(at(1), Some(80), max_gap());

        assert_eq!(drain.minutes_until(15), None);
    }

    /// One drop is a reading, not a rate: the moment it crossed is known, the moment it started
    /// falling is not.
    #[test]
    fn says_nothing_after_only_one_drop() {
        let drain = Drain::default()
            .advance(at(0), Some(80), max_gap())
            .advance(at(1), Some(79), max_gap());

        assert_eq!(drain.minutes_until(15), None);
    }

    /// A blind heartbeat is a tunnel, not a discontinuity, so the window survives it untouched.
    #[test]
    fn an_unreadable_gauge_carries_the_window_forward() {
        let drain = Drain::default()
            .advance(at(0), Some(50), max_gap())
            .advance(at(1), Some(49), max_gap())
            .advance(at(2), None, max_gap())
            .advance(at(3), Some(48), max_gap());

        // Half a percent per minute across the two drops, and ten percent of headroom.
        assert_eq!(drain.minutes_until(38), Some(20));
    }

    /// Already at the floor is a real answer, and it is zero rather than nothing.
    #[test]
    fn a_battery_at_the_floor_has_no_minutes_left() {
        let drain = Drain::default()
            .advance(at(0), Some(17), max_gap())
            .advance(at(2), Some(16), max_gap())
            .advance(at(4), Some(15), max_gap());

        assert_eq!(drain.minutes_until(15), Some(0));
    }
}
