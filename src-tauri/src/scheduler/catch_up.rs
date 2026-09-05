use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::schedule::Schedule;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerState {
    #[serde(default)]
    completed_cycles: BTreeSet<String>,
}

impl SchedulerState {
    pub fn record_cycle(&self, cycle_id: impl Into<String>) -> Self {
        let mut next = self.clone();
        next.completed_cycles.insert(cycle_id.into());
        next
    }

    pub fn has_cycle(&self, cycle_id: &str) -> bool {
        self.completed_cycles.contains(cycle_id)
    }
    pub fn cycles(&self) -> impl Iterator<Item = &String> {
        self.completed_cycles.iter()
    }
}

pub struct CatchUp;

impl CatchUp {
    pub fn should_run(
        cycle_id: &str,
        now: i64,
        state: &SchedulerState,
        schedule: &Schedule,
    ) -> bool {
        !state.has_cycle(cycle_id)
            && schedule
                .due_for_cycle(cycle_id)
                .is_some_and(|due| now >= due)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;

    fn unix(value: &str) -> i64 {
        DateTime::parse_from_rfc3339(value).unwrap().timestamp()
    }

    #[test]
    fn missed_daily_schedule_runs_once_after_wake() {
        let schedule = Schedule::daily("09:00").unwrap();
        let state = SchedulerState::default();
        let first = CatchUp::should_run(
            "2026-09-04",
            unix("2026-09-04T10:00:00+08:00"),
            &state,
            &schedule,
        );
        assert!(first);
        let state = state.record_cycle("2026-09-04");
        assert!(!CatchUp::should_run(
            "2026-09-04",
            unix("2026-09-04T10:05:00+08:00"),
            &state,
            &schedule
        ));
    }
}
