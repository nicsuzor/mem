//! Flow ranking display layer (`src/display_rank.rs`).
//!
//! Implements engine §6 (R19–R22) and flow-rule §7:
//! - `display_cmp` with key = plain sum of gain and loss averted (Nic key, Q3 settled per S17)
//! - Ready, blocked, and roots task classification (R20, E16)
//! - Cliff lane (Q9 buffer 7, Q26 trigger; S13, U14, I17)
//! - Unclassed due dates default to fake (R10, Q18)
//! - Benefit per effort as a non-default sort, never summed across nodes (R21, R22)
//!
//! Pure display layer: reads [`crate::flow::FlowOutput`] only, never mutates flow data
//! or feeds back into flow computation (I6).

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::flow::{clean_num, FlowOutput, FlowStatus, CLIFF_BUFFER_DAYS, DEFAULT_EFFORT_DAYS};
use crate::graph::{is_completed, parse_effort_days, GraphNode};
use crate::graph_store::is_ready_status;

// ── Constants (R11, §6) ───────────────────────────────────────────────────────

/// Re-export CLIFF_BUFFER_DAYS (7 days, Q9).
pub const DISPLAY_CLIFF_BUFFER_DAYS: i64 = CLIFF_BUFFER_DAYS;

/// Re-export DEFAULT_EFFORT_DAYS (3 days, display.py:14).
pub const DISPLAY_DEFAULT_EFFORT_DAYS: f64 = DEFAULT_EFFORT_DAYS;

/// Actionable node types for classification and views (ranking.md:499, display.py:21).
pub const ACTIONABLE_TYPES: &[&str] = &["task", "learn", "pr"];

/// Claimable node types eligible for the ready list (ranking.md:500, §8.1).
pub const CLAIMABLE_TYPES: &[&str] = &["task"];

// ── Deadline Class (R10) ─────────────────────────────────────────────────────

/// Deadline class for a dated node (R10, S5, S13, U15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeadlineClass {
    Fake,
    Soft,
    Hard,
}

impl DeadlineClass {
    /// Parse a deadline class from string (case-insensitive).
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "fake" => Some(Self::Fake),
            "soft" => Some(Self::Soft),
            "hard" => Some(Self::Hard),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fake => "fake",
            Self::Soft => "soft",
            Self::Hard => "hard",
        }
    }

    /// Resolve deadline class against date presence.
    ///
    /// An unclassed `due` is read as `fake` (R10, Q18).
    /// If there is no `due` date, returns `None`.
    pub fn resolve(explicit: Option<DeadlineClass>, has_due: bool) -> Option<DeadlineClass> {
        if has_due {
            Some(explicit.unwrap_or(DeadlineClass::Fake))
        } else {
            None
        }
    }
}

// ── Effort & Date Helpers ────────────────────────────────────────────────────

/// Resolve effort in days from an optional effort string.
/// Defaults to [`DEFAULT_EFFORT_DAYS`] (3) if unstated or invalid.
pub fn resolve_effort_days(effort: Option<&str>) -> i64 {
    effort
        .and_then(parse_effort_days)
        .map(|d| d.max(1))
        .unwrap_or(DEFAULT_EFFORT_DAYS as i64)
}

/// Parse due date from an ISO string (`YYYY-MM-DD` or RFC 3339).
pub fn parse_due_date(due: &str) -> Option<NaiveDate> {
    let s = due.trim();
    if s.len() >= 10 {
        if let Ok(d) = NaiveDate::parse_from_str(&s[..10], "%Y-%m-%d") {
            return Some(d);
        }
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.date_naive());
    }
    None
}

// ── Cliff Lane Calculation (R19.1, flow-rule §7) ─────────────────────────────

/// Check whether a node is in the cliff lane.
///
/// Trigger: `deadline_class: hard` AND `days_left <= effort_days + buffer_days`.
/// Fake and soft deadlines never enter the cliff lane (I17).
pub fn is_on_cliff(
    due: Option<NaiveDate>,
    deadline_class: Option<DeadlineClass>,
    effort_days: i64,
    today: NaiveDate,
    buffer_days: i64,
) -> bool {
    let due_date = match due {
        Some(d) => d,
        None => return false,
    };
    let resolved_class = deadline_class.unwrap_or(DeadlineClass::Fake);
    if resolved_class != DeadlineClass::Hard {
        return false;
    }
    let days_left = (due_date - today).num_days();
    days_left <= effort_days + buffer_days
}

/// Compute date when a hard deadline enters the cliff lane: `due - (effort + buffer)`.
/// Returns `None` for fake, soft, or undated nodes.
pub fn cliff_enters_on(
    due: Option<NaiveDate>,
    deadline_class: Option<DeadlineClass>,
    effort_days: i64,
    buffer_days: i64,
) -> Option<NaiveDate> {
    let due_date = due?;
    let resolved_class = deadline_class.unwrap_or(DeadlineClass::Fake);
    if resolved_class != DeadlineClass::Hard {
        return None;
    }
    Some(due_date - chrono::Duration::days(effort_days + buffer_days))
}

/// Display metadata structure embedded in responses (§7.2, `get_task`, `export_graph`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskDisplay {
    pub ready: bool,
    pub blocked: bool,
    pub on_cliff: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cliff_enters_on: Option<NaiveDate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_class: Option<DeadlineClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub days_until_due: Option<i64>,
}

impl TaskDisplay {
    pub fn new(
        ready: bool,
        blocked: bool,
        due: Option<NaiveDate>,
        deadline_class: Option<DeadlineClass>,
        effort_days: i64,
        today: NaiveDate,
        buffer_days: i64,
    ) -> Self {
        let resolved_class = DeadlineClass::resolve(deadline_class, due.is_some());
        let on_cliff = is_on_cliff(due, resolved_class, effort_days, today, buffer_days);
        let cliff_enters_on = cliff_enters_on(due, resolved_class, effort_days, buffer_days);
        let days_until_due = due.map(|d| (d - today).num_days());

        Self {
            ready,
            blocked,
            on_cliff,
            cliff_enters_on,
            deadline_class: resolved_class,
            days_until_due,
        }
    }
}

// ── DisplayItem & Comparator `display_cmp` (R19) ──────────────────────────────

/// A node row prepared for display and canonical ordering.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayItem {
    pub id: String,
    pub flow_status: FlowStatus,
    pub gain: Option<f64>,
    pub loss_averted: Option<f64>,
    pub decision_value: Option<f64>,
    pub on_cliff: bool,
    pub due: Option<NaiveDate>,
    pub effort_days: i64,
    pub display: Option<TaskDisplay>,
}

impl DisplayItem {
    pub fn new(
        id: impl Into<String>,
        flow: Option<&FlowOutput>,
        due: Option<NaiveDate>,
        deadline_class: Option<DeadlineClass>,
        effort_days: i64,
        today: NaiveDate,
        buffer_days: i64,
    ) -> Self {
        let resolved_class = DeadlineClass::resolve(deadline_class, due.is_some());
        let on_cliff = is_on_cliff(due, resolved_class, effort_days, today, buffer_days);
        let (flow_status, gain, loss_averted, decision_value) = match flow {
            Some(f) => (f.flow_status, f.gain, f.loss_averted, f.decision_value),
            None => (FlowStatus::Ok, None, None, None),
        };

        Self {
            id: id.into(),
            flow_status,
            gain,
            loss_averted,
            decision_value,
            on_cliff,
            due,
            effort_days,
            display: None,
        }
    }

    /// Whether this node carries non-zero figures from flow computation.
    pub fn has_figures(&self) -> bool {
        if self.flow_status != FlowStatus::Ok {
            return false;
        }
        let g = clean_num(self.gain.unwrap_or(0.0));
        let l = clean_num(self.loss_averted.unwrap_or(0.0));
        let d = clean_num(self.decision_value.unwrap_or(0.0));
        g != 0.0 || l != 0.0 || d != 0.0
    }

    /// The Nic key: plain sum of gain and loss averted (Q3 settled per S17).
    pub fn nic_sum(&self) -> f64 {
        clean_num(self.gain.unwrap_or(0.0) + self.loss_averted.unwrap_or(0.0))
    }

    /// Benefit per effort day (gain / effort_days).
    pub fn gain_per_effort(&self) -> Option<f64> {
        self.gain.map(|g| g / self.effort_days.max(1) as f64)
    }

    /// Loss averted per effort day (loss_averted / effort_days).
    pub fn loss_averted_per_effort(&self) -> Option<f64> {
        self.loss_averted
            .map(|l| l / self.effort_days.max(1) as f64)
    }

    /// Total benefit per effort day ((gain + loss_averted) / effort_days).
    pub fn total_per_effort(&self) -> Option<f64> {
        match (self.gain, self.loss_averted) {
            (Some(g), Some(l)) => Some((g + l) / self.effort_days.max(1) as f64),
            (Some(g), None) => Some(g / self.effort_days.max(1) as f64),
            (None, Some(l)) => Some(l / self.effort_days.max(1) as f64),
            (None, None) => None,
        }
    }
}

/// The single canonical comparator of work across the server (R19).
///
/// Ordering rules:
/// 1. Cliff lane first (`deadline_class: hard` and `days_left <= effort_days + CLIFF_BUFFER_DAYS`),
///    ordered by `due` ascending.
/// 2. The Nic key: plain sum of gain and loss averted (`gain + loss_averted` DESC).
///    Ties broken by `gain` DESC, then `loss_averted` DESC, then `decision_value` DESC.
/// 3. Nodes with no figure:
///    - `flow_status ≠ ok` (carrying null figures) come next,
///    - then nodes with all figures at 0.
/// 4. `id` ASC, ensuring a deterministic total order.
pub fn display_cmp(a: &DisplayItem, b: &DisplayItem) -> Ordering {
    // 1. Cliff lane first
    match (a.on_cliff, b.on_cliff) {
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        (true, true) => {
            // Ordered by due ascending
            let due_order = match (a.due, b.due) {
                (Some(da), Some(db)) => da.cmp(&db),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            };
            if due_order != Ordering::Equal {
                return due_order;
            }
            // Tie-break by subsequent steps if due dates match
        }
        (false, false) => {}
    }

    // 2. The Nic key: nodes with figures sort before nodes with no figure
    let a_has_figures = a.has_figures();
    let b_has_figures = b.has_figures();

    match (a_has_figures, b_has_figures) {
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        (true, true) => {
            // Both carry non-zero figures: sort by sum DESC
            let sum_a = a.nic_sum();
            let sum_b = b.nic_sum();
            let cmp_sum = sum_b
                .partial_cmp(&sum_a)
                .unwrap_or(Ordering::Equal);
            if cmp_sum != Ordering::Equal {
                return cmp_sum;
            }

            // Ties broken by gain DESC
            let g_a = clean_num(a.gain.unwrap_or(0.0));
            let g_b = clean_num(b.gain.unwrap_or(0.0));
            let cmp_gain = g_b.partial_cmp(&g_a).unwrap_or(Ordering::Equal);
            if cmp_gain != Ordering::Equal {
                return cmp_gain;
            }

            // Then loss_averted DESC
            let l_a = clean_num(a.loss_averted.unwrap_or(0.0));
            let l_b = clean_num(b.loss_averted.unwrap_or(0.0));
            let cmp_loss = l_b.partial_cmp(&l_a).unwrap_or(Ordering::Equal);
            if cmp_loss != Ordering::Equal {
                return cmp_loss;
            }

            // Then decision_value DESC
            let d_a = clean_num(a.decision_value.unwrap_or(0.0));
            let d_b = clean_num(b.decision_value.unwrap_or(0.0));
            let cmp_dec = d_b.partial_cmp(&d_a).unwrap_or(Ordering::Equal);
            if cmp_dec != Ordering::Equal {
                return cmp_dec;
            }

            // Fall through to id ASC total order
        }
        (false, false) => {
            // 3. Nodes with no figure: flow_status != ok comes next, then nodes with all figures at 0
            let a_err = a.flow_status != FlowStatus::Ok;
            let b_err = b.flow_status != FlowStatus::Ok;
            match (a_err, b_err) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                _ => {} // Fall through to id ASC
            }
        }
    }

    // 4. Then id ASC, total deterministic order
    a.id.cmp(&b.id)
}

// ── Non-default Sorts (R22, §7.2) ────────────────────────────────────────────

/// Sort keys accepted by `list_tasks` (§7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSort {
    #[default]
    Default,
    Gain,
    LossAverted,
    DecisionValue,
    GainPerEffort,
    LossAvertedPerEffort,
    Due,
    Id,
}

impl TaskSort {
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "default" => Some(Self::Default),
            "gain" => Some(Self::Gain),
            "loss_averted" => Some(Self::LossAverted),
            "decision_value" => Some(Self::DecisionValue),
            "gain_per_effort" => Some(Self::GainPerEffort),
            "loss_averted_per_effort" => Some(Self::LossAvertedPerEffort),
            "due" => Some(Self::Due),
            "id" => Some(Self::Id),
            _ => None,
        }
    }
}

/// Order two display items by the specified sort key (§7.2, R22).
pub fn display_cmp_by(a: &DisplayItem, b: &DisplayItem, sort: TaskSort) -> Ordering {
    match sort {
        TaskSort::Default => display_cmp(a, b),
        TaskSort::Gain => {
            // Cliff lane first
            match (a.on_cliff, b.on_cliff) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                (true, true) => {
                    let cmp = a.due.cmp(&b.due);
                    if cmp != Ordering::Equal {
                        return cmp;
                    }
                }
                (false, false) => {}
            }
            let ga = clean_num(a.gain.unwrap_or(0.0));
            let gb = clean_num(b.gain.unwrap_or(0.0));
            gb.partial_cmp(&ga)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        }
        TaskSort::LossAverted => {
            match (a.on_cliff, b.on_cliff) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                (true, true) => {
                    let cmp = a.due.cmp(&b.due);
                    if cmp != Ordering::Equal {
                        return cmp;
                    }
                }
                (false, false) => {}
            }
            let la = clean_num(a.loss_averted.unwrap_or(0.0));
            let lb = clean_num(b.loss_averted.unwrap_or(0.0));
            lb.partial_cmp(&la)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        }
        TaskSort::DecisionValue => {
            match (a.on_cliff, b.on_cliff) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                (true, true) => {
                    let cmp = a.due.cmp(&b.due);
                    if cmp != Ordering::Equal {
                        return cmp;
                    }
                }
                (false, false) => {}
            }
            let da = clean_num(a.decision_value.unwrap_or(0.0));
            let db = clean_num(b.decision_value.unwrap_or(0.0));
            db.partial_cmp(&da)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        }
        TaskSort::GainPerEffort => {
            match (a.on_cliff, b.on_cliff) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                (true, true) => {
                    let cmp = a.due.cmp(&b.due);
                    if cmp != Ordering::Equal {
                        return cmp;
                    }
                }
                (false, false) => {}
            }
            let gpea = a.gain_per_effort().map(clean_num).unwrap_or(0.0);
            let gpeb = b.gain_per_effort().map(clean_num).unwrap_or(0.0);
            gpeb.partial_cmp(&gpea)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        }
        TaskSort::LossAvertedPerEffort => {
            match (a.on_cliff, b.on_cliff) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                (true, true) => {
                    let cmp = a.due.cmp(&b.due);
                    if cmp != Ordering::Equal {
                        return cmp;
                    }
                }
                (false, false) => {}
            }
            let lpea = a.loss_averted_per_effort().map(clean_num).unwrap_or(0.0);
            let lpeb = b.loss_averted_per_effort().map(clean_num).unwrap_or(0.0);
            lpeb.partial_cmp(&lpea)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        }
        TaskSort::Due => {
            let due_order = match (a.due, b.due) {
                (Some(da), Some(db)) => da.cmp(&db),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            };
            if due_order != Ordering::Equal {
                due_order
            } else {
                display_cmp(a, b)
            }
        }
        TaskSort::Id => a.id.cmp(&b.id),
    }
}

// ── Task Classification (R20, E16) ───────────────────────────────────────────

/// Task representation for classification.
#[derive(Debug, Clone)]
pub struct DisplayTask {
    pub id: String,
    pub node_type: Option<String>,
    pub status: Option<String>,
    pub is_leaf: bool,
    pub has_acceptance_criteria: bool,
    /// Prerequisites that this task depends on (incoming needs / depends_on).
    pub depends_on: Vec<String>,
    /// Tasks blocked by this task (outgoing needs / blocks).
    pub blocks: Vec<String>,
    pub parent: Option<String>,
    pub due: Option<NaiveDate>,
    pub deadline_class: Option<DeadlineClass>,
    pub effort_days: i64,
    pub flow: Option<FlowOutput>,
}

impl DisplayTask {
    pub fn from_graph_node(node: &GraphNode, flow: Option<&FlowOutput>) -> Self {
        let due_date = node.due.as_deref().and_then(parse_due_date);
        let eff = resolve_effort_days(node.effort.as_deref());
        Self {
            id: node.id.clone(),
            node_type: node.node_type.clone(),
            status: node.status.clone(),
            is_leaf: node.leaf,
            has_acceptance_criteria: node.has_acceptance_criteria,
            depends_on: node.depends_on.clone(),
            blocks: node.blocks.clone(),
            parent: node.parent.clone(),
            due: due_date,
            deadline_class: node.deadline_class.map(|dc| match dc {
                crate::graph::DeadlineClass::Fake => DeadlineClass::Fake,
                crate::graph::DeadlineClass::Soft => DeadlineClass::Soft,
                crate::graph::DeadlineClass::Hard => DeadlineClass::Hard,
            }),
            effort_days: eff,
            flow: flow.cloned(),
        }
    }

    pub fn to_display_item(&self, today: NaiveDate, buffer_days: i64) -> DisplayItem {
        DisplayItem::new(
            &self.id,
            self.flow.as_ref(),
            self.due,
            self.deadline_class,
            self.effort_days,
            today,
            buffer_days,
        )
    }
}

/// Classify tasks into ready, blocked, and roots lists (R20, E16).
///
/// - **Blocked**: has at least one incoming `needs` link / mapped `depends_on`
///   from a node that is not `done` or `cancelled`, or being downstream of a
///   blocked node along `needs`. `status: blocked` alone is NOT blocked (E16).
/// - **Ready**: leaf node, claimable type (`task`), actionable status, and not blocked.
/// - **Roots**: actionable tasks with no parent or parent not in index.
/// - Ready and roots are ordered by [`display_cmp`].
pub fn classify_tasks(
    tasks: &HashMap<String, DisplayTask>,
    today: NaiveDate,
    buffer_days: i64,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let completed_ids: HashSet<String> = tasks
        .iter()
        .filter(|(_, t)| is_completed(t.status.as_deref()))
        .map(|(id, _)| id.to_lowercase())
        .collect();

    let mut directly_blocked: HashSet<String> = HashSet::new();
    let mut actionable_task_ids: Vec<String> = Vec::new();

    for (id, task) in tasks {
        let is_actionable = match task.node_type.as_deref() {
            Some(t) => ACTIONABLE_TYPES.contains(&t),
            None => true,
        };
        if !is_actionable {
            continue;
        }
        if is_completed(task.status.as_deref()) {
            continue;
        }
        actionable_task_ids.push(id.clone());

        // Blocked iff at least one unmet depends_on/needs dependency is not completed
        let has_unmet = task
            .depends_on
            .iter()
            .any(|d| !completed_ids.contains(&d.to_lowercase()));
        if has_unmet {
            directly_blocked.insert(id.clone());
        }
    }

    // BFS transitive propagation downstream along blocks/needs
    let effectively_blocked = {
        let mut blocked_set = directly_blocked.clone();
        let mut queue: VecDeque<String> = directly_blocked.into_iter().collect();
        while let Some(blocked_id) = queue.pop_front() {
            if let Some(task) = tasks.get(&blocked_id) {
                for downstream_id in &task.blocks {
                    if blocked_set.insert(downstream_id.clone()) {
                        queue.push_back(downstream_id.clone());
                    }
                }
            }
        }
        blocked_set
    };

    let mut ready_items: Vec<DisplayItem> = Vec::new();
    let mut blocked_items: Vec<DisplayItem> = Vec::new();

    for id in &actionable_task_ids {
        let task = &tasks[id];
        let item = task.to_display_item(today, buffer_days);
        if effectively_blocked.contains(id) {
            blocked_items.push(item);
        } else if task.is_leaf
            && is_ready_status(
                task.status.as_deref().unwrap_or("inbox"),
                task.has_acceptance_criteria,
            )
        {
            let node_type = task.node_type.as_deref().unwrap_or("task");
            if CLAIMABLE_TYPES.contains(&node_type) {
                ready_items.push(item);
            }
        }
    }

    // Sort ready items by display_cmp
    ready_items.sort_by(display_cmp);
    blocked_items.sort_by(display_cmp);

    // Roots: actionable tasks with no parent or parent not in map
    let mut root_items: Vec<DisplayItem> = tasks
        .iter()
        .filter(|(_, t)| {
            let is_act = match t.node_type.as_deref() {
                Some(ty) => ACTIONABLE_TYPES.contains(&ty),
                None => true,
            };
            is_act && !is_completed(t.status.as_deref())
        })
        .filter(|(_, t)| match &t.parent {
            None => true,
            Some(pid) => !tasks.contains_key(pid),
        })
        .map(|(_, t)| t.to_display_item(today, buffer_days))
        .collect();

    root_items.sort_by(display_cmp);

    let ready = ready_items.into_iter().map(|item| item.id).collect();
    let blocked = blocked_items.into_iter().map(|item| item.id).collect();
    let roots = root_items.into_iter().map(|item| item.id).collect();

    (ready, blocked, roots)
}

/// Adapter to classify [`GraphNode`]s using the flow display layer.
pub fn classify_graph_nodes(
    nodes: &HashMap<String, GraphNode>,
    flow_outputs: Option<&HashMap<String, FlowOutput>>,
    today: NaiveDate,
    buffer_days: i64,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let tasks: HashMap<String, DisplayTask> = nodes
        .iter()
        .map(|(id, node)| {
            let flow = flow_outputs.and_then(|m| m.get(id));
            (id.clone(), DisplayTask::from_graph_node(node, flow))
        })
        .collect();
    classify_tasks(&tasks, today, buffer_days)
}

// ── Never Summed (R21): Group Views & Progress ───────────────────────────────

/// Group-level progress strictly obeying R21 (counts and share done, never summed worth).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupProgress {
    pub total_children: usize,
    pub done_children: usize,
    pub share_done: f64,
}

impl GroupProgress {
    pub fn new(total_children: usize, done_children: usize) -> Self {
        let share_done = if total_children > 0 {
            clean_num(done_children as f64 / total_children as f64)
        } else {
            0.0
        };
        Self {
            total_children,
            done_children,
            share_done,
        }
    }
}

/// Task summary counts strictly obeying R21 (counts only, never added worth).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TaskSummaryCounts {
    pub total: usize,
    pub ready: usize,
    pub blocked: usize,
    pub on_cliff: usize,
    pub carrying_worth: usize,
    pub at_zero: usize,
    pub flow_status_error: usize,
}

/// Compute summary counts across display items adhering strictly to R21.
pub fn compute_task_summary(items: &[DisplayItem], ready_ids: &HashSet<String>) -> TaskSummaryCounts {
    let mut summary = TaskSummaryCounts::default();
    summary.total = items.len();

    for item in items {
        if ready_ids.contains(&item.id) {
            summary.ready += 1;
        }
        if item.on_cliff {
            summary.on_cliff += 1;
        }
        if item.flow_status != FlowStatus::Ok {
            summary.flow_status_error += 1;
        } else if item.has_figures() {
            summary.carrying_worth += 1;
        } else {
            summary.at_zero += 1;
        }
    }
    summary
}
