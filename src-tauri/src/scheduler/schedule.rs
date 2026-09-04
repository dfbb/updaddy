use chrono::{Datelike, Local, NaiveDate, NaiveTime, TimeZone, Weekday};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread;
use std::time::Duration;

use crate::core::{Ecosystem, Operation, PackageRecord, PackageTask};
use super::catch_up::{CatchUp, SchedulerState};

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

#[derive(Debug, Clone)]
pub struct Scheduler {
    pub schedule: Schedule,
    config_version: String,
    started: Arc<AtomicBool>,
}

impl Scheduler {
    pub fn new(schedule: Schedule) -> Self {
        Self {
            schedule,
            config_version: "v1".into(),
            started: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn with_config_version(schedule: Schedule, version: impl Into<String>) -> Self {
        Self {
            schedule,
            config_version: version.into(),
            started: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn start(&self) {
        if self.started.swap(true, Ordering::AcqRel) { return; }
        let schedule = self.schedule.clone();
        let started = self.started.clone();
        thread::spawn(move || {
            while started.load(Ordering::Acquire) {
                let now = chrono::Utc::now().timestamp();
                let wait = schedule.next_due(now).saturating_sub(now).max(1) as u64;
                thread::sleep(Duration::from_secs(wait.min(60)));
                if wait <= 60 { break; }
            }
        });
    }

    pub fn catch_up(&self, state: &SchedulerState, now: i64) -> Option<String> {
        let cycle = self.cycle_id(now);
        CatchUp::should_run(&cycle, now, state, &self.schedule).then_some(cycle)
    }
    pub fn next_due(&self, now: i64) -> i64 {
        self.schedule.next_due(now)
    }

    /// 返回稳定周期标识；计划配置变更后不会复用旧周期记录。
    pub fn cycle_id(&self, now: i64) -> String {
        let current = Local
            .timestamp_opt(now, 0)
            .single()
            .unwrap_or_else(Local::now);
        let date = match self.schedule {
            Schedule::Daily { .. } => current.date_naive(),
            Schedule::Weekly { .. } => {
                let d = current.date_naive();
                d - chrono::Duration::days(d.weekday().num_days_from_monday() as i64)
            }
        };
        format!("{}:{}", self.config_version, date.format("%Y-%m-%d"))
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
}
