//! Application state for the Planning Web TUI.

use mem::graph::GraphNode;
use mem::graph_store::{self, GraphStore};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Which field is active in the capture modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureField {
    Title,
    Project,
    Priority,
}

/// The four main views.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Focus,
    Graph,
    EpicTree,
    Dashboard,
}

impl View {
    pub fn label(&self) -> &'static str {
        match self {
            View::Focus => "Focus",
            View::Graph => "Graph",
            View::EpicTree => "Epics",
            View::Dashboard => "Dashboard",
        }
    }

    pub const ALL: [View; 4] = [View::Focus, View::Graph, View::EpicTree, View::Dashboard];
}

/// A flattened row in the tree display, pre-computed for rendering.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TreeRow {
    pub node_id: String,
    pub label: String,
    pub priority: Option<i32>,
    pub status: Option<String>,
    pub node_type: Option<String>,
    pub project: Option<String>,
    pub depth: usize,
    pub is_last_at_depth: Vec<bool>,
    pub is_context: bool,
    pub child_count: usize,
    pub downstream_weight: f64,
    pub stakeholder_exposure: bool,
    pub due: Option<String>,
    pub created: Option<String>,
    pub task_id: Option<String>,
    pub expanded: bool,
    pub has_children: bool,
}

/// Main application state.
pub struct App {
    pub pkb_root: PathBuf,
    pub graph: Option<GraphStore>,

    // View state
    pub current_view: View,
    pub selected_index: usize,
    pub scroll_offset: usize,

    // Epic tree state
    pub tree_rows: Vec<TreeRow>,
    pub expanded_nodes: HashSet<String>,
    pub priority_filter: Option<i32>,

    // Detail overlay
    pub show_detail: bool,
    pub detail_node_id: Option<String>,
    pub detail_scroll: usize,

    // Help overlay
    pub show_help: bool,

    // Search overlay
    pub show_search: bool,
    pub search_query: String,
    pub search_results: Vec<SearchHit>,
    pub search_selected: usize,

    // Quick capture overlay
    pub show_capture: bool,
    pub capture_title: String,
    pub capture_project_idx: usize,
    pub capture_priority: i32,
    pub capture_field: CaptureField,
    pub project_names: Vec<String>,

    // Focus view state
    pub focus_picks: Vec<String>,
    pub focus_reasons: HashMap<String, String>, // node_id -> reason string

    // Cross-project synergies
    pub synergies: Vec<(String, String, usize)>, // (label_a, label_b, shared_tag_count)

    // Assumption stats
    pub untested_assumptions: Vec<(String, String, f64)>, // (node_id, assumption_text, downstream_weight)

    // Filters
    pub show_completed: bool,
    pub type_filter: Option<String>,

    // Reparenting state
    pub reparent_mode: bool,
    pub reparent_node_id: Option<String>,

    // Stats
    pub total_tasks: usize,
    pub ready_count: usize,
    pub blocked_count: usize,
    pub project_count: usize,
    pub assumption_counts: (usize, usize, usize), // (untested, confirmed, invalidated)
}

/// A search result for the fuzzy search overlay.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub node_id: String,
    pub label: String,
    pub node_type: Option<String>,
    pub score: i64,
}

impl App {
    pub fn new(pkb_root: &Path, _db_path: &Path) -> Self {
        Self {
            pkb_root: pkb_root.to_path_buf(),
            graph: None,
            current_view: View::EpicTree,
            selected_index: 0,
            scroll_offset: 0,
            tree_rows: Vec::new(),
            expanded_nodes: HashSet::new(),
            priority_filter: None,
            show_detail: false,
            detail_node_id: None,
            detail_scroll: 0,
            show_help: false,
            show_search: false,
            search_query: String::new(),
            search_results: Vec::new(),
            search_selected: 0,
            show_capture: false,
            capture_title: String::new(),
            capture_project_idx: 0,
            capture_priority: 2,
            capture_field: CaptureField::Title,
            project_names: Vec::new(),
            focus_picks: Vec::new(),
            focus_reasons: HashMap::new(),
            synergies: Vec::new(),
            show_completed: false,
            type_filter: None,
            reparent_mode: false,
            reparent_node_id: None,
            untested_assumptions: Vec::new(),
            total_tasks: 0,
            ready_count: 0,
            blocked_count: 0,
            project_count: 0,
            assumption_counts: (0, 0, 0),
        }
    }

    pub fn load_graph(&mut self) {
        let gs = GraphStore::build_from_directory(&self.pkb_root);

        // Compute stats
        let ready = gs.ready_tasks();
        let blocked = gs.blocked_tasks();
        let all = gs.all_tasks();
        self.ready_count = ready.len();
        self.blocked_count = blocked.len();
        self.total_tasks = all.len();
        self.project_count = gs.by_project().len();

        // Compute focus picks with reasons
        let picks_with_reasons = select_focus_picks(&ready);
        self.focus_reasons.clear();
        self.focus_picks = picks_with_reasons
            .iter()
            .map(|(n, reason)| {
                self.focus_reasons.insert(n.id.clone(), reason.clone());
                n.id.clone()
            })
            .collect();

        // Compute assumption stats
        let mut untested = 0usize;
        let mut confirmed = 0usize;
        let mut invalidated = 0usize;
        let mut untested_list: Vec<(String, String, f64)> = Vec::new();
        for node in gs.nodes() {
            for a in &node.assumptions {
                match a.status.as_str() {
                    "confirmed" => confirmed += 1,
                    "invalidated" => invalidated += 1,
                    _ => {
                        untested += 1;
                        untested_list.push((
                            node.id.clone(),
                            a.text.clone(),
                            node.downstream_weight,
                        ));
                    }
                }
            }
        }
        untested_list.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        untested_list.truncate(10);
        self.untested_assumptions = untested_list;
        self.assumption_counts = (untested, confirmed, invalidated);

        // Detect cross-project synergies (nodes from different projects sharing tags)
        let active_nodes: Vec<&GraphNode> = gs
            .nodes()
            .filter(|n| !matches!(n.status.as_deref(), Some("done") | Some("dead")))
            .filter(|n| !n.tags.is_empty() && n.project.is_some())
            .collect();
        let mut synergy_pairs: Vec<(String, String, usize)> = Vec::new();
        for (i, a) in active_nodes.iter().enumerate() {
            for b in active_nodes.iter().skip(i + 1) {
                if a.project == b.project {
                    continue;
                }
                let a_tags: HashSet<&str> = a.tags.iter().map(|t| t.as_str()).collect();
                let shared = b
                    .tags
                    .iter()
                    .filter(|t| a_tags.contains(t.as_str()))
                    .count();
                if shared >= 2 {
                    synergy_pairs.push((a.label.clone(), b.label.clone(), shared));
                }
            }
        }
        synergy_pairs.sort_by(|a, b| b.2.cmp(&a.2));
        synergy_pairs.truncate(5);
        self.synergies = synergy_pairs;

        // Collect project names for quick capture
        let by_proj = gs.by_project();
        let mut proj_names: Vec<String> = by_proj.keys().cloned().collect();
        proj_names.sort();
        self.project_names = proj_names;

        self.graph = Some(gs);

        // Auto-expand context nodes (projects, epics, goals)
        if let Some(ref gs) = self.graph {
            for node in gs.nodes() {
                let is_context = matches!(
                    node.node_type.as_deref(),
                    Some("project") | Some("epic") | Some("goal") | Some("subproject")
                );
                if is_context {
                    self.expanded_nodes.insert(node.id.clone());
                }
            }
        }

        self.rebuild_tree();
    }

    /// Rebuild the flattened tree rows from the graph.
    // Toggles
    pub fn toggle_show_completed(&mut self) {
        self.show_completed = !self.show_completed;
        self.rebuild_tree();
    }

    pub fn cycle_type_filter(&mut self) {
        let options = [
            None,
            Some("task".to_string()),
            Some("project".to_string()),
            Some("bug".to_string()),
            Some("feature".to_string()),
            Some("learn".to_string()),
            Some("memory".to_string()),
        ];
        let current_pos = options.iter().position(|x| *x == self.type_filter).unwrap_or(0);
        self.type_filter = options[(current_pos + 1) % options.len()].clone();
        self.rebuild_tree();
    }

    /// Rebuild the flattened tree rows from the graph.
    pub fn rebuild_tree(&mut self) {
        let gs = match &self.graph {
            Some(gs) => gs,
            None => return,
        };

        let mut tasks: Vec<&GraphNode> = gs
            .nodes()
            .filter(|n| {
                // Filter completed
                if !self.show_completed {
                    if matches!(
                        n.status.as_deref(),
                        Some("done") | Some("cancelled") | Some("dead")
                    ) {
                        return false;
                    }
                }

                // Filter by type
                if let Some(ref tf) = self.type_filter {
                    let nt = n.node_type.as_deref().unwrap_or("task");
                    if nt != tf {
                        return false;
                    }
                } else {
                    // Default to actionable types only
                    match n.node_type.as_deref() {
                        Some(t) => {
                            if !graph_store::ACTIONABLE_TYPES.contains(&t) {
                                return false;
                            }
                        }
                        None => {
                            // Untyped nodes: only include if they have a task ID
                            // or live in tasks/ directory
                            let path_str = n.path.to_string_lossy();
                            let in_tasks_dir = path_str.starts_with("tasks/") || path_str.contains("/tasks/");
                            if n.task_id.is_none() && !in_tasks_dir {
                                return false;
                            }
                        }
                    }
                }
                true
            })
            .collect();

        // Sort like ready_tasks did
        tasks.sort_by(|a, b| {
            a.priority
                .unwrap_or(2)
                .cmp(&b.priority.unwrap_or(2))
                .then(
                    b.downstream_weight
                        .partial_cmp(&a.downstream_weight)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
                .then(a.label.cmp(&b.label))
        });

        if tasks.is_empty() {
            self.tree_rows.clear();
            return;
        }

        // Build visible set: tasks + ancestor context nodes
        let context_types = ["project", "epic", "goal", "subproject"];
        let mut visible: HashSet<String> = tasks.iter().map(|t| t.id.clone()).collect();
        let mut context_ids: HashSet<String> = HashSet::new();

        // Any actionable item that is also a context type should be marked as context
        for task in &tasks {
            if let Some(nt) = task.node_type.as_deref() {
                if context_types.contains(&nt) {
                    context_ids.insert(task.id.clone());
                }
            }
        }

        for task in &tasks {
            let mut current_id = task.parent.as_deref();
            while let Some(pid) = current_id {
                if context_ids.contains(pid) {
                    break;
                }
                if let Some(parent_node) = gs.get_node(pid) {
                    if parent_node
                        .node_type
                        .as_deref()
                        .map(|t| context_types.contains(&t))
                        .unwrap_or(false)
                    {
                        context_ids.insert(pid.to_string());
                        visible.insert(pid.to_string());
                    }
                    current_id = parent_node.parent.as_deref();
                } else {
                    break;
                }
            }
        }

        // Group by project
        let mut by_proj: HashMap<String, Vec<&GraphNode>> = HashMap::new();
        for task in &tasks {
            let proj = task.project.as_deref().unwrap_or("_no_project").to_string();
            by_proj.entry(proj).or_default().push(task);
        }

        let mut proj_names: Vec<String> = by_proj.keys().cloned().collect();
        proj_names.sort_by(|a, b| {
            if a == "_no_project" {
                std::cmp::Ordering::Greater
            } else if b == "_no_project" {
                std::cmp::Ordering::Less
            } else {
                a.cmp(b)
            }
        });

        let mut rows: Vec<TreeRow> = Vec::new();

        for proj_name in &proj_names {
            let proj_tasks = by_proj.get(proj_name).unwrap();

            let proj_context: HashSet<String> = context_ids
                .iter()
                .filter(|cid| {
                    gs.get_node(cid)
                        .map(|n| n.project.as_deref() == proj_tasks[0].project.as_deref())
                        .unwrap_or(false)
                })
                .cloned()
                .collect();

            let proj_visible: HashSet<String> = proj_tasks
                .iter()
                .map(|t| t.id.clone())
                .chain(proj_context.iter().cloned())
                .collect();

            // Find roots: nodes whose parent is not in this project's visible set
            let mut roots: Vec<&GraphNode> = proj_visible
                .iter()
                .filter_map(|id| gs.get_node(id))
                .filter(|n| match &n.parent {
                    None => true,
                    Some(pid) => !proj_visible.contains(pid),
                })
                .collect();

            sort_siblings(&mut roots, &context_ids);

            // Flatten tree
            for (i, root) in roots.iter().enumerate() {
                let is_last = i == roots.len() - 1;
                self.flatten_node(
                    gs,
                    root,
                    &proj_visible,
                    &context_ids,
                    0,
                    vec![is_last],
                    &mut rows,
                );
            }
        }

        // Apply priority filter
        if let Some(max_pri) = self.priority_filter {
            rows.retain(|r| r.is_context || r.priority.map(|p| p <= max_pri).unwrap_or(true));
        }

        self.tree_rows = rows;

        // Clamp selection
        if self.selected_index >= self.tree_rows.len() {
            self.selected_index = self.tree_rows.len().saturating_sub(1);
        }
    }

    fn flatten_node(
        &self,
        gs: &GraphStore,
        node: &GraphNode,
        visible: &HashSet<String>,
        context_ids: &HashSet<String>,
        depth: usize,
        is_last_at_depth: Vec<bool>,
        rows: &mut Vec<TreeRow>,
    ) {
        let is_context = context_ids.contains(&node.id);
        let expanded = self.expanded_nodes.contains(&node.id);

        let visible_children: Vec<&GraphNode> = node
            .children
            .iter()
            .filter(|cid| visible.contains(cid.as_str()))
            .filter_map(|cid| gs.get_node(cid))
            .collect();

        let child_count = if is_context {
            count_visible_tasks(gs, &node.id, visible, context_ids)
        } else {
            visible_children.len()
        };

        rows.push(TreeRow {
            node_id: node.id.clone(),
            label: node.label.clone(),
            priority: node.priority,
            status: node.status.clone(),
            node_type: node.node_type.clone(),
            project: node.project.clone(),
            depth,
            is_last_at_depth: is_last_at_depth.clone(),
            is_context,
            child_count,
            downstream_weight: node.downstream_weight,
            stakeholder_exposure: node.stakeholder_exposure,
            due: node.due.clone(),
            created: node.created.clone(),
            task_id: node.task_id.clone(),
            expanded,
            has_children: !visible_children.is_empty(),
        });

        if expanded && !visible_children.is_empty() {
            let mut children: Vec<&GraphNode> = visible_children;
            sort_siblings(&mut children, context_ids);

            for (i, child) in children.iter().enumerate() {
                let child_is_last = i == children.len() - 1;
                let mut child_depths = is_last_at_depth.clone();
                child_depths.push(child_is_last);
                self.flatten_node(
                    gs,
                    child,
                    visible,
                    context_ids,
                    depth + 1,
                    child_depths,
                    rows,
                );
            }
        }
    }

    // Navigation

    pub fn next_view(&mut self) {
        let idx = View::ALL
            .iter()
            .position(|v| *v == self.current_view)
            .unwrap_or(0);
        self.current_view = View::ALL[(idx + 1) % View::ALL.len()];
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    pub fn prev_view(&mut self) {
        let idx = View::ALL
            .iter()
            .position(|v| *v == self.current_view)
            .unwrap_or(0);
        self.current_view = View::ALL[(idx + View::ALL.len() - 1) % View::ALL.len()];
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    pub fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn move_down(&mut self) {
        let max = match self.current_view {
            View::EpicTree | View::Graph => self.tree_rows.len().saturating_sub(1),
            View::Focus => self.focus_picks.len().saturating_sub(1),
            View::Dashboard => 0,
        };
        if self.selected_index < max {
            self.selected_index += 1;
        }
    }

    pub fn expand(&mut self) {
        if let Some(row) = self.tree_rows.get(self.selected_index) {
            if row.has_children && !row.expanded {
                let id = row.node_id.clone();
                self.expanded_nodes.insert(id);
                self.rebuild_tree();
            }
        }
    }

    pub fn collapse(&mut self) {
        if let Some(row) = self.tree_rows.get(self.selected_index) {
            if row.expanded {
                let id = row.node_id.clone();
                self.expanded_nodes.remove(&id);
                self.rebuild_tree();
            }
        }
    }

    pub fn toggle_expand(&mut self) {
        if let Some(row) = self.tree_rows.get(self.selected_index) {
            let id = row.node_id.clone();
            if self.expanded_nodes.contains(&id) {
                self.expanded_nodes.remove(&id);
            } else {
                self.expanded_nodes.insert(id);
            }
            self.rebuild_tree();
        }
    }

    pub fn open_detail(&mut self) {
        let node_id = match self.current_view {
            View::EpicTree | View::Graph => self
                .tree_rows
                .get(self.selected_index)
                .map(|r| r.node_id.clone()),
            View::Focus => self.focus_picks.get(self.selected_index).cloned(),
            _ => None,
        };
        if let Some(id) = node_id {
            self.detail_node_id = Some(id);
            self.detail_scroll = 0;
            self.show_detail = true;
        }
    }

    pub fn toggle_priority_filter(&mut self, pri: i32) {
        if self.priority_filter == Some(pri) {
            self.priority_filter = None;
        } else {
            self.priority_filter = Some(pri);
        }
        self.rebuild_tree();
    }

    /// Get a node by ID from the graph (used by detail view).
    #[allow(dead_code)]
    pub fn get_node(&self, id: &str) -> Option<&GraphNode> {
        self.graph.as_ref()?.get_node(id)
    }

    /// Update search results based on current query (fuzzy substring match).
    pub fn update_search(&mut self) {
        let gs = match &self.graph {
            Some(gs) => gs,
            None => return,
        };
        let query = self.search_query.to_lowercase();
        if query.is_empty() {
            self.search_results.clear();
            return;
        }

        let mut hits: Vec<SearchHit> = gs
            .nodes()
            .filter(|n| {
                if !self.show_completed {
                    if matches!(
                        n.status.as_deref(),
                        Some("done") | Some("dead") | Some("cancelled")
                    ) {
                        return false;
                    }
                }
                true
            })
            .filter_map(|n| {
                let label_lower = n.label.to_lowercase();
                // Score: exact prefix match > contains > tag match
                let mut score: i64 = 0;
                if label_lower.starts_with(&query) {
                    score += 100;
                } else if label_lower.contains(&query) {
                    score += 50;
                }
                // Tag match
                for tag in &n.tags {
                    if tag.to_lowercase().contains(&query) {
                        score += 20;
                    }
                }
                // ID match
                if let Some(ref tid) = n.task_id {
                    if tid.to_lowercase().contains(&query) {
                        score += 30;
                    }
                }
                if score > 0 {
                    Some(SearchHit {
                        node_id: n.id.clone(),
                        label: n.label.clone(),
                        node_type: n.node_type.clone(),
                        score,
                    })
                } else {
                    None
                }
            })
            .collect();

        hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.label.cmp(&b.label)));
        hits.truncate(20);
        self.search_results = hits;
        self.search_selected = 0;
    }

    /// Open detail for the currently selected search result.
    pub fn open_search_result(&mut self) {
        if let Some(hit) = self.search_results.get(self.search_selected) {
            self.detail_node_id = Some(hit.node_id.clone());
            self.detail_scroll = 0;
            self.show_detail = true;
            self.show_search = false;
        }
    }

    #[allow(dead_code)]
    pub fn open_capture(&mut self) {
        self.show_capture = true;
        self.capture_title.clear();
        self.capture_project_idx = 0;
        self.capture_priority = 2;
        self.capture_field = CaptureField::Title;
    }

    /// Create a new task markdown file from capture fields and reload graph.
    #[allow(dead_code)]
    pub fn submit_capture(&mut self) -> bool {
        let title = self.capture_title.trim().to_string();
        if title.is_empty() {
            return false;
        }

        let project = self.project_names.get(self.capture_project_idx).cloned();
        let priority = self.capture_priority;

        // Generate a filename-safe slug
        let slug: String = title
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .to_string();
        let slug = if slug.len() > 50 {
            slug[..50].to_string()
        } else {
            slug
        };

        let tasks_dir = self.pkb_root.join("incoming");
        if !tasks_dir.exists() {
            let _ = std::fs::create_dir_all(&tasks_dir);
        }

        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let filename = format!("{slug}.md");
        let filepath = tasks_dir.join(&filename);

        // Build YAML frontmatter
        let mut frontmatter = format!(
            "---\ntitle: \"{}\"\ntype: task\nstatus: active\npriority: {}\ncreated: {}\n",
            title.replace('"', "\\\""),
            priority,
            today,
        );
        if let Some(ref proj) = project {
            frontmatter.push_str(&format!("project: {proj}\n"));
        }
        frontmatter.push_str("---\n\n");

        // Write the file
        match std::fs::File::create(&filepath) {
            Ok(mut f) => {
                let _ = f.write_all(frontmatter.as_bytes());
            }
            Err(_) => return false,
        }

        // Reload graph
        self.load_graph();
        self.show_capture = false;
        true
    }

    #[allow(dead_code)]
    pub fn change_priority(&mut self, delta: i32) {
        let node_id = match self.current_view {
            View::EpicTree | View::Graph => self
                .tree_rows
                .get(self.selected_index)
                .map(|r| r.node_id.clone()),
            View::Focus => self.focus_picks.get(self.selected_index).cloned(),
            _ => None,
        };
        if let Some(id) = node_id {
            if let Some(node) = self.get_node(&id) {
                let current_pri = node.priority.unwrap_or(2);
                let new_pri = (current_pri + delta).clamp(0, 4);
                self.set_priority(&id, new_pri);
            }
        }
    }

    #[allow(dead_code)]
    pub fn set_status(&mut self, id: &str, status: &str) {
        if let Some(node) = self.get_node(id) {
            let path = &node.path;
            let mut updates = HashMap::new();
            updates.insert("status".to_string(), serde_json::json!(status));

            if let Err(_) = crate::document_crud::update_document(path, updates) {
                // Ignore error
            } else {
                self.load_graph();
            }
        }
    }

    #[allow(dead_code)]
    pub fn set_priority(&mut self, id: &str, priority: i32) {
        if let Some(node) = self.get_node(id) {
            let path = &node.path;
            let mut updates = HashMap::new();
            updates.insert("priority".to_string(), serde_json::json!(priority));

            if let Err(_) = crate::document_crud::update_document(path, updates) {
                // Ignore error
            } else {
                self.load_graph();
            }
        }
    }

    #[allow(dead_code)]
    pub fn set_parent(&mut self, child_id: &str, parent_id: Option<&str>) {
        if let Some(node) = self.get_node(child_id) {
            let path = &node.path;
            let mut updates = HashMap::new();
            match parent_id {
                Some(pid) => {
                    updates.insert("parent".to_string(), serde_json::json!(pid));
                }
                None => {
                    updates.insert("parent".to_string(), serde_json::Value::Null);
                }
            }

            if let Err(_) = crate::document_crud::update_document(path, updates) {
                // Ignore error
            } else {
                self.load_graph();
            }
        }
    }

    #[allow(dead_code)]
    pub fn enter_reparent_mode(&mut self) {
        let node_id = match self.current_view {
            View::EpicTree | View::Graph => self
                .tree_rows
                .get(self.selected_index)
                .map(|r| r.node_id.clone()),
            View::Focus => self.focus_picks.get(self.selected_index).cloned(),
            _ => None,
        };

        if let Some(id) = node_id {
            self.reparent_mode = true;
            self.reparent_node_id = Some(id);
        }
    }

    #[allow(dead_code)]
    pub fn confirm_reparent(&mut self) {
        if !self.reparent_mode {
            return;
        }

        let target_parent_id = match self.current_view {
            View::EpicTree | View::Graph => self
                .tree_rows
                .get(self.selected_index)
                .map(|r| r.node_id.clone()),
            _ => None,
        };

        if let (Some(child_id), Some(parent_id)) = (self.reparent_node_id.clone(), target_parent_id)
        {
            if child_id != parent_id {
                self.set_parent(&child_id, Some(&parent_id));
            }
        }

        self.reparent_mode = false;
        self.reparent_node_id = None;
    }
    pub fn poll_worker(&mut self) {
        // Placeholder for future background task polling
    }
}

/// Select top focus picks
fn select_focus_picks<'a>(tasks: &[&'a GraphNode]) -> Vec<(&'a GraphNode, String)> {
    let mut scored: Vec<(&GraphNode, f64, String)> = tasks
        .iter()
        .map(|t| {
            let mut score = 0.0;
            let mut reasons: Vec<&str> = Vec::new();
            // P1 tasks get massive boost
            if t.priority == Some(1) || t.priority == Some(0) {
                score += 1000.0;
                reasons.push("high priority");
            }
            // Downstream weight (unblocks other work)
            if t.downstream_weight > 0.0 {
                score += t.downstream_weight * 10.0;
                reasons.push("unblocks other work");
            }
            // Stakeholder exposure
            if t.stakeholder_exposure {
                score += 50.0;
                reasons.push("stakeholder visibility");
            }
            // Due date urgency
            if t.due.is_some() {
                score += 30.0;
                reasons.push("has deadline");
            }
            // Staleness — older tasks get a boost
            if let Some(ref created) = t.created {
                if let Ok(dt) = chrono::NaiveDate::parse_from_str(created, "%Y-%m-%d") {
                    let days = (chrono::Local::now().date_naive() - dt).num_days();
                    score += (days as f64).min(60.0);
                    if days > 14 {
                        reasons.push("aging");
                    }
                }
            }
            if reasons.is_empty() {
                reasons.push("ready");
            }
            let reason = reasons.join(" + ");
            (*t, score, reason)
        })
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .take(5)
        .map(|(t, _, reason)| (t, reason))
        .collect()
}

/// Sort siblings: context nodes first, then tasks by priority/weight.
fn sort_siblings(nodes: &mut Vec<&GraphNode>, context_ids: &HashSet<String>) {
    nodes.sort_by(|a, b| {
        let a_ctx = context_ids.contains(&a.id);
        let b_ctx = context_ids.contains(&b.id);
        match (a_ctx, b_ctx) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (true, true) => a.label.cmp(&b.label),
            (false, false) => a
                .priority
                .unwrap_or(2)
                .cmp(&b.priority.unwrap_or(2))
                .then(
                    b.downstream_weight
                        .partial_cmp(&a.downstream_weight)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
                .then(a.label.cmp(&b.label)),
        }
    });
}

/// Count visible task descendants (not context nodes) under a node.
fn count_visible_tasks(
    gs: &GraphStore,
    node_id: &str,
    visible: &HashSet<String>,
    context_ids: &HashSet<String>,
) -> usize {
    let mut count = 0;
    if let Some(node) = gs.get_node(node_id) {
        for child_id in &node.children {
            if visible.contains(child_id.as_str()) {
                if context_ids.contains(child_id.as_str()) {
                    count += count_visible_tasks(gs, child_id, visible, context_ids);
                } else {
                    count += 1;
                }
            }
        }
    }
    count
}
