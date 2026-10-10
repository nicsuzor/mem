---
id: graph-lint
title: "Graph linter for the flow model"
type: spec
status: draft
created: 2026-10-06
task: epic_fc1de9ec
epic: aops_ee1257cb
brief: spec_866ee53d
depends_on: flow-rule
tags:
  - ranking
  - flow-rule
  - lint
  - spec
---

# Graph linter for the flow model

**Home:** `specs/graph-lint.md` in the `nicsuzor/mem` repository. Its evidence script is `specs/graph-lint/examples.py`. The checks it specifies are built into `pkb lint` (`src/lint.rs`).

**Status: draft for Nic's decision.** This spec depends on [`flow-rule.md`](flow-rule.md), which is also a draft. It specifies checks and does not implement them. Until both are approved and built, `pkb lint` keeps its current checks.

Traceability tags follow `flow-rule.md`:

- **S1–S16:** settled points in the brief (`spec_866ee53d`).
- **I1–I17:** the brief's invariants.
- **U1–U20:** user stories in `pkb-arch-framework`.
- **Q1–Q31:** questions already open in `flow-rule.md` §15.
- **L1–L12:** questions this spec adds (section 10).
- **§n:** sections of `flow-rule.md`.

---

## 0. Summary for Nic

**What the linter is for.** Under the new model, each task's worth comes from what you price and how links are valued. The linter makes sure those inputs are things the maths can actually read. If they are not, a number goes quietly wrong. It reports on the inputs. It never computes or changes a worth, and it never looks at the screen: what is shown, in what order, or what is hidden.

**What it checks:**

1. **Targets.** Each target without a price gets a warning. Today 36 of your 43 targets have none (the predicate is `STRATEGIC_TARGET_TYPES`, counting the 17 goal nodes alongside the 26 targets; `src/graph.rs:1016`, `:1371`), including `targ_safety` and `qut-f71664e8` ("Meet QUT employment obligations"; that id was observed against the live PKB via `export_graph` on 2026-10-06; not reproducible from the committed fixture or `examples.py`). A price outside −1 to +1, or a price on something that is not a target, is an error.
2. **Links.** A label, sign, quantum or probability the maths cannot read is an error. Today the stated weights "high", "medium" and "Marginal" sit on live links (observed against the live PKB via `export_graph` on 2026-10-06; not reproducible from the committed fixture or `examples.py`). Each scores zero, with only a parse warning that nothing acts on. Links pointing at deleted or cancelled nodes, and two links joining the same pair, are warnings. A link with no value is just noted, because the default is a legitimate answer (S7).
3. **Loops.** Loops are allowed, and the live graph has three. A loop is an error only when every link in it is full strength and every node on it is open. Such a loop has no stable answer, and the maths refuses it. Today's cycle check misses one live case of this shape, a child that depends on its own parent. This rests on a Python re-implementation of today's rule (`old_hard_cycles` in `examples.py`), not on a run of `pkb lint`. It also flags loops the maths would accept.
4. **Decisions.** A decision with only one option is a warning, and so is "finding out" work pointed at something that is not a decision. Neither earns anything.
5. **Deadlines.** A due date with no class (fake, soft or hard) is a warning, and it is read as fake until classed. All six open due dates today have no class.
6. **Coverage.** Open work with no route to any priced target is noted, not warned. That is 1,255 of 1,502 open items, and for most of them zero is the right answer (U8, U9).

**Who may fix what.** Agents fix only mechanical things unasked, such as stray spaces around a scale word (L10). Anything that sets a value (a price, a quantum, a deadline class) can only be proposed by an agent and changes nothing until you approve it. Anything that needs a judgement about the world, such as where an orphaned link should point or how to break a loop, is left for you.

**What retires.** The `dep-hard-cycle` error is replaced by the full-strength loop check; `parent-cycle` stays an error even at quantum 0 (L1 decided, Q1 settled). The broken-reference warnings become one dangling-link check. The parentless-task check retires. The checks on `severity`, `goal_type` and the old weight words go once migration removes those fields. Section 7 lists every change.

**Decisions waiting on you:** twelve linter questions in section 10. The first three matter most:

- **L1.** Decided (2026-10-10): parent cycle stays an error at quantum 0 (Q1 settled: a parent cycle is a filing mistake).
- **L5.** Should an unpriced target fail the lint, so that pricing all 43 is enforced, or stay a warning?
- **L4.** Decided (2026-10-10): full-strength loops are reported by the linter, never refused at write time.

---

## 1. Problem and target

**Problem.** Today's linter checks the inputs of the old engine. Under the flow model those checks are wrong in four ways:

- **They miss what now matters.**
  - Nothing reports an unpriced target, although the maths gives everything behind one zero (S14, U20).
  - Nothing checks the new edge fields (S2).
  - Nothing checks deadline classes (S5).
  - An unrecognised weight word is only a parse warning, and the edge scores zero (`src/graph.rs:1613`; `pkb-rules.md` §6.4).
- **They check the wrong loops.** `dep-hard-cycle` follows `parent` and `depends_on` in the same direction (`src/lint.rs:1622-1662`). It cannot see a child that depends on its own parent, which is a full-strength loop under the flow model (section 5.4). It also errors on loops through done nodes, which the flow accepts.
- **They check fields the maths no longer reads.** These are `severity`, `goal_type`, `edge_template` and `intent` (§9).
- **They treat a missing parent as a defect.** Under the flow model a parentless node is fine. What matters is whether it reaches anything priced (S7, U9).

**Target.** The target has four parts:

- one set of checks over exactly the inputs the flow rule reads: node state, target worth and edge fields (§2);
- checks on the inputs to the rules outside the maths that the brief settles: deadline classes (S5);
- each check given a severity, a real example and a fix authority;
- no check reading anything display produces (S1, I6).

---

## 2. Scope and boundaries

| In scope | Out of scope, and where it lives |
|---|---|
| Checks on target worth, edge fields, loops, decisions, deadline classes and coverage | The flow rule itself: `flow-rule.md` |
| Which current checks retire or change | Conversion of today's fields: migration spec (`epic_d1679d4b`) |
| The diagnostic contract (fields, severities, exit codes) | Write-time enforcement in the MCP tools and the shape of `graph_stats`: engine and tools spec (`epic_2de1b579`). This spec states the check those surfaces call |
| | The densify routine that turns proposals into approved values: skills spec (`epic_80ce44ae`) |
| | Doctrine text in `pkb-rules.md` §6: skills spec |
| | File hygiene checks (`fm-*` YAML and type rules, `md-*`, id and project checks): unchanged |

---

## 3. Architecture and data flow

```text
markdown files ──► pass 1: field checks (per file) ──────────────┐
      │            label, effect, quantum, probability, set_by,  │
      │            worth, deadline_class                          │
      ▼                                                          ├──► diagnostics ──► text / JSON, exit code
  graph build ──► pass 2: structure checks (whole graph) ────────┤
      │            dangling, cancelled, duplicate, self-edges,    │
      │            decisions, unpriced targets, deadlines         │
      ▼                                                          │
  flow engine ──► pass 3: flow verdicts ─────────────────────────┘
                   SaturatedLoop, NoConvergence, loops, reachability
```

**The linter reads:**

- stored frontmatter;
- the graph built from it;
- the flow engine's own verdicts, which are the same functions the engine uses to rank (`saturated_loops`, `NoConvergence`, `on_loops` and reachability; `flow-rule/flow.py:208`, `:85`, `:333`).

**The linter never reads:**

- any display output: order, rank, the ready-leaf filter, the cliff lane, grouping, menus, colours, the treemap;
- any display setting;
- today's date. No check does date arithmetic. The deadline checks look at whether a class is present and valid, never at how near a date is.

**The linter never writes a worth.** In fix mode it writes only the mechanical fixes marked "yes" in section 5.

**When each pass runs:**

- Pass 1 runs on every `pkb lint`, including single-file mode.
- Passes 2 and 3 run when the whole PKB is linted with `--refs`. This is the existing flag for checks that need a full scan (`src/cli.rs:537-539`).

---

## 4. Severity and fix authority

**Severity** keeps the three levels of `src/lint.rs:24-31`, with these meanings under the flow model:

| Severity | Meaning | Effect |
|---|---|---|
| error | The maths cannot read the input, or refuses it, so some number is wrong or absent | `pkb lint` exits 1 (`src/cli.rs:3025`) |
| warning | The maths reads the input, but the number probably does not match reality | reported; exit 0 |
| style | Information for curation; often the right state | reported; exit 0 |

**Fix authority** answers whether an agent may fix the problem without being asked:

| Value | Meaning | Basis |
|---|---|---|
| **yes** | `pkb lint --fix`, or any agent, may apply the fix. The fix keeps the meaning and needs no judgement | `fixable: true` today |
| **propose** | Not unasked. An agent may put a value in a proposal batch, marked `set_by: agent-proposed` with a justification. Aligned to engine R26, it is read at quantum 0.0 whatever its label, and changes no number until Nic approves it | §8 densify contract; `pkb-flow-engine.md` R26; `pkb-rules.md` §6.5, "the working agent never values its own work" |
| **no** | An agent may not change it unasked. It reports the problem to Nic or the task owner | the fix needs a judgement about the world |

---

## 5. Checks

Every check has a rule id beginning `flow-`. An **observed** example is on the live graph:

- fixture ids come from `flow-rule/fixtures/live-2026-10-05.json` and are reproduced by `python3 specs/graph-lint/examples.py`;
- live ids and counts were observed against the live PKB via `export_graph` on 2026-10-06. They are not reproducible from the committed fixture or from `examples.py`.

An **assumed** example edits a real node hypothetically, because the live graph does not carry the field yet. This is the convention of `flow-rule.md` §10.

### 5.1. Targets and worth

| Rule | Fires when | Severity | Real example | Agent may fix unasked? | Traces to |
|---|---|---|---|---|---|
| `flow-target-unpriced` | An open target (matching `STRATEGIC_TARGET_TYPES`, `src/graph.rs:1016`, including `type: goal` per `:1371`) has no `worth` | warning | **Observed:** 36 of 43 open targets. `targ_safety` has 6 incoming edges, 3 from open work, and all of that work gets nothing from it. Live (observed against the live PKB via `export_graph` on 2026-10-06; not reproducible from the committed fixture or `examples.py`): `qut-f71664e8`, `admin-73e215bc` | propose | S14, U20, §5.6 |
| `flow-worth-invalid` | `worth` is not a number in −1.0..=1.0 | error | **Assumed:** `targ_4e2cc92a` given `worth: high` or `worth: 6` | no | S12, S14, §5.6 |
| `flow-worth-not-target` | A node that is not a target (not in `STRATEGIC_TARGET_TYPES`, `src/graph.rs:1016`) carries `worth` | error | **Assumed:** `worth: 0.6` on task `proj-76fbc546`, which would make it a source of its own worth. The same mistake under today's model is observed on the live PKB (observed against the live PKB via `export_graph` on 2026-10-06; not reproducible from the committed fixture or `examples.py`): `severity` on tasks `aops_bug_subagents_fabricated_completion_reports` and `aops_bug_learn_requires_trace_halts_on_cowork` (`pkb-rules.md` §6.2 forbids it) | no | S14, §12 inputs |

### 5.2. Edge fields: labels and signs

**Matching.** Words (labels, effects, scale words, deadline classes) match regardless of letter case, as weight words do today (`src/graph.rs:293`). Case is never a defect. A word padded with spaces does not match, as today (`pkb-rules.md` §6.4). Whether agents may trim the padding unasked is L10. The "yes" entries below that depend on it say so.

| Rule | Fires when | Severity | Real example | Agent may fix unasked? | Traces to |
|---|---|---|---|---|---|
| `flow-edge-label-invalid` | `label` is not one of `serves`, `needs`, `part_of`, `supports`, `alternative`, `settles` | error | **Assumed:** `label: blocks` on `aops_epic_task_lifecycle → spec_a98d0e11`. `blocks` is a computed inverse, not a label (§5.2) | yes, if only padding differs (L10); otherwise no | S2, S3, §5.2 |
| `flow-edge-effect-invalid` | `effect` is not `helps` or `harms` | error | **Assumed:** `effect: negative` on `proj-db6ded3c → targ_safety` | no | S12, §5.1 |
| `flow-edge-negative` | `quantum` or `probability` is below 0. The sign belongs in `effect` | error | **Assumed:** `quantum: -0.3` on `proj-db6ded3c → targ_safety`, meant as "puts safety at risk" | no: `-0.3` may be a typo or a harm, and choosing between them is a judgement | S12, §5.1 |
| `flow-edge-effect-ignored` | `effect: harms` on an `alternative` or `settles` edge. The flow does not read these edges, so the sign has no effect | warning | **Assumed:** `personal_92d5909f → brain_bf2be9d8` as `settles` with `effect: harms` | no | S2, S3, §3.2, §6 |

### 5.3. Edge fields: values

| Rule | Fires when | Severity | Real example | Agent may fix unasked? | Traces to |
|---|---|---|---|---|---|
| `flow-edge-quantum-invalid` | `quantum` is not a number in 0..=1, nor a §5.4 word. This includes values above 1 and old-scale likelihood words such as `probable`, which now belong on probability | error | **Observed today in the legacy field; assumed after migration.** Live (observed against the live PKB via `export_graph` on 2026-10-06; not reproducible from the committed fixture or `examples.py`): `mem_5622c5a7 → aops_epic_worker_environments` with `stated_weight: high`; `aops_investigate_a45_hail_damage_claim → personal-66647271` with `medium`; `personal_eb03659a → aops-f770fe8a` with `Marginal`. Each scores zero today with only a parse warning (`src/graph.rs:1613`). Carried into `quantum` unchanged, `high` is not a §5.4 word. Left in `stated_weight`, it trips `flow-legacy-field` instead. Which happens is the migration spec's choice | yes, if only padding differs (L10); otherwise propose | S2, S7, §5.4 |
| `flow-edge-probability-invalid` | `probability` is not a number in 0..=1, nor a word `numeric_weight()` accepts (`src/graph.rs:292-318`) | error | **Assumed:** `probability: 85%` on `personal_344a9ec6 → targ_safety` | yes, if only padding differs (L10); otherwise propose | S2, §5.5 |
| `flow-edge-unvalued` | An edge states no quantum, so it is read at the default | style | **Observed:** fixture `n_9e0d1da5d4 → n_9da86d3306` (`serves`), the only typed edge with no stated value. Live (observed against the live PKB via `export_graph` on 2026-10-06; not reproducible from the committed fixture or `examples.py`): `aops_3a319150 → aops_8745d500` | propose | S7, U20, §5.3 |
| `flow-edge-set-by-invalid` | `set_by` is not `nic`, `agent-proposed` or `migrated` | error | **Assumed:** `set_by: ida` on `proj-76fbc546 → targ_4e2cc92a` | no | U20, §5.1, §8 |
| `flow-edge-proposal-unjustified` | An `agent-proposed` edge has no `justification` | warning | **Assumed:** a densify batch proposes `quantum: some` on `admin-3e02c20b → task_b3f01c80` with no justification | no: only the proposer can say why | U20, §8; `pkb-rules.md` §6.4 |

### 5.4. Edge endpoints and duplicates

| Rule | Fires when | Severity | Real example | Agent may fix unasked? | Traces to |
|---|---|---|---|---|---|
| `flow-edge-dangling` | An edge names a node that does not exist, whatever the state of its source (open, done or cancelled), as with today's `ref-broken-parent` and `ref-broken-dep` (`src/lint.rs:825-853`). The flow drops it, so the work loses that worth silently | warning (L3) | **Observed (live; observed against the live PKB via `export_graph` on 2026-10-06; not reproducible from the committed fixture or `examples.py`):** `tja-2421247f` `depends_on` `tja-e8890ae8`, which does not exist; 9 such dependency references and 8 such `contributes_to` targets, e.g. `zotmcp-e1d95785 → aops-f770fe8a` | no | I12, U18 |
| `flow-edge-to-cancelled` | Open work has an edge to a cancelled node. The flow does not read it (§3.2) | warning (L3) | **Observed:** 19 fixture edges (13 `supports`, 5 `needs`, 1 `part_of`), e.g. `n_45583b7b2a → n_504e4e1a1d` (`supports`) | no: whether the work goes elsewhere or is cancelled too is a judgement | S6, U19, I12 |
| `flow-edge-duplicate` | Two edges join the same ordered pair. The flow reads them as independent, `1 − (1 − s₁)(1 − s₂)`, so one relationship can count twice | warning | **Observed:** `spec_a98d0e11` is `part_of` `aops_epic_worker_environments` and also serves it at `expected` (fixture `spec_a98d0e11 → n_5a2eb5c714`; observed against the live PKB via `export_graph` on 2026-10-06; not reproducible from the committed fixture or `examples.py`, including the mapping of fixture id `n_5a2eb5c714` to `aops_epic_worker_environments`). 6 fixture pairs, 2 from open work | no: which label is meant is a judgement | S3, I3 |
| `flow-edge-self` | An edge runs from a node to itself | error | **Assumed:** `part_of` from `aops_epic_task_lifecycle` to itself. Today's equivalent, a self-parent, is the error at `src/lint.rs:1766` | no | S8, S15 |

### 5.5. Loops: errors and allowed loops

A loop is a set of nodes that reach each other along flow edges (§3.2). The brief settles that loops are modelled and counted once (S8, S15). The linter therefore separates the loops the maths refuses from the loops it accepts.

| Rule | Fires when | Severity | Real example | Agent may fix unasked? | Traces to |
|---|---|---|---|---|---|
| `flow-loop-saturated` | Every node on a loop is open and every edge in it is a full-strength helps edge (strength 1.0). The maths refuses it (`SaturatedLoop`, §3.2), and its nodes get no worth | error | **Observed shape, assumed state:** fixture loop `n_22a3f9be31 ⇄ n_7ef3b3deee`, which in the 2026-10-05 fixture carries `part_of` 1.0 one way and `needs` 1.0 the other: a child that depends on its own parent. (Under Q1 settled, `part_of` carries quantum 0 so child-depends-on-parent carries cycle product 0.0 and does not saturate the flow, while parent cycles stay an error under `parent-cycle` per L1; full-strength loops occur across mutual `needs: 1.0` or full-strength `serves`). Live example of the fixture's shape (observed against the live PKB via `export_graph` on 2026-10-06; not reproducible from the committed fixture or `examples.py`, including the mapping of the fixture ids to these live ids): `aops_services_user_level_verify` depends on its parent `aops_services_user_level`. The loop is accepted today only because the child is done. **Assumed** reopened under strength 1.0 edges, the flow refuses it. A Python re-implementation of today's `dep-hard-cycle` adjacency (`old_hard_cycles` in `examples.py`, "Loops") finds 0 cycles either way; this was not checked by running `pkb lint` | no | S15, I4, §13 A19 |
| `flow-loop-no-convergence` | A loop containing a harms edge does not settle within the iteration cap (`NoConvergence`, §3.2) | error | **None possible on the live graph.** Every live node converges (I13), and so does the worst harmful loop tested, a pure negative loop at quantum 1 (I16). The test uses a constructed graph | no | I16, §12 failure contract |
| `flow-loop` | Any other loop, which the maths accepts | style | **Observed:** `aops_epic_task_lifecycle ⇄ spec_a98d0e11` (`needs` 1.0, `serves` 0.5; strongest cycle product 0.50). Also a four-node component: the open loop `aops_twin_cost_monitor → aops_bootstrap_dogfood → aops_otel_full_text_container_spans → aops_twin_cost_monitor` (product 0.22), plus the done node `n_13b44e9a94` | not applicable: nothing is wrong; Nic judges whether a loop is real (Q16) | S8, I4, Q16 |

**Loops that are errors, and loops that are allowed:**

- **Error.** Every node is open and every edge is a full-strength helps edge, so it is saturated. A loop whose iteration cannot settle is also an error.
- **Allowed.** Any loop with at least one edge below strength 1.0. Also any loop through a done or cancelled node, because a done node is fixed at 1 and breaks the dependency. Also any loop containing a harms edge that settles.
- **The diagnostic reports the loop's strongest cycle product.** A near-1 loop can then be seen even though it is allowed. Whether to cap it is L7 (Q15).

### 5.6. Decisions

| Rule | Fires when | Severity | Real example | Agent may fix unasked? | Traces to |
|---|---|---|---|---|---|
| `flow-decision-one-option` | An open node has exactly one incoming `alternative` edge. The decision rule needs at least two (§6) | warning | **Assumed:** only `brain_7f772690` relabelled `alternative` under the open decision `brain_bf2be9d8`, not `proj-f8b942d5` | no | I10, U12, §6 |
| `flow-settles-no-decision` | A `settles` edge points at a node with fewer than two `alternative` edges, so it earns nothing | warning | **Assumed:** `personal_92d5909f` `settles` `brain_bf2be9d8`. That is the graph's state today: the decision's options are `part_of` children, not alternatives (§6, "Live finding") | no | I10, U12, §6 |

### 5.7. Deadlines

| Rule | Fires when | Severity | Real example | Agent may fix unasked? | Traces to |
|---|---|---|---|---|---|
| `flow-deadline-unclassed` | An open node has `due` and no `deadline_class`. It is read as `fake` until classed (Q18) | warning | **Observed:** all 6 open nodes with a due date, e.g. `task_d5f610e6` (due 2026-10-05) and `proj-76fbc546` (due 2026-09-30) | propose | S5, U15, I17, §7 |
| `flow-deadline-class-invalid` | `deadline_class` is not `fake`, `soft` or `hard` | error | **Assumed:** `deadline_class: firm` on `brain_61467de3` | yes, if only padding differs (L10); otherwise no | S5, S13, §7 |
| `flow-deadline-class-no-due` | `deadline_class` is set but `due` is not | warning | **Assumed:** `deadline_class: hard` on `task_d5f610e6` after its `due` is removed | no | S5, §7 |

**Left out on purpose.** A hard deadline with no `effort` stops the cliff lane timing it well (§7). Checking for that would tie a lint rule to the cliff trigger, which is open (Q26) and sits outside the maths. Whether to add it is L9.

### 5.8. Coverage and legacy fields

| Rule | Fires when | Severity | Real example | Agent may fix unasked? | Traces to |
|---|---|---|---|---|---|
| `flow-no-route` | Open work has no route to any priced target along edges of nonzero strength, so its worth is exactly 0. A route made only of zero-quantum edges counts as no route, as in the flow (`flow-rule/flow.py:73-82`) | style | **Observed:** 1,255 of 1,502 open nodes, e.g. `academic-b738bdc7` (§10 row 7) | propose: a link via the densify routine | S7, I7, U7, U8, U9, U20 |
| `flow-legacy-field` | A field the migration spec retires is still present after migration, e.g. `stated_weight`, `multiplier`, or `standing_weight` in place of `worth` | warning (promoted to error at migration R8) | **Observed (live; observed against the live PKB via `export_graph` on 2026-10-06; not reproducible from the committed fixture or `examples.py`):** `stated_weight: high` on `mem_5622c5a7 → aops_epic_worker_environments` | yes, but only by running the migration routine; otherwise no | S2, S11; migration spec (`epic_d1679d4b`, `flow-migration.md:499`) |

**Volume.** In text output `flow-no-route` prints one summary line: the count, plus the first ten node ids. JSON output lists every node (L6). This changes how lint output is shown, not how the graph is shown.

**Migration lifecycle.** `flow-legacy-field` is emitted as a warning during migration phases R1–R7 while legacy and flow models run side by side; it is promoted to an error at migration R8 (cleanup) after Nic's explicit `--confirm` (`flow-migration.md:499`).

---

## 6. No check depends on display (S1, I6)

- **What a check may read.** Every check is a function of stored frontmatter, the graph built from it, and the flow engine's verdicts (section 3). None reads an order, a rank, a ready-leaf result, the cliff lane, a grouping, or a display setting.
- **What a check may not read.** No check reads today's date. Section 5.7 checks only whether a class is present and valid.
- **What this rules out.** It rules out checks such as "a high-worth task is not in the top fifty" or "a target with no visible work". It also rules out the hard-deadline-without-effort check (section 5.7, L9).
- **How it is checked.** Test `T-display` (section 9) runs the linter under every display configuration in `flow-rule/display.py` and requires byte-identical output. It also requires that the lint module imports nothing from display.

---

## 7. Current checks: retired, replaced, unchanged

| Current check | Where | Fate under this spec |
|---|---|---|
| `dep-hard-cycle` (error) | `src/lint.rs:1622-1717` | **Retired.** Replaced by `flow-loop-saturated`. It errs both ways: it misses a child that depends on its parent (when `part_of` was 1.0), and it flags hard cycles through done nodes, which the flow accepts |
| `parent-cycle` (error) | `src/lint.rs:1689`, `:1721-1800` | **Kept as an error.** With Q1 settled (`part_of` at quantum 0), a parent cycle is not a saturated loop, but L1 is decided: a parent cycle is a filing mistake and stays an error at quantum 0. The self-parent case is also `flow-edge-self` |
| Write-time cycle rejection: `would_create_parent_cycle`, `would_create_hard_cycle` | `src/mcp_server/handlers_task.rs:452`, `:469`, `:1924`, `:1970`; `handlers_task_lifecycle.rs:1487` | **Retired.** L4 decided: full-strength loops are reported by the linter, never refused at write time |
| `ref-broken-parent`, `ref-broken-dep` (warning) | `src/lint.rs:832`, `:846`, `:866` | **Replaced** by `flow-edge-dangling` for every edge of the model. `ref-broken-dep` stays for `supersedes`, which is not an edge of the model (§8). Until migration removes stored `blocks` and `soft_blocks` (`src/lint.rs:839`), it stays for them too |
| `task-no-parent` (style or warning) | `src/lint.rs:932-958` | **Retired.** A parentless node is not a defect (S7, U9). What matters is `flow-no-route` |
| Parse warning `contributes_to.stated_weight` | `src/graph.rs:1613` | **Replaced** by `flow-edge-quantum-invalid` and `flow-edge-probability-invalid` (error) |
| Parse warnings `standing_weight` range and type | `src/graph.rs:1486`, `:1495` | **Replaced** by `flow-worth-invalid` |
| Parse warnings `severity`, `goal_type`, `edge_template.*` | `src/graph.rs:1427-1462`, `:1512-1562` | **Retired when migration removes the fields** (§9; Q24). Until then unchanged |
| `fm-intent-range`, `fm-intent-type` | `src/lint.rs:670`, `:682`, `:690` | **Unchanged, pending Q10.** The maths no longer reads `intent`, but whether the field survives is open |
| `graph_stats.hard_cycles` | `src/batch_ops/stats.rs:100` | **Replaced** by the saturated-loop list (engine and tools spec) |
| `graph_stats.soft_cycle_count` | `src/batch_ops/stats.rs:103` | **Replaced** by the count of `flow-loop` |
| `graph_stats.affordable_loss_filtered_count` | `src/batch_ops/stats.rs:105` | **Retired.** Affordable loss moves to agent logic (§9) |
| `graph_stats.projects_without_goals`, `disconnected_epics` | `src/batch_ops/stats.rs:90`, `:92` | **Replaced** by the `flow-no-route` count |
| `KNOWN_KEYS` | `src/lint.rs:115-198` | **Changed.** It gains `worth` and `deadline_class`. Edge sub-fields are checked by section 5.2–5.3. Retired keys leave as the migration spec removes them |
| `detect_weight_divergence` | MCP tool | **Not a lint check.** It stays an agent diagnostic, reading `quantum × probability` (§9) |
| `fm-*` file hygiene, `md-*`, `task-no-id`, `task-legacy-id`, `fm-missing-project`, `fm-project-alias`, `fm-unregistered-project`, `superseded-by-hand-written`, `task-missing-ac` | `src/lint.rs` | **Unchanged.** They do not touch the flow's inputs |

---

## 8. Interface contracts

**Diagnostic.** Today's `Diagnostic` (`src/lint.rs:45-51`) gains two fields:

| Field | Type | Meaning |
|---|---|---|
| `severity` | `error` \| `warning` \| `style` | section 4 |
| `rule` | rule id | section 5 |
| `message` | text | names the node, and the edge as `from → to (label)` |
| `line` | optional | as today |
| `fixable` | bool | true exactly when `agent_fix` is `yes` |
| `agent_fix` | `yes` \| `propose` \| `no` | **new**: section 4 |
| `subject` | node id, or `{from, to, label}` | **new**: what the diagnostic is about, so an agent can act without parsing `message` |

**Rule-specific details.**

- A `flow-loop*` diagnostic lists the loop's nodes and its strongest cycle product.
- A `flow-target-unpriced` diagnostic gives the number of open nodes whose only routes end at that target.

**Command line.** Unchanged except as noted:

- `pkb lint` runs pass 1.
- `pkb lint --refs` adds passes 2 and 3.
- `--fix` applies only `agent_fix: yes`.
- `--format json` emits the fields above.
- Exit code 1 means at least one error (`src/cli.rs:3025`).

**Determinism.** The output is a pure function of the PKB files and is byte-identical across runs. It does not depend on file order, node-id order or the clock.

**What the linter must not do.**

- Write `worth`, `quantum`, `probability` or `deadline_class`.
- Create proposals. The densify routine owns them.
- Change any number the engine emits.

---

## 9. Acceptance criteria and tests

Each criterion is something an observer can check by running `pkb lint --refs --format json` on a fixture PKB. The test named beside it is the one the build must carry, in `src/lint.rs` tests.

| # | Observable criterion | Traces to | Test |
|---|---|---|---|
| G1 | An open target with no `worth` yields one `flow-target-unpriced` warning with `agent_fix: propose`; pricing it removes the warning | S14, U20 | `lint_flow_target_unpriced` |
| G2 | `worth` of `high`, `6` or `-1.5` yields `flow-worth-invalid` (error, exit 1); `-1.0` and `1.0` pass | S12, S14 | `lint_flow_worth_invalid` |
| G3 | `worth` on a task yields `flow-worth-not-target` (error) | S14 | `lint_flow_worth_not_target` |
| G4 | Each of `label: blocks`, `effect: negative`, `quantum: -0.3` yields its own error; `label: Serves` yields no diagnostic; `label: "serves "` yields an error whose `agent_fix` is `yes` if L10 is answered yes, and `no` otherwise | S2, S12 | `lint_flow_edge_label_sign` |
| G5 | `effect: harms` on a `settles` edge yields `flow-edge-effect-ignored` (warning) | S2, S3 | `lint_flow_edge_effect_ignored` |
| G6 | `quantum: high`, `quantum: 1.5` and `quantum: probable` each yield `flow-edge-quantum-invalid` (error); `quantum: Most` yields no diagnostic; `probability: 85%` yields `flow-edge-probability-invalid` | S2, S7 | `lint_flow_edge_values` |
| G7 | An edge with no quantum yields `flow-edge-unvalued` (style, exit 0, `agent_fix: propose`) | S7, U20 | `lint_flow_edge_unvalued` |
| G8 | `set_by: ida` yields an error; `agent-proposed` with no justification yields a warning | U20, §8 | `lint_flow_edge_provenance` |
| G9 | An edge to a missing id yields `flow-edge-dangling`, whether its source is open or done; an edge from open work to a cancelled node yields `flow-edge-to-cancelled`, which is not emitted for a done source | I12, U19 | `lint_flow_edge_endpoints` |
| G10 | Two edges between one ordered pair yield one `flow-edge-duplicate`; a self-edge yields `flow-edge-self` | S3, I3 | `lint_flow_edge_duplicate_self` |
| G11 | Two open nodes linked by mutual full-strength helps edges (strength 1.0, e.g. `needs`): `flow-loop-saturated` (error) naming both nodes, and `dep-hard-cycle` is not emitted. The same with one node done: only `flow-loop` (style). Parent cycles stay an error under `parent-cycle` (L1, Q1 settled) | S15, I4, A19 | `lint_flow_loop_saturated` |
| G12 | A cycle over hard dependencies through one done node yields `flow-loop`, not an error | S8, S15 | `lint_flow_loop_through_done` |
| G13 | A two-node loop at strengths 1.0 and 0.5 yields `flow-loop` with cycle product 0.50 | S8, I4 | `lint_flow_loop_allowed` |
| G14 | A loop forced not to converge (iteration cap set to 1 in the test) yields `flow-loop-no-convergence` naming the loop, and nodes outside it are still linted | I16 | `lint_flow_loop_no_convergence` |
| G15 | One `alternative` into an open node yields `flow-decision-one-option`; a `settles` edge to it yields `flow-settles-no-decision`; adding a second alternative clears both | I10, U12 | `lint_flow_decisions` |
| G16 | An open node with `due` and no class yields `flow-deadline-unclassed` (warning, propose); `firm` yields an error; `Hard` yields no diagnostic; a class with no `due` yields a warning | S5, S13, U15 | `lint_flow_deadlines` |
| G17 | Open work with no route to a priced target yields `flow-no-route` (style); text output shows one summary line | S7, I7, U9 | `lint_flow_no_route` |
| G18 | No retired rule id (`dep-hard-cycle`, `task-no-parent`, `ref-broken-parent`) is emitted, and `ref-broken-dep` is emitted only for `supersedes` and, before migration, stored `blocks` and `soft_blocks` | S3, S15, U9 | `lint_retired_rules_absent` |
| G19 | Lint output is byte-identical under every display configuration in `flow-rule/display.py` and with the system date set to 2026-01-01 and to 2027-06-01, and the lint module imports no display code | S1, I6, I11 | `T-display` (`lint_display_independent`) |
| G20 | Running `--fix` changes no `worth`, `quantum`, `probability` or `deadline_class` value except by trimming padding (if L10 allows), and never adds one | §8, `pkb-rules.md` §6.5 | `lint_fix_never_values` |
| G21 | `examples.py` reproduces every observed fixture number in section 5 | brief §D, the evidence standard; no S, I or U number applies | `python3 specs/graph-lint/examples.py` |

---

## 10. Questions for Nic

The questions are ordered by consequence.

1. **L1. Parent cycles if parent links weaken (Q1).** Settled: Q1 gives `part_of` quantum 0. Decided (2026-10-10): a parent cycle stays an error at quantum 0, because a parent cycle is a filing mistake whatever its strength.
2. **L5. Unpriced targets: warning or error?** As an error, `pkb lint` would fail until all 43 targets are priced, which enforces S14. As a warning, the graph stays usable while pricing is under way (U20). Proposed: warning.
3. **L4. Refuse at write time?** Decided (2026-10-10): full-strength loops are reported by the linter, never refused at write time. Write-time rejection is retired.
4. **L2. Harms on hard links.** Can a `needs` or `part_of` edge carry `effect: harms`? Neither has an obvious meaning. Should the linter make that an error? This spec leaves it unchecked, because the label set is itself open (Q21).
5. **L3. Dangling links and links to cancelled nodes.** Both drop worth silently. Should they be errors, rather than warnings as today? On the graph that would put 17 dangling dependency and contribution references and 19 links to cancelled nodes into the error count. The 17 was observed against the live PKB via `export_graph` on 2026-10-06; not reproducible from the committed fixture or `examples.py`; the 19 is from the fixture and is reproduced by `examples.py`.
6. **L7. Strong loops (Q15).** If you cap loop strength, say at 0.9, should a loop at or above the cap be a warning?
7. **L8. Targets serving targets (Q17).** If a target's price already includes what it does for other targets, should an edge between two priced targets be a warning?
8. **L9. Hard deadline with no effort.** Should this be an agent check instead of a lint rule, given that it depends on the open cliff trigger (Q26)? Three of the six open due dates have no effort.
9. **L10. Padding fixes.** Words already match regardless of case, as today. A word padded with spaces does not match and scores zero (`pkb-rules.md` §6.4). May agents trim the padding unasked? Proposed: yes, since it changes no meaning.
10. **L6. Volume of `flow-no-route`.** Is a summary line in text output, with the full list in JSON, the right default for 1,255 items?
11. **L11. Intent checks (Q10).** If `intent` is dropped, its two checks go too. Until then they stay.
12. **L12. Unclassed dates (Q18).** If an unclassed date is read as `fake`, is a warning right, or should it be style until a hard deadline is missed?

---

## 11. Behaviour removed

Once this spec is approved and built:

- **Errors removed:**
  - `dep-hard-cycle`, replaced by `flow-loop-saturated`; `parent-cycle` is kept as an error at quantum 0 per L1 decision;
  - errors on cycles through done nodes, which are no longer reported.
- **Warnings and notes removed:**
  - `task-no-parent`;
  - `ref-broken-parent`, and `ref-broken-dep` except for `supersedes`, all replaced by `flow-edge-dangling`.
- **Parse warnings replaced or removed:**
  - `stated_weight` and `standing_weight` warnings, replaced by `flow-*` errors;
  - `severity`, `goal_type` and `edge_template` warnings, removed once migration removes the fields.
- **Statistics fields:** `graph_stats` `hard_cycles`, `soft_cycle_count`, `affordable_loss_filtered_count`, `projects_without_goals` and `disconnected_epics` are replaced or retired (section 7).
- **Write-time checks:** write-time cycle rejection is retired per L4 decision; full-strength loops are reported by the linter, never refused at write time.
- **Doctrine:** `pkb-rules.md` §6.4 says an omitted weight scores zero "without warning". It now draws a `flow-edge-unvalued` note; the skills spec updates the doctrine.

---

## 12. Files

| Path | What |
|---|---|
| `specs/graph-lint.md` | this spec |
| `specs/graph-lint/examples.py` | reproduces the observed fixture numbers in section 5; reuses `flow-rule/flow.py` and its fixture |
| `src/lint.rs` | where the checks are built (not changed by this spec) |
