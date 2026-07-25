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
    cached_grace_period: Duration,
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
            cached_grace_period: Duration::from_secs(3),
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

    pub fn has_completed_in_grace_period(&self) -> bool {
        // Short-circuit over the entries directly instead of allocating a sorted Vec, since this
        // runs every tick and only needs to know whether any visible entry is completed.
        self.entries.iter().any(|entry| {
            matches!(entry.status, acp::PlanEntryStatus::Completed)
                && self.is_visible(entry, self.last_tick, self.grace_period)
        })
    }

    pub fn cached_visible_entries(&mut self) -> &[acp::PlanEntry] {
        if self.version != self.cached_version
            || self.last_tick != self.cached_tick
            || self.grace_period != self.cached_grace_period
        {
            self.cached_entries = self.visible_entries(self.last_tick, self.grace_period);
            self.cached_version = self.version;
            self.cached_tick = self.last_tick;
            self.cached_grace_period = self.grace_period;
        }
        &self.cached_entries
    }

    /// Borrow the most recently cached visible entries without forcing a refresh.
    pub fn cached_entries(&self) -> &[acp::PlanEntry] {
        &self.cached_entries
    }

    pub fn has_entries(&self) -> bool {
        !self.entries.is_empty()
    }

    pub fn on_tick(&mut self, now: Instant) {
        self.last_tick = now;
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

    // ACP PlanEntry has no stable id field; content is the only available identity key.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::{PlanEntryPriority, PlanEntryStatus};
    use std::time::{Duration, Instant};

    const GRACE_PERIOD: Duration = Duration::from_secs(3);

    fn entry(content: &str, status: PlanEntryStatus) -> acp::PlanEntry {
        acp::PlanEntry::new(content.to_string(), PlanEntryPriority::Medium, status)
    }

    fn completed_at_for(tracker: &PlanTracker, entry: &acp::PlanEntry) -> Option<Instant> {
        tracker.completed_at.get(&PlanTracker::entry_key(entry)).copied()
    }

    // --- Visibility and grace period ---

    #[test]
    fn completed_entry_visible_immediately_after_transition() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(vec![entry("Task A", PlanEntryStatus::Pending)], now);
        tracker.replace(vec![entry("Task A", PlanEntryStatus::Completed)], now);

        let visible = tracker.visible_entries(now, GRACE_PERIOD);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].content, "Task A");
    }

    #[test]
    fn completed_entry_hidden_after_grace_period() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(vec![entry("Task A", PlanEntryStatus::Completed)], now);

        let visible = tracker.visible_entries(now + GRACE_PERIOD + Duration::from_millis(1), GRACE_PERIOD);
        assert!(visible.is_empty());
    }

    #[test]
    fn completed_entry_still_visible_within_grace_period() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(vec![entry("Task A", PlanEntryStatus::Completed)], now);

        let visible = tracker.visible_entries(now + GRACE_PERIOD, GRACE_PERIOD);
        assert_eq!(visible.len(), 1);
    }

    #[test]
    fn pending_and_in_progress_remain_visible_beyond_grace_period() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(
            vec![entry("Pending task", PlanEntryStatus::Pending), entry("Active task", PlanEntryStatus::InProgress)],
            now,
        );

        let far_future = now + GRACE_PERIOD + Duration::from_secs(100);
        let visible = tracker.visible_entries(far_future, GRACE_PERIOD);
        let contents: Vec<_> = visible.iter().map(|e| e.content.as_str()).collect();
        assert_eq!(contents, vec!["Active task", "Pending task"]);
    }

    #[test]
    fn completed_entry_visible_when_now_before_completed_at_does_not_panic() {
        let mut tracker = PlanTracker::default();
        let completed_at = Instant::now();

        tracker.replace(vec![entry("Task A", PlanEntryStatus::Completed)], completed_at);

        let now_before = completed_at.checked_sub(Duration::from_secs(1)).unwrap();
        let visible = tracker.visible_entries(now_before, GRACE_PERIOD);
        assert_eq!(visible.len(), 1);
    }

    // --- Ordering ---

    #[test]
    fn in_progress_sorted_before_pending() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(
            vec![
                entry("P-A", PlanEntryStatus::Pending),
                entry("IP-B", PlanEntryStatus::InProgress),
                entry("P-C", PlanEntryStatus::Pending),
                entry("IP-D", PlanEntryStatus::InProgress),
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

    #[test]
    fn completed_sorted_after_in_progress_and_pending() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(
            vec![
                entry("Completed", PlanEntryStatus::Completed),
                entry("Pending", PlanEntryStatus::Pending),
                entry("Active", PlanEntryStatus::InProgress),
            ],
            now,
        );

        let visible = tracker.visible_entries(now, GRACE_PERIOD);
        let statuses: Vec<_> = visible.iter().map(|e| e.status.clone()).collect();
        assert_eq!(statuses, vec![PlanEntryStatus::InProgress, PlanEntryStatus::Pending, PlanEntryStatus::Completed]);
    }

    #[test]
    fn mixed_visibility_after_grace_period_hides_completed_only() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(
            vec![
                entry("Completed Old", PlanEntryStatus::Completed),
                entry("Active", PlanEntryStatus::InProgress),
                entry("Pending", PlanEntryStatus::Pending),
            ],
            now,
        );

        let far_future = now + GRACE_PERIOD + Duration::from_millis(1);
        let visible = tracker.visible_entries(far_future, GRACE_PERIOD);
        let contents: Vec<_> = visible.iter().map(|e| e.content.as_str()).collect();
        assert_eq!(contents, vec!["Active", "Pending"]);
    }

    // --- Timestamp preservation ---

    #[test]
    fn completion_timestamp_preserved_across_repeated_updates() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();
        let entry = entry("Task A", PlanEntryStatus::Completed);

        tracker.replace(vec![entry.clone()], now);
        let initial_ts = completed_at_for(&tracker, &entry).expect("timestamp should exist");

        tracker.replace(vec![entry.clone()], now + Duration::from_secs(5));
        let later_ts = completed_at_for(&tracker, &entry).expect("timestamp should still exist");

        assert_eq!(initial_ts, later_ts);
    }

    #[test]
    fn completion_timestamp_cleared_when_item_becomes_non_completed() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(vec![entry("Task A", PlanEntryStatus::Completed)], now);
        assert!(completed_at_for(&tracker, &entry("Task A", PlanEntryStatus::Completed)).is_some());

        tracker.replace(vec![entry("Task A", PlanEntryStatus::Pending)], now + Duration::from_secs(1));
        assert!(completed_at_for(&tracker, &entry("Task A", PlanEntryStatus::Completed)).is_none());
    }

    #[test]
    fn stale_timestamp_removed_when_entry_disappears() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(vec![entry("Task A", PlanEntryStatus::Completed)], now);
        assert!(completed_at_for(&tracker, &entry("Task A", PlanEntryStatus::Completed)).is_some());

        tracker.replace(vec![], now + Duration::from_secs(1));
        assert!(completed_at_for(&tracker, &entry("Task A", PlanEntryStatus::Completed)).is_none());
    }

    // --- Clear ---

    #[test]
    fn clear_removes_all_entries_and_timestamps() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(
            vec![entry("Task A", PlanEntryStatus::Completed), entry("Task B", PlanEntryStatus::InProgress)],
            now,
        );

        tracker.clear();

        let visible = tracker.visible_entries(now, GRACE_PERIOD);
        assert!(visible.is_empty());
        assert!(!tracker.has_entries());
    }

    // --- Version ---

    #[test]
    fn version_increments_on_replace() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        let v1 = tracker.version;
        tracker.replace(vec![entry("Task A", PlanEntryStatus::Pending)], now);
        assert_ne!(tracker.version, v1);

        let v2 = tracker.version;
        tracker.replace(vec![entry("Task B", PlanEntryStatus::Pending)], now);
        assert_ne!(tracker.version, v2);
    }

    #[test]
    fn version_increments_on_clear() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(vec![entry("Task A", PlanEntryStatus::Pending)], now);
        let v_before = tracker.version;
        tracker.clear();
        assert_ne!(tracker.version, v_before);
    }

    #[test]
    fn has_entries_false_after_clear() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(vec![entry("Task A", PlanEntryStatus::Pending)], now);
        assert!(tracker.has_entries());

        tracker.clear();
        assert!(!tracker.has_entries());
    }

    // --- has_completed_in_grace_period ---

    #[test]
    fn has_completed_in_grace_period_true_while_completed_within_grace() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(vec![entry("Task A", PlanEntryStatus::Completed)], now);
        tracker.on_tick(now);

        assert!(tracker.has_completed_in_grace_period());
    }

    #[test]
    fn has_completed_in_grace_period_false_after_expiry() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(vec![entry("Task A", PlanEntryStatus::Completed)], now);
        tracker.on_tick(now + GRACE_PERIOD + Duration::from_millis(1));

        assert!(!tracker.has_completed_in_grace_period());
    }

    #[test]
    fn has_completed_in_grace_period_false_when_only_pending() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(vec![entry("Task A", PlanEntryStatus::Pending)], now);
        tracker.on_tick(now);

        assert!(!tracker.has_completed_in_grace_period());
    }

    // --- Caching ---

    #[test]
    fn cached_visible_entries_updates_on_version_change() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(vec![entry("Task A", PlanEntryStatus::Pending)], now);
        tracker.on_tick(now);
        let cached = tracker.cached_visible_entries().to_vec();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].content, "Task A");
    }

    #[test]
    fn cached_visible_entries_updates_on_grace_period_change() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(vec![entry("Task A", PlanEntryStatus::Completed)], now);
        tracker.on_tick(now + Duration::from_secs(2));
        assert_eq!(tracker.cached_visible_entries().len(), 1);

        tracker.grace_period = Duration::from_secs(1);
        assert!(tracker.cached_visible_entries().is_empty());
    }

    #[test]
    fn cached_visible_entries_updates_on_tick() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(vec![entry("Task A", PlanEntryStatus::Completed)], now);
        tracker.on_tick(now);
        let cached = tracker.cached_visible_entries().to_vec();
        assert_eq!(cached.len(), 1);

        tracker.on_tick(now + GRACE_PERIOD + Duration::from_millis(1));
        let cached_after = tracker.cached_visible_entries().to_vec();
        assert!(cached_after.is_empty());
    }
}
