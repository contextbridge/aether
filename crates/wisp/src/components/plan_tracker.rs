use agent_client_protocol::schema::{self as acp};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

type PlanEntryKey = String;

pub struct PlanTracker {
    entries: Vec<acp::PlanEntry>,
    completed_at: HashMap<PlanEntryKey, Instant>,
    pub grace_period: Duration,
    last_tick: Instant,
    version: u64,
    cached_entries: Vec<acp::PlanEntry>,
    cached_version: u64,
    cached_tick: Instant,
}

impl Default for PlanTracker {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            completed_at: HashMap::new(),
            grace_period: Duration::from_secs(3),
            last_tick: Instant::now(),
            version: 0,
            cached_entries: Vec::new(),
            cached_version: 0,
            cached_tick: Instant::now(),
        }
    }
}

impl PlanTracker {
    pub fn replace(&mut self, entries: Vec<acp::PlanEntry>, now: Instant) {
        let active_keys: HashSet<_> = entries.iter().map(Self::entry_key).collect();
        self.completed_at.retain(|key, _| active_keys.contains(key));

        for entry in &entries {
            let key = Self::entry_key(entry);
            match entry.status {
                acp::PlanEntryStatus::Completed => {
                    self.completed_at.entry(key).or_insert(now);
                }
                _ => {
                    self.completed_at.remove(&key);
                }
            }
        }

        self.entries = entries;
        self.version = self.version.wrapping_add(1);
    }

    pub fn visible_entries(&self, now: Instant, grace_period: Duration) -> Vec<acp::PlanEntry> {
        let mut visible: Vec<_> =
            self.entries.iter().filter(|entry| self.is_visible(entry, now, grace_period)).cloned().collect();
        visible.sort_by_key(Self::status_sort_order);
        visible
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.completed_at.clear();
        self.version = self.version.wrapping_add(1);
    }

    fn is_visible(&self, entry: &acp::PlanEntry, now: Instant, grace_period: Duration) -> bool {
        match entry.status {
            acp::PlanEntryStatus::Completed => self
                .completed_at
                .get(&Self::entry_key(entry))
                .is_some_and(|completed_at| now.saturating_duration_since(*completed_at) <= grace_period),
            _ => true,
        }
    }

    /// Content is the best stable identity ACP currently gives us for plan entries.
    fn entry_key(entry: &acp::PlanEntry) -> PlanEntryKey {
        entry.content.clone()
    }

    fn status_sort_order(entry: &acp::PlanEntry) -> u8 {
        match entry.status {
            acp::PlanEntryStatus::InProgress => 0,
            acp::PlanEntryStatus::Pending => 1,
            acp::PlanEntryStatus::Completed => 2,
            _ => 3,
        }
    }

    #[cfg(test)]
    fn completed_at_for(&self, entry: &acp::PlanEntry) -> Option<Instant> {
        self.completed_at.get(&Self::entry_key(entry)).copied()
    }

    pub fn has_completed_in_grace_period(&self) -> bool {
        self.entries.iter().any(|entry| {
            matches!(entry.status, acp::PlanEntryStatus::Completed)
                && self.is_visible(entry, self.last_tick, self.grace_period)
        })
    }

    pub fn cached_visible_entries(&mut self) -> &[acp::PlanEntry] {
        if self.version != self.cached_version || self.last_tick != self.cached_tick {
            self.cached_entries = self.visible_entries(self.last_tick, self.grace_period);
            self.cached_version = self.version;
            self.cached_tick = self.last_tick;
        }
        &self.cached_entries
    }

    pub fn cached_entries(&self) -> &[acp::PlanEntry] {
        &self.cached_entries
    }

    /// Advance the animation state. Call this on tick events.
    pub fn on_tick(&mut self, now: Instant) {
        self.last_tick = now;
    }

    /// Returns the version counter, incremented on each `replace` or `clear` call.
    /// Use this to detect plan content changes without comparing entry counts.
    #[cfg(test)]
    pub fn version(&self) -> u64 {
        self.version
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::{PlanEntryPriority, PlanEntryStatus};

    const GRACE_PERIOD: Duration = Duration::from_secs(3);

    fn plan_entry(content: &str, status: PlanEntryStatus) -> acp::PlanEntry {
        acp::PlanEntry::new(content.to_string(), PlanEntryPriority::Medium, status)
    }

    #[test]
    fn completed_entry_visible_immediately_after_transition() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(vec![plan_entry("Task A", PlanEntryStatus::Pending)], now);
        tracker.replace(vec![plan_entry("Task A", PlanEntryStatus::Completed)], now);

        let visible = tracker.visible_entries(now, GRACE_PERIOD);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].content, "Task A");
    }

    #[test]
    fn completed_entry_hidden_after_grace_period() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(vec![plan_entry("Task A", PlanEntryStatus::Completed)], now);

        let visible = tracker.visible_entries(now + GRACE_PERIOD + Duration::from_millis(1), GRACE_PERIOD);
        assert!(visible.is_empty());
    }

    #[test]
    fn pending_and_in_progress_entries_remain_visible() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(
            vec![plan_entry("Pending", PlanEntryStatus::Pending), plan_entry("Working", PlanEntryStatus::InProgress)],
            now,
        );

        let visible = tracker.visible_entries(now + GRACE_PERIOD + Duration::from_secs(10), GRACE_PERIOD);
        let contents: Vec<_> = visible.iter().map(|entry| entry.content.as_str()).collect();
        assert_eq!(contents, vec!["Working", "Pending"]);
    }

    #[test]
    fn completion_timestamp_preserved_across_plan_updates() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();
        let entry = plan_entry("Task A", PlanEntryStatus::Completed);

        tracker.replace(vec![entry.clone()], now);
        let initial_ts = tracker.completed_at_for(&entry).expect("timestamp should exist");

        tracker.replace(vec![entry.clone()], now + Duration::from_secs(1));
        let ts_after = tracker.completed_at_for(&entry).expect("timestamp should exist");

        assert_eq!(initial_ts, ts_after);
    }

    #[test]
    fn completion_timestamp_cleared_when_item_becomes_non_completed() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();
        let completed = plan_entry("Task A", PlanEntryStatus::Completed);
        let pending = plan_entry("Task A", PlanEntryStatus::Pending);

        tracker.replace(vec![completed.clone()], now);
        assert!(tracker.completed_at_for(&completed).is_some());

        tracker.replace(vec![pending], now + Duration::from_secs(1));
        assert!(tracker.completed_at_for(&completed).is_none());
    }

    #[test]
    fn stale_timestamp_removed_when_item_disappears() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();
        let entry = plan_entry("Task A", PlanEntryStatus::Completed);

        tracker.replace(vec![entry.clone()], now);
        assert!(tracker.completed_at_for(&entry).is_some());

        tracker.replace(vec![], now + Duration::from_secs(1));
        assert!(tracker.completed_at_for(&entry).is_none());
    }

    #[test]
    fn mixed_entries_visible_correctly() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(
            vec![
                plan_entry("Completed Old", PlanEntryStatus::Completed),
                plan_entry("Completed New", PlanEntryStatus::Completed),
                plan_entry("In Progress", PlanEntryStatus::InProgress),
                plan_entry("Pending", PlanEntryStatus::Pending),
            ],
            now,
        );

        tracker.replace(
            vec![
                plan_entry("Completed Old", PlanEntryStatus::Completed),
                plan_entry("Completed New", PlanEntryStatus::Completed),
                plan_entry("In Progress", PlanEntryStatus::InProgress),
                plan_entry("Pending", PlanEntryStatus::Pending),
            ],
            now + GRACE_PERIOD + Duration::from_millis(1),
        );

        let visible = tracker.visible_entries(now + GRACE_PERIOD + Duration::from_millis(1), GRACE_PERIOD);
        let visible_contents: Vec<_> = visible.iter().map(|e| e.content.as_str()).collect();
        assert_eq!(visible_contents, vec!["In Progress", "Pending"]);
    }

    #[test]
    fn completed_entry_visible_when_now_before_completed_at_does_not_panic() {
        let mut tracker = PlanTracker::default();
        let completed_at = Instant::now();

        // Add a completed entry at time T
        tracker.replace(vec![plan_entry("Task A", PlanEntryStatus::Completed)], completed_at);

        // Query visibility at a time BEFORE completed_at
        // This should not panic and should treat the entry as still within grace period
        // (saturating_duration_since returns 0 when now < completed_at)
        let now_before = completed_at.checked_sub(Duration::from_secs(1)).unwrap();
        let visible = tracker.visible_entries(now_before, GRACE_PERIOD);
        assert_eq!(visible.len(), 1, "completed entry should still be visible");
    }

    #[test]
    fn version_increments_on_replace() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        let initial_version = tracker.version();
        tracker.replace(vec![plan_entry("Task A", PlanEntryStatus::Pending)], now);
        assert!(tracker.version() > initial_version);

        let version_after_first = tracker.version();
        tracker.replace(vec![plan_entry("Task B", PlanEntryStatus::Pending)], now);
        assert!(tracker.version() > version_after_first);
    }

    #[test]
    fn version_increments_on_clear() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(vec![plan_entry("Task A", PlanEntryStatus::Pending)], now);
        let version_before_clear = tracker.version();
        tracker.clear();
        assert!(tracker.version() > version_before_clear);
    }

    #[test]
    fn in_progress_sorted_before_pending() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(
            vec![
                plan_entry("Pending A", PlanEntryStatus::Pending),
                plan_entry("Working B", PlanEntryStatus::InProgress),
                plan_entry("Pending C", PlanEntryStatus::Pending),
                plan_entry("Working D", PlanEntryStatus::InProgress),
            ],
            now,
        );

        let visible = tracker.visible_entries(now, GRACE_PERIOD);
        let statuses: Vec<_> = visible.iter().map(|e| e.status.clone()).collect();
        assert_eq!(
            statuses,
            vec![
                PlanEntryStatus::InProgress,
                PlanEntryStatus::InProgress,
                PlanEntryStatus::Pending,
                PlanEntryStatus::Pending,
            ]
        );
    }
}
