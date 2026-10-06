---
id: pkb-flow-engine
title: "PKB engine and tools for the flow rule"
type: spec
status: draft
created: 2026-10-06
task: epic_2de1b579
epic: aops_ee1257cb
brief: spec_866ee53d
depends_on_spec: flow-rule
tags:
  - ranking
  - flow-rule
  - mcp-tools
  - schema
  - spec
---

# PKB engine and tools for the flow rule

**Home:** `specs/pkb-flow-engine.md` in the `nicsuzor/mem` repository. Its checking scripts and the reference oracle are in `specs/pkb-flow-engine/`.

**Status: draft for Nic's decision.** This spec says what the PKB server stores, computes and returns once the rule in [`flow-rule.md`](flow-rule.md) replaces the ranking in [`ranking.md`](ranking.md). It specifies and does not implement. Until Nic approves it and the build lands, `ranking.md` still describes what ships. This PR leaves `ranking.md` unchanged, because `tests/schema_doc_integrity.rs:122` requires it to exist and to cover today's measures.

**What this spec leaves to its sibling specs:**

| Topic | Owning spec |
|---|---|
| The rule, the edge fields and their scales | flow-rule (`epic_a463f704`, merged as `specs/flow-rule.md`) |
| Moving today's frontmatter into the new schema | migration (`epic_d1679d4b`) |
| Linter checks on the new schema | linter (`epic_fc1de9ec`) |
| How the dashboard shows the figures | dashboard (`epic_adf10cd3`) |
| Skills and agent logic: densify, triage, soft-to-hard deadlines | skills (`epic_80ce44ae`) |

Traceability tags are as in `flow-rule.md`, plus two of this spec's own:

- **S1–S16:** settled points in the brief (`spec_866ee53d`, "Settled by Nic").
- **I1–I17:** the brief's invariants (section C).
- **U1–U20:** user stories in `pkb-arch-framework`.
- **Q1–Q31:** questions already open in `flow-rule.md` §15.
- **E1–E16:** questions this spec adds (section 10).
- **R1–R28:** this spec's requirements.
- **T-…:** the tests in section 9.

---

## 0. Summary for Nic

**What the server will store.**

- Each link becomes one entry in a single `links:` list in frontmatter. An entry says where the link goes, what it is for (its label), how much of the far end it delivers (its *quantum*), how likely that is (its *probability*, default 1.00), and whether it helps or harms.
- Each target gets a `worth:` from −1 to +1. You set it.
- Each dated node gets a `deadline_class:` of fake, soft or hard.
- Nothing else feeds the maths.

**What the server will compute.** For every open piece of work it computes three numbers:

- *gain*: what you would fail to gain if the work were never done;
- *loss averted*: what loss you would fail to avert;
- *decision value*: what finding out is worth, for work that settles an open decision.

The three are never added together. The server also records which priced targets each figure comes from, and through which routes. The maths sits in its own module, which cannot see dates, types, tags, severity, stakeholders or priority. Ready lists, the hard-deadline cliff lane and ordering sit in a separate display layer.

**What each tool returns.**

- `get_task` shows the three figures, the share of each target at stake, the strongest routes to each target, and a one-sentence explanation (U18). Example: "serves `task_b3f01c80` × 1.00 and `targ_4e2cc92a` × 0.85, both via `proj-f8b942d5` → `brain_bf2be9d8`."
- `list_tasks`, `nested_tasks`, `export_graph` and the CLI show the three figures per row and never add worth across tasks.
- `focus_score`, the eleven `signals`, `downstream_weight`, `urgency`, `value_lineage`, `effective_intent`, `cost_of_delay`, `severity_gate` and `queue_rank` all disappear from every output (section 7).

**What it costs.** Measured with the reference calculator on the committed copy of your graph (3,710 nodes, 1,502 open, 1,502 links that carry worth):

- A full recompute takes under 0.1 s in Python with all 26 targets priced (section 5.4). The Rust build must stay under 0.25 s, and a test enforces it.
- If wikilinks were also read as links (Q23), the graph would form one loop of 852 nodes. Each recompute would then do 1,250 times the work and take 36 to 46 s in Python. This spec's cost budget assumes wikilinks stay out.

**What it needs from you.** This spec adds sixteen questions (E1–E16, section 10). Ten questions already open in `flow-rule.md` change what gets built here. The three that matter most:

- **E1.** Store links as one `links:` list, as proposed, or keep today's separate keys with new fields added?
- **E3.** When a loop is rejected or fails to settle, should the work feeding it show "no figure", with the loop named, as proposed? The reference calculator shows 0.0, which looks the same as "linked to nothing".
- **Q3** (from flow-rule). What orders the list? Until you answer, this spec proposes gain first, then loss averted, then id. That is a pending default, not a decision.

---

## 1. Problem and target

**Problem.**

- **The pipeline.** Ranking is computed by eleven pipeline stages in `GraphStore::build_internal`: `compute_downstream_metrics` through `compute_focus_scores` and `compute_target_ancestors` (`src/graph_store.rs:497-542`).
- **The outputs.** They are written to about twenty fields on `GraphNode` and returned by at least twelve tools and five CLI commands (section 7).
- **Five orderings.** The server sorts tasks in five different ways:
  - `focus_cmp` (`src/graph_store.rs:969`);
  - the ready list, urgency first (`src/graph_store.rs:4452-4472`);
  - `actionable_tasks`, urgency first (`src/graph_store.rs:1012`);
  - the mcp-index ready list, by intent then `downstream_weight` (`src/task_index.rs:279`);
  - `focus_picks`, urgent nodes first (`src/graph_store.rs:2153`).
- **Silent data loss.** One malformed `contributes_to` entry silently drops the whole list (`src/graph.rs:1600-1604`: `.ok().unwrap_or_default()`).

**Target.** The target has five parts:

- one stored edge form;
- one stored target worth;
- one maths module computing the rule of `flow-rule.md` §3;
- one display layer, with one ordering;
- tool outputs that carry gain, loss averted, decision value and their routes, and no other ranking number.

---

## 2. Architecture and data flow

```text
frontmatter                    parse (graph.rs)              maths: src/flow.rs (new)           display: src/display_rank.rs (new)      tools / CLI
───────────                    ────────────────              ────────────────────────           ──────────────────────────────────      ───────────
links: [...]        ─┐         Link {from,to,label,     ─┐   FlowInput {ids, state, worth,  ─►  FlowOutput per open node        ─►  ready / blocked (needs edges)   ─►  get_task, list_tasks,
worth: ±x            ├──────►        quantum,prob,       ├──►            edges}                  gain, loss_averted,                 cliff lane (hard deadlines)          nested_tasks, export_graph,
status               │               effect, set_by}     │   SCC + baseline + knockouts          decision_value, stake,              display order (one comparator)       top_n_by_metric, CLI …
deadline_class, due ─┘         ParseWarning per entry   ─┘   decision rule (EVPI)                loop_extra, flow_status
                                                              routes (on demand)
```

**R1. The maths module.** `src/flow.rs` is the only code that computes worth. Its single input type is `FlowInput`, which holds:

- node ids;
- node state (`open | done | gone`);
- target worth;
- the edge fields of `flow-rule.md` §5.1.

`FlowInput` has no field for a date, type, tag, stakeholder, severity, intent, effort or deadline class, so the maths cannot read them (S1, S5, I6, I11).

**R2. The display layer.** `src/display_rank.rs` reads `FlowOutput` and any node field it needs. It writes nothing that `flow.rs` reads (I6).

**R3. The pipeline.** `build_internal` (`src/graph_store.rs:272-601`) becomes:

```text
canonical node sort (id ASC)                  kept   (ranking.md §1.1)
  → compute_inverses                          kept   (resolves link endpoints, builds incoming lists)
  → compute_degree_metrics                    kept   (diagnostic)
  → compute_centrality_metrics                kept   (diagnostic; skipped on fast rebuilds as today)
  → build_flow_input                          new    (§3.1)
  → compute_flow                              new    (§5)
  → compute_decision_value                    new    (§5.3)
  → compute_scope                             kept   (display: size of the part_of tree)
  → compute_project_field                     kept
  → [similarity edges, if requested]          kept
  → classify_tasks                            rewritten in display_rank.rs (§6)
  → compute_divergence_anomalies              rewritten to read strength (§7.3)
```

The stages removed are listed in §7.1.

---

## 3. Stored schema

### 3.1. Links (S2, S3)

**R4. One edge form.** Every link that can carry worth is one entry in a `links:` list in the frontmatter of either end (E1, E2):

```yaml
links:
  - to: targ_4e2cc92a          # exactly one of `to` or `from`; the other end is this node
    label: serves              # flow-rule §5.2: serves | needs | part_of | supports | alternative | settles
    quantum: 0.6               # 0.0..=1.0, or a word from flow-rule §5.4; omitted → default quantum (Q2)
    probability: probable      # 0.0..=1.0, or a word from flow-rule §5.5; omitted → 1.00
    effect: helps              # helps | harms; omitted → helps
    justification: "Second chapter of the monograph"   # free text, optional
    set_by: nic                # nic | agent-proposed | migrated; omitted → agent-proposed
```

- **`to` is the natural form for work.** "This task serves that target" is written on the task.
- **`from` is the natural form for blocked work.** "This task needs that one done first" is written on the blocked task as `{from: X, label: needs}`. In the maths, the edge always runs from the work to what it serves (X → this node), as in `flow-rule.md` §5.1.
- **Words are stored as written.** The parser maps them to numbers, so Nic's words stay visible in the file (E2).
- **Duplicates.** One logical edge declared at both ends (A lists `to: B` and B lists `from: A` with the same label) is read once. If the two declarations disagree on any field, the parser emits a `ParseWarning` and reads the entry on the `from` node (E4).

**R5. Parsing.** Each `links` entry is parsed on its own.

- **A bad entry.** A malformed entry yields one `ParseWarning` naming the entry's index and field, and is dropped. The other entries are kept. This fixes today's silent whole-list drop (`src/graph.rs:1600-1604`).
- **A recognised word.** It maps to its number.
- **An unrecognised word.** It yields a `ParseWarning`, and the field is read as unstated: quantum takes the default quantum and probability takes 1.00. The parser never invents a number. This follows `ranking.md:475-476` for `stated_weight`.
- **A number out of range.** A quantum or probability outside `0.0..=1.0`, NaN or infinite yields a `ParseWarning`, and the field is read as unstated.
- **A label outside the set.** It yields a `ParseWarning`, and the entry is dropped (Q21).

**R6. The parent field.** What happens to the `parent:` field depends on Q1:

- **If Q1 keeps it:** `parent: P` is read as the link `{to: P, label: part_of}`, with the quantum Q1 fixes. It is not a second edge kind.
- **If Q1 removes it:** the field is no longer read by the maths. Grouping reads `part_of` links (§6).

**R7. Old keys during migration.** Until the migration spec retires them, the parser maps today's keys into `links` with the defaults in `flow-rule.md` §8:

| Old key | Read as |
|---|---|
| `depends_on: [X]` | `{from: X, label: needs, quantum: 1.0}` |
| `soft_depends_on: [X]` | `{from: X, label: supports, quantum: 0.3, set_by: migrated}` |
| `contributes_to: [{to, stated_weight, multiplier}]` | `{to, label: serves, quantum: min(1, numeric_weight), set_by: migrated}` |

When a node carries both a mapped old key and a `links` entry for the same ordered pair and label, the `links` entry wins, with a `ParseWarning`. The migration spec decides when the old keys stop being read (E5).

**R8. Not edges of the flow.** The following are never read by the maths (Q23, Q30):

- wikilinks (`src/graph.rs:1079-1102`);
- `supersedes`;
- `closes`;
- similarity edges.

They stay as today for search, `pkb_trace` and display.

### 3.2. Target worth (S12, S14)

**R9. The `worth` field.** `worth:` is a frontmatter value, either:

- a float in `-1.0..=1.0`; or
- an anchor word from `flow-rule.md` §5.6: `critical`, `high`, `substantial`, `moderate`, `low`, or the loss words `catastrophic`, `severe`, `substantial loss`, `moderate loss`, `minor loss`.

Out-of-range values, unknown words, NaN and infinity yield a `ParseWarning`, and the node is read as unpriced. An unpriced node has worth 0, which is never inferred (U20).

- **Any node may carry it.** The maths reads `worth` on any node, because the maths never reads type (`flow-rule.md` §2). The linter, not the maths, flags `worth` on a node that is not a target (owned by `epic_fc1de9ec`).
- **The old field.** `standing_weight` (`src/graph.rs:1480-1505`, `0.0..=1.0`) is read as `worth` until the migration spec retires it. If both are present, `worth` wins, with a `ParseWarning`.

### 3.3. Deadline class (S5, S13)

**R10. The `deadline_class` field.** `deadline_class: fake | soft | hard` sits on any node with a `due` date.

- **Unclassed dates.** An unclassed `due` is read as `fake` (Q18).
- **Who reads it.** Only the display layer reads it. The maths never does (R1).
- **Extension history.** How a soft deadline becomes hard is decided by the skills spec. The server stores only the current class (E6).

### 3.4. Fields kept but not read by the maths

The following stay as stored frontmatter, are returned as plain fields, and are never read by `flow.rs` (S1):

- `intent` / `priority` (Q10);
- `severity`;
- `goal_type`;
- `stakeholder`;
- `waiting_since`;
- `consequence`;
- `effort`;
- `confidence`;
- `affordable_loss`.

`effort` is read by the display layer, for the cliff lane and for benefit per effort.

---

## 4. Engine configuration

**R11. Constants.** The open numbers are constants in one place, `src/flow.rs` and `src/display_rank.rs`, each named after the question that will set it:

| Constant | Proposed value | Set by |
|---|---|---|
| `DEFAULT_QUANTUM` | 0.0 | Q2 |
| `PART_OF_QUANTUM` | 1.0 | Q1 |
| `CLIFF_BUFFER_DAYS` | 7 | Q9 |
| `ROUTE_CAP` | 5 per target | E8 |
| `TOL` | 1e-12, as `flow.py:39` | — |
| `ITERATION_CAP` | 20,000 sweeps per component, as `flow.py:129` and `flow.py:162` | — |

Changing a constant is a pull request (S11: simple beats complicated). Moving them to runtime configuration is E7.

---

## 5. Computing the flow

### 5.1. Algorithm

`compute_flow` computes exactly the quantities `flow-rule.md` §3.2 defines. The reference is `worth_all` (`specs/flow-rule/flow.py:216`). The engine may compute them faster, but must give the same numbers (T-parity).

1. **Flow edges.** Keep the edges whose label is not `alternative` or `settles`, whose endpoints are both not `gone`, and whose strength `quantum × probability` is above 0 (`flow.py:73-82`).
2. **Loops.** Find the loops: Tarjan's strongly connected components over flow edges, as `tarjan_scc` does today (`src/graph_store.rs:4707`). Then order the component graph topologically, breaking ties by node id.
3. **Saturated loops.** Mark as *saturated* each component in which open nodes are joined by full-strength `helps` edges into a loop (`flow.py:208-213`; S15).
4. **Baseline.** Compute the baseline `y⁰`, under which every open node is assumed done and only harms from done nodes fire (`flow-rule.md` §3.2, "Baseline and harms semantics"; Q20). In a graph with no harms this is 1.0 everywhere, so no work is needed.
5. **Knockouts.** For each open node `u` whose forward cone reaches a priced target:
   - knock `u` out, or for harms, compare `u` done against `u` knocked out (`flow-rule.md` §3.2);
   - propagate forward over `u`'s cone only, in topological order of components;
   - outside loops, one pass;
   - inside a loop with only `helps` edges, Gauss–Seidel sweeps;
   - inside a loop with any `harms` edge, synchronous damped steps at damping 0.5 (`flow.py:124-158`);
   - stop at `TOL` or raise `NoConvergence` for that component.
6. **Figures.** Read `δ_t(u)` for each priced target `t` in the cone, and form `gain`, `loss_averted` and `stake` exactly as `flow-rule.md` §3.2. Round each figure to 9 decimal places, as `_clean` does (`flow.py:264`).
7. **Loop extra.** Record `loop_extra_t(u)` as `δ_t(u)` minus the loop-free bound `1 − ∏(1 − route strength)` (`flow-rule.md` §3.2, P3), when the difference is positive.

**R12.** Every open node gets a `FlowOutput`. A node with no priced target in its cone gets `gain = loss_averted = decision_value = 0` and `flow_status = "ok"` (I7).

**Done nodes carry their parent's worth no further.** A done node holds `y = 1` (`flow-rule.md` §3.2), so an open child of a done parent carries nothing through that parent. On the fixture, `n_4b3ea8a012` is `part_of` the done `n_65c051e9f4`, and carries 0. This follows from the rule. Whether it is the behaviour Nic wants is E9.

### 5.2. Failure (S15)

**R13. Failure status.** Each `FlowOutput` carries `flow_status`, which takes one of three values:

- `ok`;
- `saturated_loop`, with `loop: [ids]`;
- `no_convergence`, with `loop: [ids]`.

A node whose cone touches a saturated or unsettled component gets `flow_status ≠ ok`. Its `gain`, `loss_averted` and `decision_value` are `null`, not 0.

- **Other nodes are unaffected.** They are ranked as normal (`flow-rule.md` §3.2, lines 156 and 158).
- **The linter names the loop** (`epic_fc1de9ec`).

**This departs from the reference, and resolves a conflict (E3).**

- **The reference shows zero.** `flow.py` returns `Worth(0.0, 0.0, {})` for such nodes (`flow.py:239-240`, `flow.py:254-255`), which reads the same as "linked to nothing" (U7, I7).
- **The foundation spec contradicts itself.** `flow-rule.md:634` says "nothing is ranked from a partial result", while `flow-rule.md:156` and `flow-rule.md:158` say unaffected components remain ranked.

This spec follows lines 156 and 158, and reports `null` instead of 0.

### 5.3. Decision value

**R14. `compute_decision_value`** runs after `compute_flow`. It implements `flow-rule.md` §6 exactly, as `decision_worth` (`flow.py:288`):

- For each open node with at least two incoming `alternative` edges, compute EVPI over its options.
- Add `quantum × EVPI` to each open node with a `settles` edge into it.

**Two cautions.**

- **The comparison scalar.** The reference compares options by `gain + loss_averted` (`flow.py:295`). Whether that sum is allowed is Q4. Until Q4 is answered, the engine does the same, and the tool description says so.
- **Its own figure.** `decision_value` is never added to `gain` or `loss_averted`, in storage or in any output (S16).

### 5.4. Cost at the live graph's size (I13)

`python3 specs/pkb-flow-engine/cost.py` prints these figures. It runs the reference calculator unchanged on the committed fixture `specs/flow-rule/fixtures/live-2026-10-05.json`.

| Scenario | Flow edges | Priced targets | Open nodes reaching a priced target | Σ cone edges (max per node) | Loops (largest) | Python `worth_all`, median per run, over three runs |
|---|---|---|---|---|---|---|
| A: fixture as committed | 1,502 | 7 | 247 | 1,571 (35) | 3 (4 nodes) | 0.035–0.051 s |
| B: every target priced at +0.35 (**assumed**) | 1,502 | 26 | 370 | 2,126 (35) | 3 (4 nodes) | 0.041–0.090 s |
| C: B, plus wikilinks as links at 0.05 (**assumed**, Q23) | 6,027 | 26 | 672 | 2,659,951 (4,054) | 34 (852 nodes) | 35.7–45.6 s (one timing per run) |

**Recompute scope.** These are the nodes whose figures can change when one node's state, worth or links change: its upstream set (P4).

| Scenario | Median | 95th percentile | 99th percentile | Largest |
|---|---|---|---|---|
| B | 0 | 3 | 22 | 460 |
| C | 0 | 1,346 | 1,347 | 1,482 |

**What the counts mean.** The counts are identical on every run. Wall times vary with machine load, so only their order of magnitude is used.

- **The live graph is smaller than the fixture.** The brief describes the live graph as "about 1,900 nodes and 3,700 edges". The fixture holds 3,710 nodes, including done nodes, and 6,744 stored edges, of which 4,958 are wikilinks. Only 1,502 edges carry worth. Every cost above is measured on the fixture, which is the larger graph.
- **Cost is the sum of cone edges.** The knockout rule costs `O(Σ_u |E(cone(u))|)` edge visits, plus the iterations inside any loop a cone crosses. In the worst case that is `O(V · E)`.
- **Scenario B costs little.** It is 2,126 edge visits.
- **Scenario C dominates.** One loop of 852 nodes, iterated inside the cone of every node that feeds it, accounts for most of its 2.66 million.

**R15. The cost budget.** At scenario B scale, a release build of `compute_flow` plus `compute_decision_value` completes in under 0.25 s on the CI runner (T-cost). Today's whole background rebuild "typically completes in 1–3s" (`.agent/CORE.md`, "Graph rebuild"), so the flow takes at most a quarter of the low end. The budget assumes Q23 keeps wikilinks out of the flow. If Nic decides otherwise, this section must be revisited before build.

**R16. Recompute on every rebuild.** The flow is recomputed in full on every background rebuild (`schedule_graph_rebuild`, `src/mcp_server/mod.rs:559-753`) and on every full rebuild.

- **On a single write.** The synchronous in-place patch carries over the node's previous `FlowOutput`, as it carries over `focus_score` today (`src/graph_store.rs:764-775`). The figures are "as of the last rebuild" until the background rebuild lands.
- **No incremental recompute.** Scenario B's largest upstream set is 460 nodes, and a full run is cheap, so incremental recompute is not worth its complexity (S11; E10).

**R17. Determinism.** `FlowOutput` is a pure function of `FlowInput`, and byte-identical across all build entry points from identical input. This keeps `ranking.md` §1.1:

- Nodes and components are visited in id order.
- Loops containing a harms edge use synchronous updates, so the result does not depend on id order (`flow.py:124-158`).
- Figures are rounded to 9 places.

### 5.5. Routes (I12, U18)

**R18. Routes on demand.** Routes are computed when asked for, not stored. For node `u` and each priced target `t` with `δ_t(u) ≠ 0`, `routes(u)` returns three things.

**1. The strongest route.**

- It is the route whose product of strengths has the largest absolute value.
- It is found exactly, as a maximum-product path over `u`'s cone. For a route with no harms edge this is Dijkstra's algorithm on `−log(strength)`.
- Each route lists its node ids, its strength (signed when it crosses a harms edge), and its labels.

**2. Further simple routes.** Up to `ROUTE_CAP − 1` more, strongest first, with `routes_truncated: true` when more exist. The reference `routes()` (`flow.py:311-331`) enumerates every simple route up to length 12. The engine must not, because the number of routes can grow exponentially.

**3. The loop extra,** `loop_extra_t(u)`, when it is positive.

**The one-sentence explanation.** The explanation string is built from the strongest route to each target:

> "`<u>` serves `<t1>` × `<δ>` and `<t2>` × `<δ>`, via `<path>`."

Its form follows `flow-rule.md` §12. The cost is one maximum-product search per (node, target) pair over a cone. On the fixture the largest cone has 35 edges (§5.4).

---

## 6. The display layer (S1, S4, S5, S13)

**R19. One comparator.** `display_cmp` is the only ordering of work in the server. It replaces all five orderings listed in §1.

1. **The cliff lane first.** These are nodes with `deadline_class: hard` and `days_left ≤ effort_days + CLIFF_BUFFER_DAYS`, ordered by `due` ascending (`flow-rule.md` §7; `display.py:47-52`; Q26). Fake and soft dates never enter it (I17).
2. **Then the Nic key (Q3).** Until Q3 is answered, the proposed key is `gain` DESC, then `loss_averted` DESC, then `decision_value` DESC. This is lexicographic, so it never nets the figures. A task with gain 0.95 and loss −0.95 sorts with tasks of gain 0.95, not with tasks linked to nothing (I15).
3. **Then nodes with no figure.** Nodes whose `flow_status ≠ ok` come next, then nodes with all figures at 0.
4. **Then id ASC,** a deterministic total order.

**R20. Ready, blocked and roots.** These keep their meaning from `ranking.md:497-517`, rewritten in `display_rank.rs`:

- **Blocked** means at least one incoming `needs` link, or mapped `depends_on`, from a node that is not `done` or `cancelled`, or being downstream of a blocked node along `needs`.
- **Ready** means a leaf, of a claimable type, with an actionable status, and not blocked. The exact type and status gates are Q31.
- **Ordering.** The ready list is ordered by `display_cmp`, not urgency first as today (`src/graph_store.rs:4452-4472`).
- **Separation.** None of these feeds the maths. Blocked work keeps its figures (S4, I5).

**R21. Never summed.** No tool, CLI command or export emits a sum, mean or total of `gain`, `loss_averted` or `decision_value` across two or more nodes (`flow-rule.md` §4.1, "What it breaks"). Group views such as `nested_tasks` and `task_summary` show counts and the share of children done, never added worth.

**R22. Benefit per effort.** Display may show `gain / effort_days` and `loss_averted / effort_days` side by side, for U16. These are display fields, never stored inputs. Whether a sort by them is offered is Q12. Until then, `list_tasks` offers it as a non-default `sort` value (§7.2).

---

## 7. Interfaces

### 7.1. What is removed

**Pipeline stages removed** from `build_internal` (`src/graph_store.rs:497-542`):

- `compute_downstream_metrics`;
- `compute_effective_intent`;
- `compute_blocking_urgency`;
- `compute_urgency`, together with `chain_slack`;
- `compute_uncertainty`;
- `compute_criticality` (E11);
- `compute_voi_term`;
- `compute_value_lineage`;
- `compute_unlock_breadth`;
- `compute_focus_scores`;
- `compute_target_ancestors` (E12).

**Functions removed:**

- `compute_cost_of_delay` (`:1820-2033`);
- `focus_cmp` (`:969`), replaced by `display_cmp`;
- `focus_picks` (`:2111-2186`), replaced by the head of `display_cmp` over ready work plus human gates;
- `actionable_tasks`' urgency-first sort (`:1012`).

**`GraphNode` fields removed** (`src/graph.rs:358-618`), with the reason for each in `flow-rule.md` §9:

- `focus_score`, `focus_tuple`;
- `downstream_weight`, `stakeholder_exposure`;
- `effective_intent`;
- `urgency`, `blocking_urgency`, `chain_slack`;
- `unlock_breadth`;
- `value_lineage`;
- `voi_value`;
- `uncertainty`;
- `criticality` (E11);
- `affordable_loss_filtered`;
- `target_ancestors` (E12);
- `ContributesTo.current_weight`, `.multiplier` and `.inherits_from` (`src/graph.rs:239-278`; `inherits_from` is E13).

**Fields added:**

- `links: Vec<Link>`;
- `worth: Option<f64>`;
- `deadline_class: Option<DeadlineClass>`;
- `flow: Option<FlowOutput>`, serialised under the name `flow`.

**Kept as diagnostics:** `pagerank`, `betweenness` and the degree counts (`ranking.md` §5; `src/metrics.rs`). They never enter `flow.rs` or `display_cmp`.

### 7.2. Tool contracts

Every tool's description in `src/mcp_server/schemas.rs` is updated to match. A `FlowOutput` serialises as follows:

```json
"flow": {
  "gain": 1.11, "loss_averted": 0.0, "decision_value": 0.0,
  "flow_status": "ok",
  "stake": {"task_b3f01c80": 1.0, "targ_4e2cc92a": 0.85},
  "loop_extra": {}
}
```

When `flow_status ≠ ok`, the three figures are `null` and a `loop: [ids]` key is present (R13).

| Tool | Returns (new) | Removed from output or input | Today's builder |
|---|---|---|---|
| `get_task` | `flow` (above); `routes: {target: [{strength, path, labels}]}`, `routes_truncated`, `explanation` (one sentence, §5.5) when `include_routes` (default true); `links_out`, `links_in` with every field of §3.1 plus `strength`; `display: {ready, blocked, on_cliff, cliff_enters_on, deadline_class, days_until_due}` | `focus_score`, `effective_intent`, `stakeholder_exposure`, `target_ancestors`, `urgency_ratio`, `standing_weight` (shown as `worth`), the `signals` object and its 11 keys; the `include_signals` input, replaced by `include_routes` | `src/mcp_server/handlers_task.rs:823-883` |
| `claim_task` | as `get_task` (it returns `get_task`) | as `get_task` | `src/mcp_server/handlers_task_lifecycle.rs:12-90` |
| `list_tasks` (json) | per row: `id`, `title`, `status`, `gain`, `loss_averted`, `decision_value`, `flow_status`, `ready`, `blocked`, `on_cliff`, `due`, `deadline_class`, `effort`; ordered by `display_cmp` | `focus_score`, `effective_intent`, `signals` | `handlers_task.rs:1487-1525` |
| `list_tasks` (markdown ready view) | columns Gain, Loss averted, Decision, Due (class), Cliff | Weight, "!", Crit, Urg | `handlers_task.rs:1605-1696` |
| `list_tasks` inputs | add `gain_gte`, `loss_averted_gte`, and `sort` ∈ {`default`, `gain`, `loss_averted`, `decision_value`, `gain_per_effort`, `loss_averted_per_effort`, `due`, `id`}; `intent` filters on the stored `intent` (Q10) | `focus_score_gte`, `weight_gte` (today a filter on `downstream_weight`: `handlers_task.rs:1334-1336`) | `handlers_task.rs:1146-1749`; `src/batch_ops/filters.rs:305-306` |
| `nested_tasks` | each `NestedTaskNode` carries `gain`, `loss_averted`, `decision_value`, `blocked`; parents show the share of children done, never a sum (R21); siblings ordered by `display_cmp` | `effective_intent`, `downstream_weight` | `src/graph_display.rs:739-765`, `:912-923`, `:1117-1160` |
| `task_search` (markdown) | Gain / Loss averted per hit; order stays semantic | "Metrics: scope / uncertainty / criticality" | `src/mcp_server/handlers_search.rs:177-190` |
| `search` | unchanged; its `confidence` nudge (`handlers_search.rs:290`, `:395`) is a search score, not ranking, and is out of scope | — | — |
| `get_dependency_tree` | each node adds `gain`, `loss_averted`; edges read from `needs` links (and mapped `depends_on`) | — | `handlers_task.rs:956-1011` |
| `get_task_children` | unchanged, except the intent column follows Q10 | — | `handlers_task.rs:1076-1088` |
| `task_summary` | counts only: ready, blocked, on the cliff lane, carrying worth, at zero, with `flow_status ≠ ok`; `by_intent` follows Q10 | none removed; never sums worth (R21) | `handlers_task.rs:2293-2338` |
| `top_n_by_metric` | `metric` enum adds `gain`, `loss_averted`, `decision_value` (`schemas.rs:117`); keeps `pagerank`, `betweenness`, `degree` | `downstream_weight`, `stakeholder_exposure` in its `NetworkMetrics` | `src/mcp_server/handlers_batch.rs:56-181`; `src/metrics.rs:11-18` |
| `get_network_metrics` | pagerank, betweenness, degrees | `downstream_weight`, `stakeholder_exposure` | `handlers_batch.rs:11-54` |
| `detect_weight_divergence` | edges whose `strength = quantum × probability ≥ 0.75` (today's threshold, `src/graph_store.rs:71`) with an idle source, across every label with a stated quantum; shows quantum, probability and strength separately | the single `stated_weight` number | `handlers_batch.rs:628-674`; `src/graph_store.rs:1543-1586` |
| `export_graph` (json) | per node: `flow` (no routes), `display` (as in `get_task`), `display_rank` (1..N under `display_cmp` over the exported set, replacing `queue_rank`); top-level `cliff: [ids]`; edges carry the fields of §3.1 | per node: `focus_score`, `downstream_weight`, `urgency`, `criticality`, `uncertainty`, `voi_value`, `affordable_loss_filtered`, `chain_slack`, `unlock_breadth`, `value_lineage`, `standing_weight`, `target_ancestors`, `cost_of_delay`, `severity_gate`, `queue_rank`; top-level `focus` | `src/graph_store.rs:21-37`, `:2189-2280` |
| `graph_stats` | adds priced and unpriced target counts, nodes carrying worth, loops, saturated loops, unsettled components | `affordable_loss_filtered_count` | `src/batch_ops/stats.rs:105`, `:296` |
| `create_task`, `update_task`, `create`, `batch_update` | accept `links`, `worth`, `deadline_class`; on write, an entry R5 would warn about is **rejected with an error** instead, so tools never write a value the parser would drop | the `contributes_to` / `depends_on` / `soft_depends_on` inputs stay, mapped by R7, until the migration spec retires them (E5) | `handlers_task.rs:135`, `:1751` |
| `pkb_trace`, `complete_task` / `release_task` neighbourhood | unchanged (no ranking fields today) | — | `handlers_search.rs:570-641`; `handlers_task_lifecycle.rs:287-391` |

**R23. No other ranking number.** No tool emits a ranking number other than `gain`, `loss_averted`, `decision_value`, `stake`, `loop_extra`, `display_rank` and the centrality diagnostics (`flow-rule.md` §12).

### 7.3. CLI and other outputs

| Surface | New | Removed | Today |
|---|---|---|---|
| `pkb tasks`, `pkb list` | default order `display_cmp`; `--sort gain\|loss\|decision\|due\|intent`; columns Gain / Loss averted / Due | `--sort weight`, the WEIGHT column, "!" | `src/cli.rs:1509-1588` |
| `pkb focus` (the default command) | the cliff lane, then the head of `display_cmp` over ready work and human gates | `focus_picks` | `src/cli.rs:1025`, `:1648-1666` |
| `pkb show` | the `get_task` flow block and explanation | Weight, exposure | `src/cli.rs:1712-1722` |
| `pkb metrics` | pagerank, betweenness, degrees | D.WT ranking, exposure | `src/cli.rs:1788-1905` |
| `format_task_line` | `g:` and `l:` figures | `wt:`, "!" | `src/cli.rs:4452-4493`; `src/graph_display.rs:813` |
| `pkb graph --format mcp-index` | `gain`, `loss_averted`, `decision_value`, `display_rank` | `downstream_weight`, `stakeholder_exposure`, `focus_score`; the intent-then-weight ready order | `src/task_index.rs:15-59`, `:279` |
| Excalidraw card size | the larger of `gain` and `\|loss_averted\|`, never their sum (E14) | `effective_intent`, `focus_score ≥ 1000` | `src/excalidraw/schema.rs:101`, `:120-127`; `layout.rs:706`; `merge.rs:277`, `:402` |

**R24. One field set everywhere.** The CLI and the MCP tools print the same figures from the same `FlowOutput`. Parity is tested as `tests/cli_default_ordering.rs` tests it today.

### 7.4. Sections of `ranking.md` superseded

| `ranking.md` section | Outcome |
|---|---|
| §1 Overview and sort tuple (`:25-62`) | superseded by §2 and §6 here |
| §1.1 Canonical ordering and determinism (`:64-69`) | **kept**; R17 extends it to `FlowOutput` |
| §2, §2.1–§2.9 tuple components (`:74-261`) | superseded; measure map in `flow-rule.md` §9 |
| §3 Caps and observed ranges (`:263-291`) | superseded; one unit, the worth of a Critical target (`flow-rule.md` §5.6) |
| §4 intro, §4.1 `downstream_weight`, §4.3 urgency, §4.3a chain slack, §4.4 `voi_value`, §4.5 uncertainty, §4.6 effective intent, §4.10 unlock breadth, §4.11 value lineage (`:293-433`, except §4.7–§4.8) | superseded |
| §4.2 `criticality` (`:315-323`) | superseded; it is computed from `downstream_weight` (E11) |
| §4.7 `scope` (`:397-401`) | **kept** as a display field |
| §4.8 pagerank, betweenness, degrees (`:403-408`) | **kept** as diagnostics |
| §5 Standing doctrine on graph metrics (`:435-447`) | **kept**, and extended: centrality never enters `flow.rs` or `display_cmp` |
| §6 Severity ladder (`:449-459`) | superseded; severity is not read (Q24) |
| §7 Verbal contribution scale, §7.1 multiplier and quantum (`:461-495`) | superseded by `flow-rule.md` §5.4–§5.5 and §3.1 here |
| §8 intro, §8.1–§8.3 predicates (`:497-517`) | kept in substance, rewritten by R20 |
| §8.4 `focus_cmp` (`:519-522`) | superseded by `display_cmp` (R19) |
| §8.5 `export_graph` ranking (`:524-532`) | superseded by §7.2 here |
| §9 Testing vs validation (`:534-547`) | superseded by §9 here; the "mechanism, not validation" caution is kept |
| §10 Consumers by measure (`:549-568`) | superseded by §7.2–§7.3 here |

When the build lands, `ranking.md` is rewritten to the kept sections plus a pointer here. `tests/schema_doc_integrity.rs:88-194` changes with it: `test_tool_descriptions_enumerate_all_eight_focus_score_components` and the canonical-sections list in `test_ranking_spec_exists_and_contains_canonical_sections` are rewritten for the new terms.

---

## 8. Acceptance criteria

Each criterion is something an observer can check against the built server. Every test is in the Rust suite, run by `cargo test` in `.github/workflows/pr-pipeline.yml:46`.

| # | Observable criterion | Traces to | Test |
|---|---|---|---|
| A1 | `get_task` on either of two open `needs` prerequisites of a priced node shows that node's gain; the node's own gain is unchanged with or without them; no other node's figures move | S10, I1, U17 | T-I1 |
| A2 | Replacing a node by two necessary parts changes no other node's `flow` | I2 | T-I2 |
| A3 | `stake[t]` is never above 1; a node serving two priced targets shows both in `stake` and their sum in `gain` | I3, U11 | T-I3 |
| A4 | A feeder into a reinforcing loop shows at least the gain it shows with the loop opened, and the rebuild completes | S8, S15, I4 | T-I4 |
| A5 | A blocked node shows its gain, and each open blocker shows at least as much | S4, I5, U3 | T-I5 |
| A6 | `flow` for every node is byte-identical under every `sort`, filter, view and format of `list_tasks`, and under every `CLIFF_BUFFER_DAYS` | S1, I6 | T-I6 |
| A7 | Every open node with no route to a priced target shows exactly 0 / 0 / 0 with `flow_status: ok` | S7, I7, U8, U9 | T-I7 |
| A8 | The last open `part_of` child of a priced node shows the node's full gain | S9, I8, U16 | T-I8 |
| A9 | Creating one target and one link changes `flow` only on the new link's upstream set | I9, U4, U10 | T-I9 |
| A10 | Work with a `settles` link into an open decision shows `decision_value = quantum × EVPI`; doubling every price doubles it; marking the decision done sets it to 0 | I10, U12 | T-I10 |
| A11 | `FlowInput` has no date-typed field and `src/flow.rs` names no date type or function; shifting every `due` by 400 days changes no `flow` | S5, I11 | T-I11 |
| A12 | `get_task` returns, for each target in `stake`, a strongest route whose strength is at most `stake[t]`, and reports `loop_extra` for any excess over the loop-free bound | I12, U18 | T-I12 |
| A13 | On the committed fixture, every open node's `gain` and `loss_averted` match `specs/pkb-flow-engine/expected-live-2026-10-05.json` within 1e-9 | I13 | T-parity |
| A14 | Restating a positive target as a negative one moves each protective task's figure from `gain` to `loss_averted` unchanged | S12, I14, U13 | T-I14 |
| A15 | A task serving one target and harming another equally shows two non-zero figures, distinct from a task linked to nothing | S16, I15 | T-I15 |
| A16 | A loop with a harms edge, including a pure negative loop at quantum 1, settles, with results independent of node ids | I16 | T-I16 |
| A17 | A hard-deadline node with worth 0 is first in `list_tasks` from `effort + CLIFF_BUFFER_DAYS` days before `due`; the same node classed fake or soft never enters the cliff lane | S13, I17, U14, U15 | T-I17 |
| A18 | No output of any tool or CLI command carries a field removed in §7.1–§7.3 | §7 | T-removed |
| A19 | A malformed `links` entry yields one `ParseWarning` and leaves the node's other entries in place | R5 | T-parse |
| A20 | A rejected or unsettled loop gives the nodes feeding it `flow_status ≠ ok` with null figures and the loop named; unrelated nodes keep their figures | R13, S15 | T-fail |
| A21 | `compute_flow` and `compute_decision_value` together take under 0.25 s on the fixture with every target priced, in a release build | R15, I13 | T-cost |
| A22 | Two rebuilds from identical input give byte-identical `flow` across every build entry point | R17 | T-determinism |
| A23 | No tool or CLI output contains a sum of `gain`, `loss_averted` or `decision_value` over two or more nodes | R21 | T-nosum |
| A24 | `pkb tasks`, `pkb focus` and `list_tasks` agree on order and figures | R19, R24 | T-cli-parity |

---

## 9. Test plan

There is one test per invariant, T-I1 to T-I17, and seven engine tests. Each invariant test builds the small graph its reference row uses in `specs/flow-rule/invariants.py`, so that the expected numbers are the ones `flow-rule.md` §10 already publishes.

| Test | Invariant / criterion | Construction | Expected (from `flow-rule.md` §10 unless stated) |
|---|---|---|---|
| T-I1 `flow_inv01_parallel_prerequisites_full_worth` | I1 / A1 | a node carrying 0.95 with two `needs` prerequisites (`brain_448bb804`) | each prerequisite gain 0.95; node 0.95 with and without them; largest change elsewhere 0.0 |
| T-I2 `flow_inv02_split_changes_nothing_unrelated` | I2 / A2 | split one prerequisite into two `needs` parts | parts 0.95 and 0.95; all other `flow` unchanged |
| T-I3 `flow_inv03_one_source_once_two_add` | I3 / A3 | node serving a 0.60 target directly at 1.0 and via its parent at 0.5, plus a 0.35 target | `stake` 1.0, not 1.5; gain 0.95 |
| T-I4 `flow_inv04_reinforcing_loop_bounded` | I4 / A4 | three-node loop from row 4, plus a feeder at 0.5 | feeder 0.2965 with the loop, 0.2801 with it opened |
| T-I5 `flow_inv05_blocked_passes_worth` | I5 / A5 | a blocked node carrying 0.95, with open blockers | node 0.95; each blocker 0.95 |
| T-I6 `flow_inv06_display_changes_no_number` | I6 / A6 | run `list_tasks` under every `sort` value and four `CLIFF_BUFFER_DAYS` | serialised `flow` map byte-identical across runs |
| T-I7 `flow_inv07_unlinked_is_default` | I7 / A7 | node with only wikilinks and `supersedes` to a priced target | 0 / 0 / 0, `flow_status: ok` |
| T-I8 `flow_inv08_last_step_full_worth` | I8 / A8 | a parent carrying 1.11 with four `part_of` children done and one open (`proj-f8b942d5`) | open child gain 1.11 |
| T-I9 `flow_inv09_opportunity_one_node` | I9 / A9 | add a 0.35 target and one `serves` link | changes only on the upstream set; the served node 1.11 → 1.46 |
| T-I10 `flow_inv10_decision_value` | I10 / A10 | decision with two `alternative` options (p 0.4 and 0.3, each 1.11) and one `settles` link | 0.1998; 0.3996 with prices doubled; 0.0 once decided |
| T-I11 `flow_inv11_no_dates_in_flow` | I11 / A11 | (a) a compile-time test that `FlowInput`'s fields are only ids, states, worths and edges; (b) a source scan of `src/flow.rs` for `chrono`, `NaiveDate`, `Utc`, `due`, `today`; (c) shift every `due` by 400 days | (a) compiles; (b) no match; (c) `flow` byte-identical |
| T-I12 `flow_inv12_routes_explain` | I12 / A12 | the fixture | for every (node, target) pair: strongest-route strength ≤ `stake`; any excess over the loop-free bound appears as `loop_extra` (`n_d663317dd7`: 0.9712 against 0.9449) |
| T-I13 → T-parity | I13 / A13 | load the fixture through the engine's own parser | every open node within 1e-9 of `expected-live-2026-10-05.json` (1,502 rows); 246 carrying worth |
| T-I14 `flow_inv14_negative_target_symmetry` | I14 / A14 | `targ_safety` at +0.35, then restated at −0.35 with protections as harms | 0.2625 / 0.2625 / 0.2822 move from `gain` to `loss_averted` unchanged |
| T-I15 `flow_inv15_gain_and_loss_not_netted` | I15 / A15 | task serving a 1.0 target at 0.6 and harming another 1.0 target at 0.6 | `(0.60, −0.60)`, distinct from `(0, 0)` |
| T-I16 `flow_inv16_harmful_loop_settles` | I16 / A16 | row-16 loop with a harms closing edge; a pure negative loop at quantum 1; ids permuted | 0.3878 / 0.3878 / 0.8163 and 0.2308 / 0.2308 / 0.7692; identical under permutation |
| T-I17 `display_inv17_cliff_lane` | I17 / A17 | hard node due D with worth 0, effort 1 day; fake and soft nodes; `today` stepped from D−40 to D | (this spec's construction) hard first in `list_tasks` from D−8 (1 + 7 days); fake and soft never on the cliff lane |
| T-removed `tool_outputs_carry_no_removed_field` | A18 | call every tool in §7.2 and every CLI command in §7.3 on a small graph | no key from the §7.1 removed list appears in any output |
| T-parse `links_entry_errors_are_isolated` | A19 | `links` with one valid entry, one unknown label, one quantum of 1.5, and one unknown word | 1 entry kept; 3 `ParseWarning`s, each naming its index and field; the unknown word read at the default |
| T-fail `flow_failure_is_null_and_local` | A20 | a full-strength `helps` loop of two open nodes fed by X; an unrelated priced chain | X: `flow_status: saturated_loop`, figures `null`, `loop` names both; unrelated chain unchanged |
| T-cost `flow_cost_budget` (release-only, `#[ignore]` in debug) | A21 | fixture with every target priced at 0.35 | under 0.25 s |
| T-determinism `flow_rebuild_determinism` | A22 | build through every entry point listed at `ranking.md:67` | byte-identical `flow` |
| T-nosum `no_output_sums_worth` | A23 | two nodes with gain 0.4 and 0.5 under one parent; every group view | no numeric field equal to 0.9 in any group-level output |
| T-cli-parity `cli_and_mcp_display_order_parity` | A24 | replaces `cli_focus_agrees_with_canonical_focus_order` (`tests/cli_default_ordering.rs`) | identical id order and figures |

**Tests retired with the code they test.** The build removes these tests and does not adapt them:

- in `src/graph_store.rs` `mod tests`: urgency, slack, effective intent, focus scoring, deadline multiplier, courtesy decay, stakeholder waiting, unlock breadth, value lineage, VoI, cone walks and `target_ancestors`;
- in `src/mcp_server/tests/task_list_tests.rs`: the `focus_score` and `signals` tests;
- in `src/mcp_server/tests/tag_date_filter_tests.rs`: `test_list_tasks_focus_score_gte_*`;
- `tests/export_graph_tool_test.rs::test_export_graph_json_emits_engine_queue_rank_and_cost_of_delay`;
- in `tests/simplify_node_types_test.rs`: the multiplier propagation tests.

The parse tests for severity and `goal_type` stay, since those fields stay stored (§3.4).

**Not validation.** As `ranking.md:545` says of today's tests, these check that the code does what the spec says. They do not show that the order is right for Nic. Calibration is the comparison in `flow-rule.md` §11.

---

## 10. Questions for Nic

### 10.1. Questions this spec adds (E1–E16)

Each comes with the proposal this spec builds against if it is not answered. None is decided.

| # | Question | Proposed |
|---|---|---|
| E1 | Store every link in one `links:` list (§3.1), or keep `depends_on` / `soft_depends_on` / `contributes_to` as separate keys with quantum, probability and effect added to each? | one list |
| E2 | Store Nic's words as written (`probable`) and map them on parse, or store numbers only? | words as written |
| E3 | When a loop is rejected or does not settle, should the nodes feeding it show `null` with the loop named, unlike the reference's 0.0? This resolves `flow-rule.md:634` against `:156` and `:158`. | `null`, rest ranked |
| E4 | When one edge is declared at both ends and the two disagree, which wins? | the `from` end, with a warning |
| E5 | Until migration ends, may tools keep accepting `contributes_to` / `depends_on` / `soft_depends_on` and map them by R7? | yes, until the migration spec retires them |
| E6 | Does the server need to store deadline extension history for the soft-to-hard rule, or does the skills spec keep it elsewhere? | the skills spec decides; the server stores only the class |
| E7 | Are the open constants (§4) compile-time, changed by pull request, or runtime configuration? | compile-time |
| E8 | How many routes per target should `get_task` return? | 5 |
| E9 | An open child of a done parent carries nothing through that parent (§5.1, `n_4b3ea8a012`). Is that right? | yes: the parent's worth is already realised |
| E10 | Full recompute on every background rebuild, with no incremental path? | full |
| E11 | `criticality` is defined from `downstream_weight` (`src/graph_store.rs:4357-4374`), which `flow-rule.md` §9 drops, yet §9 keeps criticality as a diagnostic. Drop it, or redefine it? | drop; keep pagerank and betweenness |
| E12 | `target_ancestors` is replaced by the keys of `stake`, which only list priced targets. Is a list of unpriced targets a node serves still wanted, for example to prompt pricing? | drop, and let the linter list unpriced targets |
| E13 | `ContributesTo.inherits_from` and `brier_history` are dormant edge fields. Keep them on `links`? | drop `inherits_from`; keep `brier_history` as an optional field the maths does not read |
| E14 | How big should an Excalidraw card be drawn? | by the larger of `gain` and `\|loss_averted\|`, never their sum |
| E15 | Today a task with `affordable_loss: false` is removed from ranking. Should the server keep that filter, or leave it to agents? | leave it to agents (`flow-rule.md` §9); the field stays stored |
| E16 | `status: blocked` counts as blocked in the mcp-index (`src/task_index.rs:260`) but not in `classify_tasks`. Should it count as blocked in R20? | no, to match `classify_tasks`; this replaces both |

### 10.2. Open questions in `flow-rule.md` §15 that change this build

| Question | What it changes here |
|---|---|
| Q1 | R6, `PART_OF_QUANTUM`, and whether grouping reads `parent` or `part_of` |
| Q2 | `DEFAULT_QUANTUM` |
| Q3 | step 2 of `display_cmp` (R19) |
| Q4 | the comparison scalar in `compute_decision_value` (R14) |
| Q5, Q21 | the label set the parser accepts (R5) |
| Q9, Q26 | `CLIFF_BUFFER_DAYS` and the cliff trigger |
| Q10 | whether `intent` stays as a filter and a column |
| Q12 | whether `gain_per_effort` is offered as a sort |
| Q18 | reading an unclassed `due` as `fake` (R10) |
| Q20 | the baseline harms semantics in §5.1 |
| Q23 | R8, and the cost budget R15 (scenario C) |
| Q31 | the ready predicate's type and status gates (R20) |

---

## 11. Corrections to the foundation spec found while writing this one

These are cited lines in `flow-rule.md` that no longer match `specs/flow-rule/flow.py` at main `2c3fef2`. The rule is unaffected.

| `flow-rule.md` says | Actual |
|---|---|
| `worth_all`, `flow.py:175` (line 162) | `flow.py:216` |
| `SaturatedLoop`, `flow.py:179` (line 156) | class at `flow.py:89`; detection at `flow.py:208` |
| `NoConvergence`, `flow.py:155` (line 158) | class at `flow.py:85`; raised at `flow.py:158` and `flow.py:180` |
| `_settle`, `flow.py:114` | correct |
| `E[max]`, `flow.py:206` (line 408) | `expected_best`, `flow.py:271` |

---

## 12. Files

| Path | What |
|---|---|
| `specs/pkb-flow-engine.md` | this spec |
| `specs/pkb-flow-engine/cost.py` | the cost figures in §5.4; runs the reference calculator unchanged |
| `specs/pkb-flow-engine/expected-live-2026-10-05.json` | oracle for T-parity: `gain` and `loss_averted` for every open node of the fixture, from `python3 specs/flow-rule/flow.py live specs/flow-rule/fixtures/live-2026-10-05.json --out …` |
