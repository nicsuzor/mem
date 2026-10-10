---
id: flow-migration
title: "Migrating the live graph to the flow rule"
type: spec
status: draft
created: 2026-10-06
task: epic_d1679d4b
epic: aops_ee1257cb
brief: spec_866ee53d
depends_on: flow-rule
tags:
  - ranking
  - flow-rule
  - migration
  - spec
---

# Migrating the live graph to the flow rule

**Home:** `specs/flow-migration.md` in the `nicsuzor/mem` repository, with its reference scripts in `specs/flow-migration/`. The migration tool it specifies belongs to the `pkb` binary in this repository. The data it changes lives in Nic's PKB repository (`nicsuzor/brain`, private).

**Status: draft for Nic's decision.** This spec says how the live graph moves from today's ranking (`ranking.md`) to the flow rule and edge model (`flow-rule.md`) without losing what the graph knows. It specifies and does not implement. Its numbers come from the reference dry run in [`flow-migration/dryrun.py`](flow-migration/dryrun.py). That script is a checking aid, not the migration tool.

Traceability tags:

- **S1–S16, I1–I17:** settled points and invariants in the brief (`spec_866ee53d`).
- **U1–U20:** user stories in `pkb-arch-framework`.
- **FR §n, FR-Qn:** sections and questions of [`flow-rule.md`](flow-rule.md).
- **T1–T6:** this task's own requirements (`epic_d1679d4b`, Acceptance), which the brief does not number: T1 every edge kind and field mapped, with live counts and defaults; T2 the pricing step; T3 what is lost and how it is reversed; T4 a dry run on the live graph; T5 the order of release; T6 in-flight tasks.
- **M1–M16:** this spec's acceptance criteria (section 12).
- **MQ1–MQ16:** this spec's questions for Nic (section 13).

---

## 0. Summary for Nic

**What happens.** The move has four steps, and each one can be undone:

1. **Add, don't replace.** The migration adds the new values (a `worth` on each target, a `quantum` on each link) next to the old fields. It deletes nothing. Today's ranking reads the graph exactly as before. The new ranking runs alongside in shadow.
2. **You price every target.** One sitting covers 43 targets: 26 typed `target` and 17 old `goal` nodes the engine already reads as targets. 7 carry a price today. An agent prepares the sheet and you pick a word for each.
3. **Switch the ranking over.** This waits until you have compared the two rankings on real data. Switching back is one setting.
4. **Clean up.** Only after you say so, a second change deletes the old fields. This is the only step that cannot be undone, except from git history.

**What it changes on today's graph** (live export, 6 October 2026, `dryrun.py`):

- **Writes.** 346 values on 255 of 3,701 nodes:
  - 7 target prices copied from `standing_weight`;
  - 184 "soft dependency" links given quantum 0.3;
  - 155 contribution links given the number for their old word.
  - In addition, the 24 `type: task` nodes with no status that the adapter reads as done receive a ledger row and a lint note (§7.1, :95) so nothing is lost silently.
  - Epic `hdr_prospective_enquiries` is converted into the first prototype hub (§9 R4b).
- **Untouched.** Nothing else: no other key, and none of the 5,375 wikilinks. That no page body changes is checked on the real files by M5; the fixture holds no bodies.
- **Undo works.** On today's data, undoing the migration restores the migrated fields of all 255 changed nodes exactly. The byte-level check on files is M3, for the build.
- **Rankings.** 52 of 1,489 open items carry worth under the new rule (with `part_of` at quantum 0, Q1 settled, and `status: retired` read as done, MQ10 settled).
  - Of today's top 50, 4 still carry worth. The other 46 carry no route to a priced target over edges of non-zero strength (`serves`, `needs`, `supports`).
  - Calibration cases reflect the settled Q1 rule (`part_of` quantum 0). The list's current first item (`proj-76fbc546`) moves to 2nd (gain 0.95 via direct `serves`). Nodes whose only connection to priced targets was through parent edges (`brain_448bb804`, `admin-3e02c20b`, `task_e79f1a57`, `personal_92d5909f`, `brain_7f772690`) carry 0.0 worth until direct contribution links are wired.

**What it costs you:**

- **About 36 target prices.** 19 targets reach no open work yet, so pricing them changes nothing until links are valued.
- **Six dates to class** as fake, soft or hard.
- **Six contribution links** whose old word was not on the scale ("medium" 3, "high" 2, blank 1). These are left unvalued for the first densify batch.
- **27 in-flight tasks to settle:** 2 to cancel, 9 to supersede, 5 to pause, 11 to keep (section 10).

**Decisions waiting on you:** section 13 questions. Note that C1 storage is confirmed additive in place (MQ4; engine owns schema, `links:` list as R8 option), MQ10 is settled (`retired` → done), and MQ14 is closed by the Q17 rule (price for worth in itself, never for what it feeds). The primary open questions are:

- **MQ1.** Do you switch over only once every target has a price (zero allowed)?
- **MQ2.** Are the 17 old `goal` nodes priced as targets, merged into targets, or retired?
- **MQ3.** Do you confirm the migrated values: soft links at 0.3, old words read as quantum with probability 1.00 (FR-Q22)?

---

## 1. Problem and target

**Problem.** `flow-rule.md` defines new inputs:

- a signed `worth` on targets;
- a `quantum`, `probability`, `effect` and `set_by` on every edge;
- a `deadline_class` on dated nodes.

The live graph carries none of them. It carries old inputs instead:

- `standing_weight` on 7 targets;
- a seven-word `stated_weight` on contribution links;
- bare id lists for `depends_on` and `soft_depends_on`;
- node fields the new maths never reads, such as `severity`, `stakeholder` and `intent`.

Four repositories read or write these fields:

- the engine (`mem`);
- the linter (`mem`, `src/lint.rs`);
- the skills (the ida plugin);
- the dashboard (`overwhelm-dashboard`).

**Target.** A path from today's graph to the new model with five properties:

- **It loses nothing silently.** Every edge kind and field is mapped, with live counts. Each loss is listed (section 7), including the 24 status-less tasks read as done (:205).
- **It puts Nic's prices first** (S14, U20).
- **It can be undone** at every step but the last, and the last needs Nic's say-so.
- **It is checked on live data** before anything changes (I13).
- **The parts release in an order** in which no component reads a field before it exists, or loses one it still needs (section 9).

---

## 2. Architecture and data flow

```text
                 ┌──────────── reversible ─────────────────────────────────────────────┐
 live graph ──►  snapshot ──► dry run ──► pricing ──► apply (additive) ──► shadow ──► cutover ──► window ──► cleanup
 (brain repo)    export +     report       (Nic)      one commit in       both        one engine  legacy     second commit
                 today's      (§8)         worth,     brain, ledger       rankings    setting     fields     deletes legacy
                 order                     deadline   (§5)                computed    flips       kept       fields (MQ7)
                                           classes                        (§9 R1)     (§9 R6)                ── one-way ──
```

- **The migration tool reads:**
  - the PKB files;
  - the engine's resolved edges;
  - today's `list_tasks` order.
- **It writes:**
  - frontmatter keys in the allowed set (§5.3), and nothing else;
  - a ledger file.
- **The engine reads the result** in both modes (§9). The legacy ranking ignores the added keys. The flow ignores the legacy ones (FR §2).

---

## 3. Inventory: every edge kind on the live graph

**Snapshot.** `export_graph` (format json, include_done) and `list_tasks` were taken on 2026-10-06 at about 11:55 UTC: 3,701 nodes and 7,258 edges. The committed fixture `flow-migration/fixtures/live-2026-10-06.json` keeps the structure and fields without titles, bodies, names or prose (section 14). Every count below is printed by `python3 specs/flow-migration/dryrun.py` (sections 1–2 of its output).

**How to read the columns.**

- **Stored entries** are the references written in frontmatter.
- **Resolved edges** are the edges the engine builds from them (`build_node_edges`, `src/graph_store.rs:2545-2691`).
- **The state columns** classify each resolved edge from the side of the work: the node that serves.

| Today | Stored entries | Resolved edges | Entries naming no node | Open → open | Open → done | From done | Touching cancelled | New label (FR §5.2) | Quantum written | Default it receives if not written |
|---|---|---|---|---|---|---|---|---|---|---|
| `parent` | 1,332 | 1,306 | 27 | 506 | 12 | 634 | 154 | `part_of` | none | `part_of` default, 0.0 (FR-Q1 settled) |
| `depends_on` | 202 | 196 | 6 | 49 | 1 | 92 | 54 | `needs` | none | `needs` default, 1.0 |
| `soft_depends_on` | 187 | 184 | 3 | 43 | 10 | 84 | 47 | `supports` | 0.3, `set_by: migrated` on all 184 | default quantum, 0.0 (FR-Q2) |
| `contributes_to` | 169 | 161 | 8 | 88 | 1 | 58 | 14 | `serves` | the old word's number, probability 1.00, `set_by: migrated` on 155 | default quantum, 0.0, on the 6 left unvalued |
| `supersedes` | 45 | 36 | 10 | 0 | 11 | 16 | 9 | not a flow edge (FR-Q30) | none | — |
| wikilink (`link`) | — | 5,375 | — | 1,247 | 668 | 3,027 | 433 | not a flow edge (FR-Q23) | none | — |
| `goals` (frontmatter list) | 24 | 0 (the engine builds no edge) | 0 | 19 | 0 | 5 | 0 | not migrated (MQ2) | none | — |
| `children`, `blocks`, `soft_blocks` stored in frontmatter | 1 (`children`) | 1 `parent` edge, `creative-bc3db522` → `ns-a27b77ce` | — | — | — | — | — | as `parent` | none | as `parent` |
| `closes` | 0 | 0 | — | — | — | — | — | not a flow edge | none | — |
| `similar_to` | not stored | computed on request | — | — | — | — | — | not a flow edge | none | — |

**Notes.**

- **Orientation.** `export_graph` writes `depends_on` and `soft_depends_on` as dependent → dependency. The flow reads them the other way, with the dependency serving the dependent (`flow-rule/flow.py:430-435`). The table counts them from the dependency's side.
- **Entries naming no node** are references that are not an exact node id: 27 + 6 + 3 + 8 + 10. The engine also resolves aliases (`resolve_ref`, `src/graph_store.rs:2575-2577`), and one `supersedes` entry resolves that way. That is why `supersedes` shows 36 resolved edges against 45 − 10 = 35. The other 53 build no edge today, and the migration does not change them (M13). The linter reports them (`epic_fc1de9ec`). The migration tool must resolve references the way the engine does, not by exact id (§5.2).
- **The stored `children` entry** points a node at a legacy `goal`, which the taxonomy says is never a parent (`specs/pkb-type-taxonomy.md:34`). The migration does not touch it. It is listed for the linter (MQ12).
- **The brief's counts** (parent 555, `contributes_to` 81, `soft_depends_on` 48, `depends_on` 47) and FR §8's (524, 83, 51, 64) used other filters on earlier days. The table above is the snapshot this spec stands on, and the dry run recounts at run time (M1).

**Flow edges after migration.** 419 edges have nonzero strength and no cancelled end (`flow-rule/flow.py:73-82`):

| Label | Flow edges |
|---|---|
| `needs` | 142 |
| `serves` | 140 |
| `supports` | 137 |

Edges from done nodes are included; the flow reads them as realised (FR §8). Under Q1 settled (`part_of` quantum 0.0), `parent` links migrate at default quantum 0.0 (`parent:` kept for grouping), so their 1,151 edges carry zero strength and are omitted from `flow_edges()`.

---

## 4. Inventory: every field the change touches

### 4.1. Fields on edges

| Field | Where | Live count | Fate at migration | Fate at cleanup (MQ7) |
|---|---|---|---|---|
| `stated_weight` | `contributes_to` | 169 entries: expected 76, probable 39, uncertain 20, certain 12, fifty-fifty 10, possible 3, improbable 1, impossible 1, unrecognised 7 (medium 3, high 2, marginal 1 on an entry naming no node), blank 1 | kept; its number is copied to `quantum` | deleted; the ledger keeps it |
| `justification` | `contributes_to` | 156 | kept unchanged; it becomes the edge's `justification` (FR §5.1) | kept |
| `multiplier` / `x` | `contributes_to` | 0 | if present, the quantum is `word × multiplier`, or the multiplier alone when the word is blank, capped at 1.0, as `flow.stated_weight` (`dryrun.py`, `apply`) | deleted (FR §14) |
| `anomaly_flag` | `contributes_to` | 18 | kept; no longer read (the divergence check moves to `quantum × probability`, FR §9) | deleted |
| `current_weight` | `contributes_to` | 0 (dormant, `src/graph.rs:260-262`) | none | deleted |
| `quantum`, `probability`, `set_by` | all edge kinds | 0 | written as in section 3 | kept |
| `effect` | all edge kinds | 0 | not written: every migrated edge helps (FR §5.1 default) | kept |

### 4.2. Fields on nodes

The counts are taken over all nodes, with open nodes in brackets. The new maths reads none of these fields except `worth` and node state (FR §2).

| Field | Live count | Fate at migration | Who reads it afterwards | Fate at cleanup |
|---|---|---|---|---|
| `standing_weight` | 7 (7) | copied to `worth` | nobody | deleted |
| `worth` | 0 | 7 written by copy; the rest by Nic in the pricing step (§6) | the flow | kept |
| `type: goal` | 17 nodes, read as `target` by the engine (`src/graph.rs:1014`, `:1369`) | unchanged | the flow, as targets | MQ2 |
| `severity` | 20 (19), 2 of them SEV0 | unchanged | the pricing sheet as a prompt (FR §5.6) | FR-Q24 |
| `goal_type` | 18 (17) | unchanged | the pricing sheet as a prompt | MQ7 |
| `consequence` | 136 (70) | unchanged | agents and Nic | kept (prose) |
| `intent` | 144 (71) | unchanged | display, if kept (FR-Q10) | FR-Q10 |
| `stakeholder` | 41 (15) | unchanged | agent triage (FR §9) | FR-Q25 |
| `waiting_since` | 25 (15) | unchanged | agent triage | FR-Q25 |
| `due` | 20 (6) | unchanged | the cliff lane (FR §7) | kept |
| `deadline_class` | 0 | not written; Nic classes the 6 open dates in the pricing sitting | the cliff lane | kept |
| `effort` | 124 (61) | unchanged | benefit per effort, in display (FR §7) | kept |
| `complexity` | 76 (47) | unchanged | nobody in ranking | kept |
| `confidence` | 177 (19) | unchanged | nobody: edge probability replaces it (FR §9) | MQ7 |
| `order` | 13 (12) | unchanged | display tie-break (FR §9) | kept |
| `assignee`, `classification` | 352 (127), 147 (38) | unchanged | agents | kept |
| `has_open_question` | 82 (22) | unchanged | nobody: value of information comes from `alternative` edges (FR §6) | MQ7 |
| node status | 12 values: ready 1,183, done 1,652, none 412, inbox 219, cancelled 147, queued 36, in_progress 22, paused 14, review 8, partial 5, blocked 2, retired 1 | unchanged; the flow reads done, retired (MQ10) or none as done, cancelled as gone, the rest as open (`flow-rule/flow.py:422-423`) | the flow | MQ10 settled (`retired` → done) |

Of the 412 nodes with status `none`, 24 are `type: task`. The flow adapter reads status-less nodes as done. These 24 tasks get a ledger row and a lint note (§7.1, :95).

### 4.3. Computed fields

These are computed, not stored, so nothing in the PKB files changes. The engine stops emitting them at cleanup, per FR §9 and §14. Counts are nodes carrying a nonzero value in the export:

| Field | Count | Fate |
|---|---|---|
| `focus_score` | 955 (602 open) | kept in shadow for comparison, then removed |
| `value_lineage` | 187 | kept in shadow for comparison, then removed |
| `downstream_weight` | 287 | kept in shadow for comparison, then removed |
| `unlock_breadth` | 24 | kept in shadow for comparison, then removed |
| `urgency` | 1,770 | kept in shadow for comparison, then removed |
| `voi_value` | 2 | kept in shadow for comparison, then removed |
| `stakeholder_exposure` | 58 | kept in shadow for comparison, then removed |
| `queue_rank`, `cost_of_delay`, `severity_gate` | on ranked tasks | kept in shadow for comparison, then removed |

---

## 5. The transform

### 5.1. Storage

**Confirmed: additive, in place (C1, MQ4).** Each of the four keys keeps its name and its file, and the key names the label:

| Key | Label |
|---|---|
| `parent` | `part_of` |
| `depends_on` | `needs` |
| `soft_depends_on` | `supports` |
| `contributes_to` | `serves` |

A value that differs from the label's default is stored on the entry itself. A list entry can therefore be a bare id, or a map `{to, quantum, probability, effect, set_by, justification}`.

**What this storage buys:**

- The migration touches only the 255 nodes that need a value.
- The legacy ranking keeps working once R1 ships, because the R1 engine reads the id from either form. **Today's parser does not:** `parse_string_array` keeps strings only (`src/graph.rs:1065`) and would silently drop a map entry. Apply therefore waits for R1 (§9).
- Undoing the migration is a mechanical inverse.

**Schema ownership and the R8 alternative.** The engine spec (`epic_2de1b579`) owns the stored schema for edges and target worth; this spec owns the migration transform. Additive in place is confirmed for R1–R7. A single unified `links:` list remains an R8 option for Nic at cleanup (§9). Whichever form is finalized at R8, the ledger, reversal, and acceptance criteria apply unchanged.

**Engine work under the proposal.** The engine must accept the map form inside `soft_depends_on` and `depends_on` lists. Today they parse as string arrays only (`src/graph.rs:1706-1712`). That is the one engine change the migration needs before it can be applied (§9, R1).

### 5.2. What is written

The transform is a pure function from one node's frontmatter to new frontmatter plus ledger rows. Its reference version is `apply` in `dryrun.py`.

| Input | Written | Live count |
|---|---|---|
| `standing_weight: w` and no `worth` | `worth: w` | 7 |
| `soft_depends_on` entry naming a node | `{to: id, quantum: 0.3, set_by: migrated}` | 184 |
| `contributes_to` entry naming a node, with a recognised word | `quantum: numeric_weight(word) × multiplier` (capped at 1.0), `probability: 1.0`, `set_by: migrated`, added to the entry | 155 |
| `contributes_to` entry with a blank or unrecognised word | nothing; listed for the first densify batch | 6 |
| `parent`, `depends_on` | nothing; label default applies (`needs` 1.0, `part_of` 0.0 per Q1 settled) | 0 |
| `type: task` with no `status` | nothing in file; ledger row `status: None → done` and lint note | 24 |
| entries naming no node | nothing | 0 |
| anything else | nothing | 0 |

**Quanta written to `contributes_to`:**

| Old word | Quantum | Entries |
|---|---|---|
| certain | 1.00 | 12 |
| probable | 0.85 | 38 |
| expected | 0.75 | 72 |
| fifty-fifty | 0.50 | 10 |
| uncertain or possible | 0.25 | 21 |
| improbable | 0.15 | 1 |
| impossible | 0.00 | 1 |

The word-to-number table is today's `numeric_weight()` (`src/graph.rs:292-318`), unchanged, as FR §8 proposes (FR-Q22, MQ3).

**Not written, by design:**

- `deadline_class` comes from Nic (§6).
- Decision relabelling (FR §6, FR-Q8) is a single hand edit to `brain_bf2be9d8`'s children after Nic answers FR-Q8, not part of the bulk transform (MQ9).
- Negative targets do not exist yet; Nic adds them in the pricing step if he wants them.

### 5.3. Allowed keys

The transform may add exactly these keys:

- `worth`;
- `quantum`;
- `probability`;
- `set_by`.

It may also change one shape: a `soft_depends_on` entry from bare id to map.

It never touches:

- a page body;
- a wikilink;
- any other key;
- any node it has no row for.

On the live graph the top-level keys changed are `contributes_to`, `soft_depends_on` and `worth` (`dryrun.py` output, section 4).

### 5.4. The ledger

Each run writes one ledger file in the PKB repository, under `.agents/migrations/flow-<date>.json`. It holds:

- **the snapshot:** the export's content hash and today's `list_tasks` order, with focus scores;
- **one row per write:** node id, file path, key path, value before, value after;
- **status-less task records:** the 24 `type: task` nodes with no status that the adapter reads as done (:205; `dryrun.py:69`), each given a ledger row and a lint note so they are auditable and not lost silently (§7.1, :95);
- **the run's report:** sections 3, 4 and 8.

The ledger is how the migration is reversed (§7.2). It is also how the dashboard shows "before" during the shadow period.

### 5.5. Tool contract

The `pkb` binary gains one subcommand. Its interface is fixed here; its build belongs to the engine work (`epic_2de1b579`).

| Command | Effect | Writes |
|---|---|---|
| `pkb migrate flow --dry-run` | Prints the report: §3, §4, writes by kind, the reversal self-check, the flow and calibration tables (§8) | nothing |
| `pkb migrate flow --apply` | Refuses unless the working tree is clean and the dry run is current. Writes §5.2, writes the ledger, and makes one commit with the trailer `Migration: flow-<date>` | PKB files, ledger |
| `pkb migrate flow --revert LEDGER` | Undoes each ledger row whose current value still equals its "after" value. Reports every row that has since been edited, and leaves those rows alone | PKB files |
| `pkb migrate flow --status` | Shows whether a migration is applied, how many rows have drifted since, and which targets still lack a price | nothing |
| `pkb migrate flow --cleanup LEDGER` | Refuses without `--confirm`. Deletes the legacy fields Nic chose in MQ7, in a separate commit | PKB files |

**Properties of the tool:**

- **Idempotent.** A second `--apply` finds nothing to write (M4).
- **Re-runs are scoped to the snapshot.** A re-run migrates only entries that existed at the first apply's snapshot commit, which the ledger records. A bare soft link written after R5 means "default quantum", and a re-run must not turn it into 0.3 (M4; reference: `dryrun.py` §4, "bare soft link added after the snapshot … rows written 0"). Links written between R4 and R5, before the skills switch, are listed by `--status` for the first densify batch, not re-valued silently (MQ16).

---

## 6. The pricing step

Nic prices every target (S14). Agents never propose a target's price (U20). They prepare the sheet and write what Nic says.

**Who is priced.** 43 nodes:

- 26 of `type: target`;
- 17 of legacy `type: goal`, which the engine reads as targets (`src/graph.rs:1369`).

7 carry `standing_weight` today and are copied over; Nic may change them. The brief counted 26 because it counted `type: target` only. The pricing sheet includes:
- **`garage-69a576ed`**: typed `target`, priced 0.15 by Ida (2026-10-09) pending Nic's feel. It sits inside the work tree under a done epic, against `specs/pkb-type-taxonomy.md`, and serves `targ_4e2cc92a`. The sheet asks whether to price it in itself or retype it as an epic and leave it unpriced to pass worth through.
- **The 17 legacy `type: goal` nodes (MQ2)**: goals carry no stakes under `specs/pkb-type-taxonomy.md`. The sheet asks whether Nic prices each as a target, merges it into an existing target, or retires it.

**The sheet.** One row per target, ordered by **reach**: the number of open nodes that carry worth once it is priced (`dryrun.py` output, section 9). Pricing the top rows first moves the most work. Each row shows:

- id and title;
- the current `standing_weight`, if any;
- `severity`, `goal_type` and `consequence`, as prompts only (FR §5.6);
- its reach;
- any outgoing links to other targets (the 19 target→target links identified in `dryrun.py` §9).

**The Q17 rule (closing MQ14).** Worth passes between priced targets and adds along the chain. A target is priced for what it is worth in itself, never for what it feeds; a target valued only for what it feeds is left unpriced and passes worth through (S18). The pricing sheet asks that question per target and per target→target link, settling MQ14.

Under Q1 settled (`part_of` quantum 0.0), reach reflects direct contribution paths (`serves`, `needs`, `supports`) rather than hierarchical parent grouping:

| Reach band | Targets | Examples |
|---|---|---|
| 10 or more | 3 | `targ-7d49f8a0` (25, priced 0.35), `ns-a27b77ce` (17, goal), `targ_4e2cc92a` (15, priced 0.60) |
| 2–9 | 9 | `goal-9ade5854` (8), `targ_safety` (7), `targ-1e7d4733` (4), `accountability-4b99fcaa` (2), `engagement-f7548c35` (2), `ns-135cf982` (2), `research-supervision-1b44af33` (2), `task_7327d7af` (2), `task_b3f01c80` (2) |
| 1 | 12 | `garage-69a576ed` (1), `qut-f71664e8` (1), `brain-b948a148` (1), `academicops-dfb31347` (1), `proj-home-c3997b42` (1), … |
| 0 | 19 | four `goal_type: committed` targets: `admin-73e215bc`, `brain-1b370df8`, `targ-8f16a7c2`, `targ_hdr_current_completion` |

A target with reach 0 is still priced. Its price moves nothing until the densify routine values a link to it (FR §8, skills spec `epic_80ce44ae`). The four committed targets with reach 0 are the clearest case: today severity lifts their work, and after cutover nothing does until a link is valued. The cliff lane still catches hard deadlines (FR §7).

**How Nic answers.** For each row, one of:

- a positive anchor word (Critical, High, Substantial, Moderate, Low);
- a negative anchor word (Catastrophic, Severe, Substantial, Moderate or Minor loss);
- "nothing" (worth 0);
- for targets serving other targets: whether it has worth in itself or only passes worth through (Q17).

The tool writes `worth` from the anchor table (FR §5.6, FR-Q7). If Nic says a target should go, that is recorded as a request and handled with MQ2 and §10, not written by the pricing tool. The sheet also asks two things:

- **The six open dates.** Is each one fake, soft or hard? An unclassed date is read as fake (FR-Q18):
  - `proj-76fbc546`;
  - `task_1760e611`;
  - `task_d5f610e6`;
  - `brain_61467de3`;
  - two further dated tasks, hashed in the fixture because their ids reveal a personal event. The pricing sheet is private and shows their real ids and titles.
- **Outcomes to avoid.** Is there one Nic wants to price as a negative target, for example a restated `targ_safety` (FR §4.4)?

**Size.** Pricing is one sitting of at most 43 answers plus 6 date classes. By FR §8's count, it is the first of about four batches of fifty.

**Gate.** As proposed, cutover (§9, R6) refuses while any target lacks a `worth` (zero is allowed) and names those that lack one (MQ1). `pkb migrate flow --status` shows the count.

**What pricing does to the numbers** (illustration only; **assumed** prices, not Nic's). With every unpriced target and goal set to Moderate (0.15):

- 103 open nodes carry worth instead of 52 (`dryrun.py` output, section 8);
- `proj-76fbc546` carries gain 1.0625;
- tasks with no direct contribution links remain at 0.0 until links are valued.

These figures show the scale of the step. They are not a proposal.

---

## 7. What is lost, and how the migration is reversed

### 7.1. Losses, by stage

| Stage | What is lost | Size on today's graph | Recoverable? |
|---|---|---|---|
| Apply | nothing: the change is additive. Of the 255 nodes it changes, all 255 keep the same legacy reading once the R1 engine reads map entries (`dryrun.py` §4); before R1 the legacy parser would drop soft links, so apply waits for R1 | 255 of 255 changed nodes | yes, `--revert` |
| Apply | **meaning, not data**: an old word mixed amount and likelihood; it becomes a quantum with probability 1.00 | 155 links | the word is kept until cleanup; densify re-asks it (FR §5.4) |
| Apply | **meaning, not data**: a soft link that recorded context, not contribution, now moves worth at 0.3 | 184 links, 43 between open nodes | densify can set any of them to 0 |
| Shadow | nothing | — | — |
| Cutover | today's order | 529 ranked tasks; the order is saved in the ledger | yes, one setting |
| Cutover | the pull of severity, a named person waiting, dates, priority band, age, blast radius, unlock breadth and value-of-information gates (FR §9, §14) | 46 of today's top 50 fall to zero under Q1 settled (`part_of` at 0.0); 4 carry worth (`dryrun.py` §7) | yes, one setting; the hard-deadline cliff and Nic's prices bring back what matters (FR §7) |
| Cutover | worth for 4 committed targets with no valued link | reach 0 (§6) | by densify, not by reversal |
| Cutover / Adapter | 24 tasks with no status: the flow adapter reads status-less nodes as done (:205; `dryrun.py:69`), treating them as completed without explicit status. Each gets a ledger row and a lint note so nothing is lost silently (:95) | 24 tasks: `n_07d115101c`, `n_0b06664fc4`, `n_11ebbcf82d`, `n_3be6d9cce6`, `n_42995d77f7`, `n_485b5d97c3`, `n_4bce0b411b`, `n_4d0abba538`, `n_51f53f6e11`, `n_6543db808e`, `n_7d5a883f2d`, `n_85426e629a`, `n_93a403f3e1`, `n_9c4482b6e8`, `n_ac2aa14b93`, `n_ac43e9ee9b`, `n_b57c314c23`, `n_b65ef54fad`, `n_c609df7d5a`, `n_cbbf29e240`, `n_da89aafae0`, `n_dac7ad7c10`, `n_e466433623`, `n_f0c34f69fb` | in files; linter flags them |
| Cleanup | the legacy fields Nic chose (MQ7) | up to `standing_weight` 7, `stated_weight` 169, `anomaly_flag` 18, plus any of the node fields in §4.2 | only from git history and the ledger |
| Never migrated | references naming no node; `goals` lists; wikilinks as edges | 53 unresolvable entries; 24 `goals` entries; 5,375 wikilinks | they stay in the files as they are; nothing is deleted |

### 7.2. Reversal

There are three levels. Each is checkable (M3, M10, M14).

1. **Cutover back.** The engine setting `ranking: legacy | flow` returns to `legacy`. No file changes. The `list_tasks` order matches what the legacy engine gives from the same files (M10).
2. **Data back.** `pkb migrate flow --revert LEDGER` restores every row still holding its migrated value. Nodes edited after the migration are reported, not overwritten. If no later commit touched the migrated lines, `git revert` of the migration commit does the same. On the live graph's fixture, the ledger-driven revert restores all 255 changed nodes exactly. It also keeps a `worth` that was already set, and reports a link edited after apply as drift and leaves it alone (`dryrun.py` §4). The fixture holds the fields the migration reads, not whole files, so the byte-level check is M3.
3. **After cleanup.** Reversal is no longer a tool operation. The ledger holds every deleted value, and git holds every file. Cleanup therefore needs Nic's explicit approval (`--confirm`, MQ7). It runs only after the reversal window he sets (MQ6).

---

## 8. Dry run on the live graph

**How to reproduce:** `python3 specs/flow-migration/dryrun.py`. It runs in under a second.

The dry run applies §5 to every node of the fixture, reads the migrated graph as the engine would (`read_migrated`), and runs the flow-rule reference calculator. As a cross-check it also runs flow-rule's own adapter (`flow.from_export`) on the unmigrated fixture. The two agree on all 1,489 open nodes. So the stored form proposed in §5, read back, gives the same numbers as FR §8's adapter. No files are written; the check on real files is M7.

**Headline:**

- 1,489 open nodes (with `retired` read as done, MQ10 settled);
- 52 carry worth, from 7 priced targets with `part_of` at quantum 0 (Q1 settled);
- loops cover 2 nodes, and none is saturated (FR §3.2).

**Before and after on the calibration cases** (`kb_weighting_user_stories_test_cases`):

- **"Today"** is the `list_tasks` order at the snapshot (529 tasks).
- **"After"** orders the same tasks by gain plus loss averted, with ties broken by today's order. It is a comparison device only, as in FR §11; the ordering is FR-Q3.

| Case | Node | Status | Today rank | focus_score | After: gain | After: loss averted | After rank | Why it moves |
|---|---|---|---|---|---|---|---|---|
| 1 triage | `admin_1fece2e0` | cancelled | — | — | — | — | — | Case no longer live: cancelled since the case was written |
| 2 menu | `brain_448bb804` | ready | 256 | 51 | 0 | 0 | 263 | Under Q1 settled (`part_of` quantum 0.0), worth does not flow over parent links; requires direct contribution link |
| 2 menu | `academic-b738bdc7` | ready | 307 | 200 | 0 | 0 | 308 | No route to a priced target; 0.15 if its targets are priced Moderate (§6, assumed) |
| 2, 3, 6 | `task_model_ten_tasks_properly` | partial | 200 | 39 | 0 | 0 | 223 | No route to a priced target; proposed for cancellation (§10) |
| 2, 5 | `admin-59965524` | done | — | — | — | — | — | Case no longer live |
| 3 untangle | `mem_722b54d7` | queued | 413 | 4 | 0 | 0 | 413 | No route to a priced target; superseded by the flow rule itself (§10) |
| 3 untangle | the premise check the case names (its id carries personal names, so it is hashed in the fixture and not printed) | done | — | — | — | — | — | Case no longer live: the blocker it names is done |
| 3 untangle | `task-11182949` | inbox | not ranked | 185 | 0 | 0 | — | Not ready today; no route to a priced target |
| 4 opportunity | `admin-3e02c20b` | ready | 289 | 2 | 0 | 0 | 291 | Route to decision option was via `parent`; with `part_of` at 0.0, worth requires direct `serves` / `alternative` |
| 4 opportunity | `trustcon_1c23b18d` | ready | 237 | 10 | 0 | 0 | 248 | No route to a priced target |
| 5 frontier | `aops_polecat_mcp_server_build` | done | — | — | — | — | — | Case no longer live |
| 6 fun | `proj-76fbc546` | ready | 1 | 14,864 | 0.95 | 0 | 2 | Serves two priced targets directly via `serves`: 0.60 (`targ_4e2cc92a`) + 0.35 (`targ-7d49f8a0`) |
| top | `task_e79f1a57`, `personal_92d5909f`, `brain_7f772690` | ready, ready, inbox | 2, 3, 4 | ~11,100 | 0 each | 0 | 44, 45, 46 | Linked to decision `brain_bf2be9d8` via `parent`; with `part_of` at 0.0, carries 0 until relabelled `alternative` |
| top | `task_d5f610e6` | inbox | 5 | 10,956 | 0 | 0 | 47 | Named person times date; no priced target. If its date is classed hard, it heads the cliff lane (FR §7, I17) |
| top | `admin_3fcff4d3` | ready | 10 | 9,336 | 0 | 0 | 52 | Value lineage was parent-propagated; a name alone adds nothing (U7) |

**Movement at the top:**

- **Today's top ten, after:** ranks 2, 44, 45, 46, 47, 48, 49, 50, 51 and 52 (`dryrun.py` §7).
- **The new top ten, today:** ranks 192 (`brain_bf2be9d8`), 1 (`proj-76fbc546`), 280 (`n_babb53b707`), 187 (`n_4bc0b6c27a`), 198 (`n_2b7d0dfb98`), 203 (`n_49a3fc84c1`), 281 (`n_1b572ba86e`), 239 (`n_d7f3292689`), 278 (`n_b4d537b6bb`), and 38 (`n_f4de0462b4`).

Under Q1 settled (`part_of` quantum 0.0), worth does not flow over parent edges. The decision `brain_bf2be9d8` carries 1.11 worth directly from its outgoing links to `task_b3f01c80` and `targ_4e2cc92a`. However, its options (`task_e79f1a57`, `brain_7f772690`, `task_1760e611`, `proj-f8b942d5`, and sub-nodes `personal_92d5909f`, `admin-3e02c20b`) were grouped as `parent:` children, so they do not inherit worth until they are relabelled `alternative` (FR-Q8, MQ9).

**What the dry run shows:**

- **Direct contribution drives ranking.** With `part_of` at quantum 0, only tasks serving priced targets along `serves`, `needs`, and `supports` edges carry worth. `proj-76fbc546` (case 6) ranks 2nd with gain 0.95. Tasks previously inflated by hierarchical parent links or names alone drop down the ordering.
- **Pricing comes first.** Tasks without links to priced targets sit at zero. With every target priced (§6, assumed), 103 open nodes carry worth instead of 52. The order of release (§9) therefore puts pricing before cutover.
- **Four of the six cases describe a state that is no longer true:** cases 1 and 5, and the named blockers of cases 2 and 3. A fresh calibration set before cutover is MQ11.

---

## 9. Order of release

**Principles:**

- No component reads a field before it is written.
- No component loses a field it still reads.
- Every step before R8 can be undone.

**Owners:**

- engine: `epic_2de1b579`, in `nicsuzor/mem`;
- linter: `epic_fc1de9ec`;
- skills: `epic_80ce44ae`, in the ida plugin;
- dashboard: `epic_adf10cd3`, in `nicsuzor/overwhelm-dashboard`;
- data: this spec, in `nicsuzor/brain`.

| Step | Component | What ships | Precondition (checkable) | Undo |
|---|---|---|---|---|
| R0 | — | Nic approves the six specs. He answers the questions that block: FR-Q1, FR-Q2, FR-Q22 / MQ3, MQ1, MQ2, MQ4 | assessment `epic_3c8110d9` done | — |
| R1 | engine | **Dual read, shadow flow.** The engine parses the new keys and the map form of list entries (§5.1). It computes the flow beside the legacy ranking, and emits `gain`, `loss_averted`, `stake`, `decision_value` and `loop_extra` (FR §12) next to the legacy fields. The legacy ranking still sorts. The setting `ranking: legacy` is the default. The `pkb migrate flow` tool ships | R0 | release revert; no data written |
| R2 | linter | Ships with graph-lint §5 severities (errors, warnings, style), legacy checks unchanged: unpriced targets, unvalued edges, invalid labels and signs, saturated loops, unclassed dates, references naming no node | R1 | config |
| R3 | data | **Pricing sitting** (§6). Writes `worth` and `deadline_class` only | R1 (the engine reads `worth`) and R2 (the linter lists what is missing) | `--revert` on its own ledger |
| R4 | data | **Apply.** `--dry-run`, then Nic reviews, then `--apply`, as one pull request in `nicsuzor/brain` that Nic merges (MQ8) | R3; the dry run regenerated on the day; linter clean of errors | `--revert` |
| R4b | data | **Prototype hub conversion.** Convert epic `hdr_prospective_enquiries` into the first prototype hub: set `type: prototype`, `edge_template: {serves: {to: hdr_prospective_enquiries, quantum: 1.0}}` for instances; prototype carries `serves` to `research-supervision-1b44af33` at its existing `probable` link (quantum 0.85); no prototype node exists on the graph today (S20). Instances inherit class-level worth through this unpriced hub | R4 | revert frontmatter of `hdr_prospective_enquiries` |
| R5 | skills | `/q`, `/decompose`, densify and the email placement write `quantum`, `probability` and `set_by` on new links, and `worth` on new targets. They stop writing `stated_weight`. Today the plugin's `skills/q/SKILL.md:19` and `skills/decompose/SKILL.md:25,30` name `depends_on` and `soft_depends_on` only. `--status` lists links added between R4 and R5 for the first densify batch (§5.5) | R4 | plugin release revert |
| R5 | dashboard | Shows gain and loss averted side by side, and the route explanation, from the shadow fields. Legacy views stay, so Nic can compare | R1 (fields exist); R4 (values are real) | release revert |
| R6 | engine | **Cutover:** `ranking: flow` | MQ1 gate (every target priced); R5 shipped; Nic has compared both rankings on the dashboard | `ranking: legacy` |
| R7 | — | **Reversal window.** Both rankings are still computed, and legacy fields are still on disk | — | — |
| R8 | data, engine, linter, dashboard | **Cleanup** after Nic's `--confirm` (MQ6, MQ7): legacy fields are deleted. The engine stops computing the legacy ranking (FR §14). The linter switches to **error** for legacy fields (referring to lint §5.8 promoting `flow-legacy-field` to error). The dashboard removes legacy views. Single `links:` list is an option for Nic | Nic's explicit approval | git history and ledger only |

**Why this order:**

- **Engine before data.** The engine must read the map form before any file uses it. Otherwise the legacy parser drops the soft links (`src/graph.rs:1706-1712` parses string arrays).
- **Pricing before cutover.** With 7 of 43 targets priced, the new order is mostly a function of what is unpriced (§8).
- **Prototype hub before cutover.** Converting `hdr_prospective_enquiries` into the first prototype hub (R4b) establishes class-level worth for incoming enquiries before cutover (S20).
- **Skills soon after apply.** Links created between apply and the skills switch are listed for densify (§5.5), not lost and not silently re-valued.
- **Dashboard before cutover.** Nic's comparison of the two rankings happens on the dashboard, and the dashboard reads both sets of fields.
- **Cleanup last and separately.** It is the only step that cannot be undone.

---

## 10. In-flight tasks to cancel, supersede or pause

These come from a bounded read-only survey of the PKB on 2026-10-06:

- 51 semantic searches;
- 32 title searches over non-terminal tasks;
- the open children of the ranking-related epics.

The survey excluded the siblings under `aops_ee1257cb`.

The dispositions are **proposals**. This task's authority covers itself only, so no task is changed until Nic approves the list (MQ5). Approved changes are applied at R0:

- cancelled tasks get `status: cancelled` with a reason;
- superseded tasks get a `supersedes` edge from the named sibling spec;
- paused tasks get `status: paused`, and are reconsidered at R6.

| Task | Status | Work | Proposed | Reason |
|---|---|---|---|---|
| `task_model_ten_tasks_properly` | partial | elicit weights for ten tasks across every current channel | cancel | its channels (severity → urgency, value of information, stakeholder waiting, deadline multiplier) are removed. Brief S12 and S16 answer its questions on bad and good consequences. Cancelling it unblocks `mem_722b54d7` |
| `brain_743e8381` | inbox | repair escapes in the ranking-maths section of the taxonomy note | cancel | that section is rewritten. Any writer bug belongs in an issue |
| `mem_722b54d7` | queued | carry value lineage across `depends_on` to unblockers | supersede by engine `epic_2de1b579` | the flow does it over `needs` (S4, I5) |
| `overwhelm_dashboard_e0191453` | inbox | certainty discount on `contributes_to` | supersede by engine `epic_2de1b579` | an edge's strength is quantum × probability (S2) |
| `task_116d1c7d` | inbox | rewrite one target's weight justification | supersede by this spec | `standing_weight` becomes `worth`, re-priced in §6 |
| `brain_repair_extend_20260924` | ready | bulk repair of `contributes_to` with the seven-word scale | supersede by this spec | writing at scale in a vocabulary R5 retires. Its targets go into the densify queue |
| `task_1c89053b` | paused | show all weight sources in task detail | supersede by dashboard `epic_adf10cd3` | most of those sources are removed |
| `od_focus_signal_headline_stale` | queued | fix the focus-score headline | supersede by dashboard `epic_adf10cd3` | `focus_score` and `queue_rank` are removed |
| `od_engine_fields_for_dashboard_derivations` | queued | move client-side derivations into `export_graph` | supersede by dashboard `epic_adf10cd3` (fields from `epic_2de1b579`) | the severity gate and age staleness it relies on are removed |
| `od_gravity_view_design` | partial | force view sized by value received | supersede by dashboard `epic_adf10cd3` | `value_lineage`, `standing_weight` and `stated_weight` all change |
| `od_force_view_unblocker_emphasis` | ready | emphasise unblockers | supersede by dashboard `epic_adf10cd3` | flow to unblockers (S4) defines the measure |
| `overwhelm_dashboard_fce30143` | ready | deploy the cost-of-delay engine and dashboard | pause | it matters only while the legacy engine is live |
| `od_effective_intent_border_size_conflict` | ready | intent border against propagated band | pause; cancel at R8 | `effective_intent` is removed |
| `od_daily_treemap_strategic_review` | ready | daily review against the ranking doctrine | pause; the rubric moves to skills `epic_80ce44ae` | its rubric is the focus tuple |
| `ida_instr_daily_treemap_strategic_review` | ready | the standing ledger for the same review | pause with its twin | as above |
| `mem_ac9a47ac` | ready | QA of restored weight notes | pause; likely cancel | the notes describe the engine being replaced |
| `task_1760e611` | inbox | a direction decision, then pricing the academic targets | keep; re-scope its pricing goal into §6 | it supplies S14 |
| `overwhelm_dashboard_02f5fa9b` | ready | weighting discussion | keep; its measure table is superseded by FR §9 | — |
| `task_48255cdd` | inbox | force-view grouping | keep | affected only if the parent edge is removed (FR-Q1) |
| `task_a8fa1f4b`, `od_container_tiles_uncoloured_legend_counts_them`, `mem_bffac995`, `overwhelm-dashboard-e993b43b`, `task-overwhelm-metro-design` | various | display and schema text | keep | not ranking maths |
| `mem_c94f7303` | ready | move PKB rules into mem specs | keep; leave out ranking rules | the engine spec rewrites them |
| `aops_288dcb95` | inbox | consolidate the strategic-conversation persona | keep; overlaps skills `epic_80ce44ae` on value of information | — |
| `aops_6d5a74a7` | ready | track the redesign specs | keep | — |

**Survey gap:** the title searches did not include subtasks. A matching subtask that semantic search also missed could be uncounted.

---

## 11. Behaviour removed

**At apply:** none. The change is additive (§7.1).

**At cutover** (R6), as the default ranking, though still computed until R8:

- the legacy sort tuple and every term FR §14 lists;
- the `list_tasks` order derived from them.

**At cleanup** (R8):

- **Frontmatter keys:**
  - `standing_weight`;
  - `stated_weight`;
  - `multiplier` / `x`;
  - `anomaly_flag`;
  - `current_weight`;
  - whichever node fields Nic retires in MQ7.
- **Engine:**
  - the legacy ranking stages (`ranking.md:40-57`);
  - the `ranking: legacy` setting;
  - the legacy fields in `export_graph`.
- **Linter:** the parse warning for unrecognised contribution words (`src/graph.rs:1605-1623`), replaced by the new edge checks.
- **Skills:** writing `stated_weight`.

---

## 12. Acceptance criteria and tests

Each criterion is something an observer can check. The test beside it is the one the build must carry. Criteria tagged only T1–T6 trace to this task's own requirements, which the brief does not number (its settled points say nothing about reversal or release order); whether that meets the common expectation of tracing to the brief is for the assessment (`epic_3c8110d9`). Where a reference version exists, the table names it.

| # | Observable criterion | Traces to | Test |
|---|---|---|---|
| M1 | `--dry-run` prints, for each edge kind in §3, stored entries, resolved edges and entries naming no node. The resolved counts equal `export_graph`'s count of edges of that type, taken at the same moment | T1, brief §B (edge kinds), I13 | compare the dry-run table with a direct count over `export_graph` edges; reference: `dryrun.py` §1 |
| M2 | After `--apply`, the legacy ranking's `list_tasks` order is identical to the order before apply | T3, S1 | with the R1 engine and `ranking: legacy`: list before, apply, list after, diff empty. Reference (weaker: it shows only that no key outside the allowed set moved): `dryrun.py` §4 "R1 legacy view is unchanged … 255 of 255" |
| M3 | `--revert` after `--apply` leaves the PKB byte-identical to the snapshot commit | T3 | `git diff <snapshot>` empty after revert; reference: `dryrun.py` §4 "restored exactly … 255 of 255" |
| M4 | A second `--apply` writes nothing | T3 | run apply twice: the second commit is empty; add a bare soft link after apply and re-run: it is not written. Reference: `dryrun.py` §4 |
| M5 | The migration commit changes only frontmatter. Its added keys are a subset of §5.3, and no body line changes | T1, T3 | parse the diff; every hunk is inside frontmatter and every added key is allowed |
| M6 | Every changed value has exactly one ledger row, and every ledger row matches a change | T3 | join the ledger with the diff; no unmatched rows on either side; reference: 346 rows on the fixture |
| M7 | After apply, the engine's shadow `gain` and `loss_averted` equal the dry run's for every open node | I13, FR §12 | compare the shadow export with the dry-run table; reference: `dryrun.py` §5 "identical to flow-rule's own adapter: 1489 of 1489" |
| M8 | Each pricing answer writes `worth` per FR §5.6, and only that, plus `deadline_class` for the dates asked | T2, S14, U20 | price one target and class one date; the diff shows those two keys only |
| M9 | Under MQ1, cutover refuses while a target lacks `worth` and names it | T2, S14, MQ1 | remove one `worth`; the cutover command exits non-zero with that id |
| M10 | Setting `ranking: legacy` after cutover restores the legacy order exactly | T3, T5 | list before cutover, cut over, set legacy, list again, diff empty |
| M11 | The dry-run report regenerates the calibration table in §8, and every row whose rank moves by more than 50 carries an explanation | T4, I13, U3, U4, U17 | the report's calibration section is non-empty, and its explanation column has no blanks for such rows |
| M12 | Each release step's precondition in §9 is a check the tool or CI runs before the step proceeds | T5 | for R4 and R6, run the step with its precondition unmet: it refuses |
| M13 | References that name no node are unchanged by the migration and listed by the linter | T1 | count before and after apply: equal; the linter lists the same ids |
| M14 | `--cleanup` refuses without `--confirm` and without a ledger. With both, it deletes only the keys chosen in MQ7 | T3, MQ7 | run without the flag: it refuses; run with it: the diff deletes only the listed keys |
| M15 | `--status` reports drift: rows edited since apply. `--revert` skips them and names them | T3 | apply, edit one migrated value, revert: that row is reported and untouched; the others are reverted |
| M16 | Each in-flight task Nic approves in §10 carries its disposition: status, or a `supersedes` edge from the named spec | T6, MQ5 | `get_task` on each id shows the change |

---

## 13. Questions for Nic

The questions are ordered by consequence. Where a question repeats one in `flow-rule.md`, it says so. Those must be answered once, for both specs.

1. **MQ1. Pricing gate.** Should cutover wait until every target has a `worth`, with zero allowed? Proposed: yes. Otherwise work for unpriced targets falls to zero at cutover (§8).
2. **MQ2. The 17 legacy `goal` nodes.** The engine already reads them as targets. Should they be priced, merged into existing targets, or retired? And their 24 `goals:` entries, which today build no edge: should they become `serves` links at the default quantum, so densify can value them, or stay inert? Proposed: inert until the goals themselves are decided.
3. **MQ3 (= FR-Q22). Migrated values.** Do you confirm soft links at quantum 0.3, and the old words read as quantum with probability 1.00?
4. **MQ4. Storage, and who decides it.** Settled (C1): Additive in place is confirmed for R1–R7 (§5.1); a unified `links:` list remains an R8 option. The engine spec (`epic_2de1b579`) owns the stored schema; this spec owns the transform.
5. **MQ5. In-flight tasks.** Do you approve the dispositions in §10?
6. **MQ6. Reversal window.** How long do both rankings run after cutover before cleanup may be proposed? Proposed: no fixed length; cleanup only on your word.
7. **MQ7. What cleanup deletes.** Which legacy fields go at R8? Candidates:
   - `standing_weight`, `stated_weight`, `anomaly_flag`, `multiplier`;
   - `severity`, `goal_type` and `confidence` (see FR-Q24);
   - `stakeholder` and `waiting_since` (see FR-Q25);
   - `intent` (see FR-Q10);
   - `has_open_question`.
8. **MQ8. Who applies.** Does an agent open the migration as a pull request in `nicsuzor/brain` for you to merge, as proposed, or do you run it yourself?
9. **MQ9 (with FR-Q8). The open career decision.** Should `brain_bf2be9d8`'s options be relabelled `alternative` before cutover? Until then each option carries the decision's full worth, and the decision's subtree fills seven of the new top ten (§8).
10. **MQ10. `retired` status.** Settled: `status: retired` maps to `done` in the flow adapter (engine E16).
11. **MQ11. Calibration set.** Four of the six calibration cases describe a state that is no longer true (§8). Should a fresh set be drawn before cutover, so the comparison at R6 tests live cases?
12. **MQ12. The one stored `children` entry** points at a legacy goal. Should the linter fix it unasked, or report it?
13. **MQ13. Unrecognised words.** Six contribution links carry "medium" (3), "high" (2) or a blank (1). They are left unvalued for the first densify batch, as proposed. Or should you value them in the pricing sitting?
14. **MQ14 (= FR-Q17). Targets serving targets.** Closed by the Q17 rule (S18): A target is priced for its worth in itself, never for what it feeds; a target valued only for what it feeds is left unpriced and passes worth through. The pricing sheet (§6) asks that question per target and per target→target link.
15. **MQ15. Snapshot of today's order.** The ledger keeps today's `list_tasks` order for comparison. Should it also be kept after cleanup, as a record of what the old engine said?
16. **MQ16. Links written between apply and the skills switch.** Should they be listed for densify (proposed), or should the skills switch ship in the same release as apply so that no such links exist?

Questions from `flow-rule.md` that block a release step here:

- **Settled:** FR-Q1 (`part_of` default quantum 0.0), FR-Q2 (default quantum 0.0), FR-Q17 (targets priced for worth in itself).
- **Before R6:** FR-Q3 (ordering), FR-Q8 (decision relabelling) and FR-Q18 (unclassed dates).

---

## 14. Files

| Path | What |
|---|---|
| `specs/flow-migration.md` | this spec |
| `specs/flow-migration/dryrun.py` | reference dry run: inventory, transform, reversal check, flow, calibration, pricing sheet |
| `specs/flow-migration/extract_fixture.py` | builds the fixture from a private export |
| `specs/flow-migration/fixtures/live-2026-10-06.json` | the live graph of 2026-10-06 |

**About the fixture:**

- **What it keeps:** structure, the fields in §4 (free text reduced to "present"), and today's `list_tasks` order.
- **What it drops:** titles, bodies and names.
- **Ids:** uncited ids are replaced by a keyed hash whose key was discarded. Ids that carry a person's name or reveal a personal event are never cited, so they are hashed too.

The dry run imports the flow-rule reference calculator from `specs/flow-rule/flow.py`, so the two specs share one implementation of the rule.
