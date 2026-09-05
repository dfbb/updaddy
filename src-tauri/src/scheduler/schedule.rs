use chrono::{Datelike, Local, NaiveDate, NaiveTime, TimeZone, Weekday};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex,
};
use std::thread;
use std::time::Duration;

use super::catch_up::{CatchUp, SchedulerState};
use crate::core::{Ecosystem, Operation, PackageRecord, PackageTask};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Schedule {
    Daily { time: NaiveTime },
    Weekly { weekday: Weekday, time: NaiveTime },
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ScheduleError {
    #[error("invalid time, expected HH:MM")]
    InvalidTime,
}

pub trait IntoWeekday {
    fn into_weekday(self) -> Option<Weekday>;
}

impl IntoWeekday for Weekday {
    fn into_weekday(self) -> Option<Weekday> {
        Some(self)
    }
}

impl IntoWeekday for &str {
    fn into_weekday(self) -> Option<Weekday> {
        match self.to_ascii_lowercase().as_str() {
            "mon" | "monday" => Some(Weekday::Mon),
            "tue" | "tues" | "tuesday" => Some(Weekday::Tue),
            "wed" | "wednesday" => Some(Weekday::Wed),
            "thu" | "thur" | "thurs" | "thursday" => Some(Weekday::Thu),
            "fri" | "friday" => Some(Weekday::Fri),
            "sat" | "saturday" => Some(Weekday::Sat),
            "sun" | "sunday" => Some(Weekday::Sun),
            _ => None,
        }
    }
}

impl IntoWeekday for String {
    fn into_weekday(self) -> Option<Weekday> {
        self.as_str().into_weekday()
    }
}

fn parse_time(value: &str) -> Result<NaiveTime, ScheduleError> {
    NaiveTime::parse_from_str(value, "%H:%M").map_err(|_| ScheduleError::InvalidTime)
}

impl Schedule {
    pub fn daily(time: &str) -> Result<Self, ScheduleError> {
        Ok(Self::Daily {
            time: parse_time(time)?,
        })
    }

    pub fn weekly<W: IntoWeekday>(weekday: W, time: &str) -> Result<Self, ScheduleError> {
        let weekday = weekday.into_weekday().ok_or(ScheduleError::InvalidTime)?;
        Ok(Self::Weekly {
            weekday,
            time: parse_time(time)?,
        })
    }

    pub fn next_due(&self, now: i64) -> i64 {
        let current = Local
            .timestamp_opt(now, 0)
            .single()
            .unwrap_or_else(Local::now);
        let date = current.date_naive();
        let candidate_date = match self {
            Self::Daily { .. } => date,
            Self::Weekly { weekday, .. } => {
                let days = weekday.num_days_from_monday() as i64
                    - date.weekday().num_days_from_monday() as i64;
                date + chrono::Duration::days(days.rem_euclid(7))
            }
        };
        let time = match self {
            Self::Daily { time } | Self::Weekly { time, .. } => *time,
        };
        let candidate = Local
            .from_local_datetime(&candidate_date.and_time(time))
            .single()
            .unwrap_or_else(|| Local.from_utc_datetime(&candidate_date.and_time(time)));
        if candidate.timestamp() > now {
            candidate.timestamp()
        } else {
            let next_date = match self {
                Self::Daily { .. } => date + chrono::Duration::days(1),
                Self::Weekly { .. } => candidate_date + chrono::Duration::days(7),
            };
            Local
                .from_local_datetime(&next_date.and_time(time))
                .single()
                .unwrap_or_else(|| Local.from_utc_datetime(&next_date.and_time(time)))
                .timestamp()
        }
    }

    pub(crate) fn due_for_cycle(&self, cycle_id: &str) -> Option<i64> {
        let date_part = cycle_id.rsplit_once(':').map_or(cycle_id, |(_, date)| date);
        let date = NaiveDate::parse_from_str(date_part, "%Y-%m-%d")
            .ok()
            .or_else(|| NaiveDate::parse_from_str(cycle_id, "%Y-W%W-%w").ok())?;
        let (date, time) = match self {
            Self::Daily { time } => (date, *time),
            Self::Weekly { weekday, time } => {
                let offset = weekday.num_days_from_monday() as i64
                    - date.weekday().num_days_from_monday() as i64;
                (date + chrono::Duration::days(offset), *time)
            }
        };
        Some(
            Local
                .from_local_datetime(&date.and_time(time))
                .single()
                .unwrap_or_else(|| Local.from_utc_datetime(&date.and_time(time)))
                .timestamp(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnabledEcosystems(std::collections::HashSet<Ecosystem>);

impl EnabledEcosystems {
    pub fn only(list: &[Ecosystem]) -> Self {
        Self(list.iter().copied().collect())
    }
    pub fn all() -> Self {
        Self(Ecosystem::ALL.into_iter().collect())
    }
    pub fn contains(&self, ecosystem: Ecosystem) -> bool {
        self.0.contains(&ecosystem)
    }
}

#[derive(Clone)]
pub struct Scheduler {
    schedule: Arc<Mutex<Option<Schedule>>>,
    config_version: String,
    started: Arc<AtomicBool>,
    state: Arc<Mutex<SchedulerState>>,
    runner: Arc<Mutex<Option<SchedulerRunner>>>,
    state_recorder: Arc<Mutex<Option<StateRecorder>>>,
    wake: Arc<(Mutex<u64>, Condvar)>,
}

type SchedulerRunner = Arc<dyn Fn() -> bool + Send + Sync>;
type StateRecorder = Arc<dyn Fn(&SchedulerState) + Send + Sync>;

impl Scheduler {
    pub fn new(schedule: Schedule) -> Self {
        Self {
            schedule: Arc::new(Mutex::new(Some(schedule))),
            config_version: "v1".into(),
            started: Arc::new(AtomicBool::new(false)),
            state: Arc::new(Mutex::new(SchedulerState::default())),
            runner: Arc::new(Mutex::new(None)),
            state_recorder: Arc::new(Mutex::new(None)),
            wake: Arc::new((Mutex::new(0), Condvar::new())),
        }
    }
    pub fn with_config_version(schedule: Schedule, version: impl Into<String>) -> Self {
        Self {
            schedule: Arc::new(Mutex::new(Some(schedule))),
            config_version: version.into(),
            started: Arc::new(AtomicBool::new(false)),
            state: Arc::new(Mutex::new(SchedulerState::default())),
            runner: Arc::new(Mutex::new(None)),
            state_recorder: Arc::new(Mutex::new(None)),
            wake: Arc::new((Mutex::new(0), Condvar::new())),
        }
    }
    pub fn set_runner(&self, runner: impl Fn() -> bool + Send + Sync + 'static) {
        *self.runner.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(runner));
    }

    pub fn restore_state(&self, state: SchedulerState) {
        *self.state.lock().unwrap_or_else(|p| p.into_inner()) = state;
    }

    pub fn set_state_recorder(&self, recorder: impl Fn(&SchedulerState) + Send + Sync + 'static) {
        *self
            .state_recorder
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(recorder));
    }

    pub fn set_schedule(&self, schedule: Option<Schedule>) {
        *self
            .schedule
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = schedule;
        self.wake.1.notify_all();
    }

    fn schedule_snapshot(&self) -> Option<Schedule> {
        self.schedule
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn trigger(&self) {
        let Some(schedule) = self.schedule_snapshot() else {
            return;
        };
        let now = chrono::Utc::now().timestamp();
        let cycle = self.cycle_id(now);
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if CatchUp::should_run(&cycle, now, &state, &schedule) {
            if let Some(run) = self
                .runner
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
            {
                // 忙碌时保留本周期，下一次唤醒后重试，避免手动批次吞掉计划任务。
                if run() {
                    *state = state.record_cycle(cycle);
                    if let Some(record) = self
                        .state_recorder
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .clone()
                    {
                        record(&state);
                    }
                }
            }
        }
    }
    pub fn start(&self) {
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let started = self.started.clone();
        let this = self.clone();
        thread::spawn(move || {
            this.trigger();
            while started.load(Ordering::Acquire) {
                let now = chrono::Utc::now().timestamp();
                let wait = this
                    .schedule_snapshot()
                    .map(|schedule| schedule.next_due(now).saturating_sub(now).max(1) as u64)
                    .unwrap_or(60);
                let (generation, wake) = &*this.wake;
                let current = generation.lock().unwrap_or_else(|p| p.into_inner());
                let _ = wake.wait_timeout(current, Duration::from_secs(wait.min(60)));
                if !started.load(Ordering::Acquire) {
                    break;
                }
                this.trigger();
            }
        });
    }

    pub fn stop(&self) {
        self.started.store(false, Ordering::Release);
        let mut generation = self.wake.0.lock().unwrap_or_else(|p| p.into_inner());
        *generation = generation.wrapping_add(1);
        self.wake.1.notify_all();
    }

    pub fn catch_up(&self, state: &SchedulerState, now: i64) -> Option<String> {
        let schedule = self.schedule_snapshot()?;
        let cycle = self.cycle_id(now);
        CatchUp::should_run(&cycle, now, state, &schedule).then_some(cycle)
    }
    pub fn next_due(&self, now: i64) -> i64 {
        self.schedule_snapshot()
            .map_or_else(|| now.saturating_add(60), |schedule| schedule.next_due(now))
    }

    /// 返回稳定周期标识；计划配置变更后不会复用旧周期记录。
    pub fn cycle_id(&self, now: i64) -> String {
        let Some(schedule) = self.schedule_snapshot() else {
            return format!("{}:disabled", self.config_version);
        };
        let current = Local
            .timestamp_opt(now, 0)
            .single()
            .unwrap_or_else(Local::now);
        let (schedule_key, date) = match schedule {
            Schedule::Daily { time } => (
                format!("daily-{}", time.format("%H-%M")),
                current.date_naive(),
            ),
            Schedule::Weekly { weekday, time } => {
                let d = current.date_naive();
                (
                    format!(
                        "weekly-{}-{}",
                        weekday.num_days_from_monday(),
                        time.format("%H-%M")
                    ),
                    d - chrono::Duration::days(d.weekday().num_days_from_monday() as i64),
                )
            }
        };
        format!(
            "{}:{}:{}",
            self.config_version,
            schedule_key,
            date.format("%Y-%m-%d")
        )
    }

    pub fn plan_visible_updates<I>(visible: EnabledEcosystems, snapshots: I) -> Vec<PackageTask>
    where
        I: IntoIterator<Item = PackageRecord>,
    {
        snapshots
            .into_iter()
            .filter(|p| visible.contains(p.ecosystem) && p.update_available)
            .map(|p| PackageTask::new(p.ecosystem, p.name, Operation::Update))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;
    use std::sync::atomic::AtomicUsize;

    fn unix(value: &str) -> i64 {
        DateTime::parse_from_rfc3339(value).unwrap().timestamp()
    }

    #[test]
    fn daily_next_due_uses_local_time() {
        let schedule = Schedule::daily("09:00").unwrap();
        let now = unix("2026-09-04T08:00:00+08:00");
        assert_eq!(
            Local
                .timestamp_opt(schedule.next_due(now), 0)
                .single()
                .unwrap()
                .time(),
            NaiveTime::from_hms_opt(9, 0, 0).unwrap()
        );
    }

    fn snapshot_with_all_ecosystems() -> Vec<PackageRecord> {
        Ecosystem::ALL
            .into_iter()
            .map(|ecosystem| PackageRecord {
                id: format!("{ecosystem:?}"),
                ecosystem,
                resource_kind: crate::core::ResourceKind::Package,
                name: format!("{ecosystem:?}-package"),
                current_version: Some("1.0".into()),
                target_version: Some("2.0".into()),
                disk_usage: None,
                update_available: true,
            })
            .collect()
    }

    #[test]
    fn hidden_ecosystem_is_excluded_from_scheduled_batch() {
        let visible = EnabledEcosystems::only(&[Ecosystem::Npm]);
        let tasks = Scheduler::plan_visible_updates(visible, snapshot_with_all_ecosystems());
        assert!(tasks.iter().all(|task| task.ecosystem == Ecosystem::Npm));
        assert_eq!(tasks.len(), 1);
    }

    #[test]
    fn rejected_run_is_not_recorded_and_an_accepted_run_is_recorded_once() {
        let scheduler = Scheduler::new(Schedule::daily("00:00").unwrap());
        let accepted = Arc::new(AtomicBool::new(false));
        let accepted_for_runner = accepted.clone();
        scheduler.set_runner(move || accepted_for_runner.load(Ordering::Acquire));
        let records = Arc::new(AtomicUsize::new(0));
        let records_for_callback = records.clone();
        scheduler.set_state_recorder(move |_| {
            records_for_callback.fetch_add(1, Ordering::AcqRel);
        });

        scheduler.trigger();
        assert_eq!(records.load(Ordering::Acquire), 0);
        accepted.store(true, Ordering::Release);
        scheduler.trigger();
        scheduler.trigger();
        assert_eq!(records.load(Ordering::Acquire), 1);
    }

    #[test]
    fn scheduler_state_round_trips_for_cross_restart_catch_up() {
        let state = SchedulerState::default().record_cycle("v1:daily-09-00:2026-09-05");
        let json = serde_json::to_string(&state).unwrap();
        let restored: SchedulerState = serde_json::from_str(&json).unwrap();
        assert!(restored.has_cycle("v1:daily-09-00:2026-09-05"));
    }
}
