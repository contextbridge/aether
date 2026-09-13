use agent_client_protocol::schema::v2::{self as acp};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// How long a completed plan entry stays on screen before it is dropped, so the
/// user sees the tick before the row disappears.
pub const GRACE_PERIOD: Duration = Duration::from_secs(3);

/// The agent's plan, with completed entries expiring after a grace period.
#[derive(Debug)]
pub struct PlanTracker {
    plans: Vec<(acp::PlanId, Vec<acp::PlanEntry>)>,
    /// When each entry was first reported complete. ACP plan entries have no
    /// stable id, so content is the only available identity key.
    completed_at: HashMap<(acp::PlanId, String), Instant>,
    last_tick: Instant,
}

impl Default for PlanTracker {
    fn default() -> Self {
        Self { plans: Vec::new(), completed_at: HashMap::new(), last_tick: Instant::now() }
    }
}

impl PlanTracker {
    pub fn apply_update(&mut self, update: &acp::PlanUpdate, now: Instant) {
        if let acp::PlanUpdateContent::Items(items) = &update.plan {
            self.replace(items.plan_id.clone(), items.entries.clone(), now);
        }
    }

    pub fn replace(&mut self, plan_id: acp::PlanId, entries: Vec<acp::PlanEntry>, now: Instant) {
        let active_keys: HashSet<_> = entries.iter().map(|entry| entry.content.clone()).collect();
        self.completed_at.retain(|(id, key), _| *id != plan_id || active_keys.contains(key));

        for entry in &entries {
            let key = (plan_id.clone(), entry.content.clone());
            if terminal_status(&entry.status) {
                self.completed_at.entry(key).or_insert(now);
            } else {
                self.completed_at.remove(&key);
            }
        }

        if let Some((_, current)) = self.plans.iter_mut().find(|(id, _)| *id == plan_id) {
            *current = entries;
        } else {
            self.plans.push((plan_id, entries));
        }
    }

    /// Entries to draw at `now`, ordered in-progress, then pending, then the
    /// completed ones still inside their grace period.
    pub fn visible_entries(&self, now: Instant) -> Vec<acp::PlanEntry> {
        let mut visible: Vec<_> = self
            .plans
            .iter()
            .flat_map(|(id, entries)| entries.iter().filter(|entry| self.is_visible(id, entry, now)))
            .cloned()
            .collect();
        visible.sort_by_key(|entry| match entry.status {
            acp::PlanEntryStatus::InProgress => 0,
            acp::PlanEntryStatus::Pending => 1,
            _ if terminal_status(&entry.status) => 2,
            _ => 3,
        });
        visible
    }

    /// Entries to draw as of the last tick.
    pub fn current_entries(&self) -> Vec<acp::PlanEntry> {
        self.visible_entries(self.last_tick)
    }

    pub fn clear(&mut self) {
        self.plans.clear();
        self.completed_at.clear();
    }

    /// Whether a completed entry is still counting down, which is what keeps the
    /// tick loop running long enough to expire it.
    pub fn has_completed_in_grace_period(&self) -> bool {
        self.plans.iter().any(|(id, entries)| {
            entries.iter().any(|entry| terminal_status(&entry.status) && self.is_visible(id, entry, self.last_tick))
        })
    }

    pub fn has_entries(&self) -> bool {
        self.plans.iter().any(|(_, entries)| !entries.is_empty())
    }

    pub fn on_tick(&mut self, now: Instant) {
        self.last_tick = now;
    }

    fn is_visible(&self, plan_id: &acp::PlanId, entry: &acp::PlanEntry, now: Instant) -> bool {
        match entry.status {
            _ if terminal_status(&entry.status) => self
                .completed_at
                .get(&(plan_id.clone(), entry.content.clone()))
                .is_some_and(|completed_at| now.saturating_duration_since(*completed_at) <= GRACE_PERIOD),
            _ => true,
        }
    }
}

fn terminal_status(status: &acp::PlanEntryStatus) -> bool {
    matches!(status, acp::PlanEntryStatus::Completed)
        || matches!(status, acp::PlanEntryStatus::Other(value) if value == "_aether_cancelled" || value == "cancelled")
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v2::{PlanEntryPriority, PlanEntryStatus};
    use std::time::{Duration, Instant};

    fn entry(content: &str, status: PlanEntryStatus) -> acp::PlanEntry {
        acp::PlanEntry::new(content.to_string(), PlanEntryPriority::Medium, status)
    }

    #[test]
    fn completed_entry_visible_immediately_after_transition() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace("plan".into(), vec![entry("Task A", PlanEntryStatus::Pending)], now);
        tracker.replace("plan".into(), vec![entry("Task A", PlanEntryStatus::Completed)], now);

        let visible = tracker.visible_entries(now);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].content, "Task A");
    }

    #[test]
    fn completed_entry_hidden_after_grace_period() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace("plan".into(), vec![entry("Task A", PlanEntryStatus::Completed)], now);

        let visible = tracker.visible_entries(now + GRACE_PERIOD + Duration::from_millis(1));
        assert!(visible.is_empty());
    }

    #[test]
    fn completed_entry_still_visible_within_grace_period() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace("plan".into(), vec![entry("Task A", PlanEntryStatus::Completed)], now);

        let visible = tracker.visible_entries(now + GRACE_PERIOD);
        assert_eq!(visible.len(), 1);
    }

    #[test]
    fn pending_and_in_progress_remain_visible_beyond_grace_period() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(
            "plan".into(),
            vec![entry("Pending task", PlanEntryStatus::Pending), entry("Active task", PlanEntryStatus::InProgress)],
            now,
        );

        let far_future = now + GRACE_PERIOD + Duration::from_secs(100);
        let visible = tracker.visible_entries(far_future);
        let contents: Vec<_> = visible.iter().map(|e| e.content.as_str()).collect();
        assert_eq!(contents, vec!["Active task", "Pending task"]);
    }

    #[test]
    fn completed_entry_visible_when_now_before_completed_at_does_not_panic() {
        let mut tracker = PlanTracker::default();
        let completed_at = Instant::now();

        tracker.replace("plan".into(), vec![entry("Task A", PlanEntryStatus::Completed)], completed_at);

        let now_before = completed_at.checked_sub(Duration::from_secs(1)).unwrap();
        let visible = tracker.visible_entries(now_before);
        assert_eq!(visible.len(), 1);
    }

    #[test]
    fn in_progress_sorted_before_pending() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(
            "plan".into(),
            vec![
                entry("P-A", PlanEntryStatus::Pending),
                entry("IP-B", PlanEntryStatus::InProgress),
                entry("P-C", PlanEntryStatus::Pending),
                entry("IP-D", PlanEntryStatus::InProgress),
            ],
            now,
        );

        let visible = tracker.visible_entries(now);
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
            "plan".into(),
            vec![
                entry("Completed", PlanEntryStatus::Completed),
                entry("Pending", PlanEntryStatus::Pending),
                entry("Active", PlanEntryStatus::InProgress),
            ],
            now,
        );

        let visible = tracker.visible_entries(now);
        let statuses: Vec<_> = visible.iter().map(|e| e.status.clone()).collect();
        assert_eq!(statuses, vec![PlanEntryStatus::InProgress, PlanEntryStatus::Pending, PlanEntryStatus::Completed]);
    }

    #[test]
    fn mixed_visibility_after_grace_period_hides_completed_only() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(
            "plan".into(),
            vec![
                entry("Completed Old", PlanEntryStatus::Completed),
                entry("Active", PlanEntryStatus::InProgress),
                entry("Pending", PlanEntryStatus::Pending),
            ],
            now,
        );

        let far_future = now + GRACE_PERIOD + Duration::from_millis(1);
        let visible = tracker.visible_entries(far_future);
        let contents: Vec<_> = visible.iter().map(|e| e.content.as_str()).collect();
        assert_eq!(contents, vec!["Active", "Pending"]);
    }

    #[test]
    fn completion_timestamp_preserved_across_repeated_updates() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();
        let entry = entry("Task A", PlanEntryStatus::Completed);

        tracker.replace("plan".into(), vec![entry.clone()], now);
        tracker.replace("plan".into(), vec![entry], now + Duration::from_secs(2));

        assert_eq!(tracker.visible_entries(now + GRACE_PERIOD).len(), 1);
        assert!(tracker.visible_entries(now + GRACE_PERIOD + Duration::from_millis(1)).is_empty());
    }

    #[test]
    fn completion_timestamp_cleared_when_item_becomes_non_completed() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace("plan".into(), vec![entry("Task A", PlanEntryStatus::Completed)], now);
        tracker.replace("plan".into(), vec![entry("Task A", PlanEntryStatus::Pending)], now + Duration::from_secs(1));
        tracker.replace("plan".into(), vec![entry("Task A", PlanEntryStatus::Completed)], now + Duration::from_secs(2));

        assert_eq!(tracker.visible_entries(now + GRACE_PERIOD + Duration::from_millis(1)).len(), 1);
    }

    #[test]
    fn stale_timestamp_removed_when_entry_disappears() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace("plan".into(), vec![entry("Task A", PlanEntryStatus::Completed)], now);
        tracker.replace("plan".into(), vec![], now + Duration::from_secs(1));
        tracker.replace("plan".into(), vec![entry("Task A", PlanEntryStatus::Completed)], now + Duration::from_secs(2));

        assert_eq!(tracker.visible_entries(now + GRACE_PERIOD + Duration::from_millis(1)).len(), 1);
    }

    #[test]
    fn clear_removes_all_entries_and_timestamps() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace(
            "plan".into(),
            vec![entry("Task A", PlanEntryStatus::Completed), entry("Task B", PlanEntryStatus::InProgress)],
            now,
        );

        tracker.clear();

        let visible = tracker.visible_entries(now);
        assert!(visible.is_empty());
        assert!(!tracker.has_entries());
    }

    #[test]
    fn has_entries_false_after_clear() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace("plan".into(), vec![entry("Task A", PlanEntryStatus::Pending)], now);
        assert!(tracker.has_entries());

        tracker.clear();
        assert!(!tracker.has_entries());
    }

    #[test]
    fn has_completed_in_grace_period_true_while_completed_within_grace() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace("plan".into(), vec![entry("Task A", PlanEntryStatus::Completed)], now);
        tracker.on_tick(now);

        assert!(tracker.has_completed_in_grace_period());
    }

    #[test]
    fn has_completed_in_grace_period_false_after_expiry() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace("plan".into(), vec![entry("Task A", PlanEntryStatus::Completed)], now);
        tracker.on_tick(now + GRACE_PERIOD + Duration::from_millis(1));

        assert!(!tracker.has_completed_in_grace_period());
    }

    #[test]
    fn has_completed_in_grace_period_false_when_only_pending() {
        let mut tracker = PlanTracker::default();
        let now = Instant::now();

        tracker.replace("plan".into(), vec![entry("Task A", PlanEntryStatus::Pending)], now);
        tracker.on_tick(now);

        assert!(!tracker.has_completed_in_grace_period());
    }
}
