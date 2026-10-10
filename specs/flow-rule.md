---
id: flow-rule
title: "Flow rule and edge model"
type: spec
status: draft
created: 2026-10-05
task: epic_a463f704
epic: aops_ee1257cb
brief: spec_866ee53d
tags:
  - ranking
  - prioritisation
  - flow-rule
  - edge-model
  - spec
---

# Flow rule and edge model

**Home:** `specs/flow-rule.md` in the `nicsuzor/mem` repository, with its reference scripts in `specs/flow-rule/`.

**Status: draft for Nic's decision.** This spec proposes the rule that replaces the ranking maths in [`ranking.md`](ranking.md). Until Nic approves it, `ranking.md` remains the description of what ships. It specifies and does not implement. The numbers in it come from the reference calculator in [`flow-rule/`](flow-rule/), which is a checking aid and not the engine.

Traceability tags used throughout:

- **S1–S16:** settled points in the brief (`spec_866ee53d`, "Settled by Nic").
- **I1–I17:** the brief's invariants (section C).
- **U1–U20:** user stories in `pkb-arch-framework`.
- **Q1–Q20:** questions for Nic (section 15), listed there in order of consequence, not by number.

---

## 0. Summary for Nic

**What changes.** Two figures replace today's tuple of eleven terms: what you would fail to gain, and what loss you would fail to avert, if a piece of work were never done. They are never netted into one number. You price the targets. Every link says how much of the thing it points at the work delivers (its *quantum*) and, optionally, how likely that is (its *probability*, default 1.00). Worth flows backwards along links from the targets to the work.

**The rule in one line.** A task is worth what you would lose if it were never done, assuming everything else gets done.

**What that gives you:**

- **Prerequisites and finishing steps.** Two prerequisites of the same thing each carry its full worth, because without either one it fails (S10, U17). The last small step before finished work goes out carries the whole worth (S9, U16).
- **Blocked work.** Blocked work keeps its worth and passes it to whatever unblocks it (S4, U3).
- **Shared foundations.** Work serving several targets adds them up. A target reached by several routes is counted once (U11).
- **Loops.** A loop can add worth but cannot run away (S8, S15). A loop where every link is full strength is rejected as a modelling error, as cycles of hard dependencies are today.
- **Targets you want to avoid.** You price them as negative. Work that protects against one shows a positive *loss averted*, kept beside *gain* and never netted against it (S12, S16, U13).
- **Unlinked work stays at zero.** Anything not linked to a priced target is worth exactly zero. On today's graph that is 1,255 of 1,502 open items (U7, U8, U9).
- **What is not in the maths.** Dates, deadline classes, effort, readiness, grouping and the order of the list are all decided after the maths and never change a number (S1, S5, S13).

**What it costs you:**

1. Worths do not add up across tasks. Two prerequisites of a 1.0 target are each worth 1.0, so "the sum of open work" means nothing and the dashboard must not show it.
2. Only 7 of your 26 targets are priced. Until the rest are, work for unpriced targets is worth zero, even work that today's ranking pushes up by severity, a stakeholder or a date. Example: `task_d5f610e6` is 9th today and 236th under this rule. A hard-deadline class brings it back to the top as its date nears, through the display rule, without touching its worth (section 7).
3. Open decisions use one extra small rule, the value of finding out (section 6).
4. You supply about 153 numbers before deadline classes: 19 target prices, 134 migrated link values to confirm (section 8). Parent links and default quantum are settled (Q1, Q2). Agents propose them in batches of fifty.

**Decisions waiting on you:** section 15 records settled choices (Q1 part_of quantum 0 and prototype hubs, Q2 default quantum 0, Q3 ordering by plain sum of gain and loss averted, Q4 summing option gain/loss, Q8 decision children relabelled alternative, Q17 targets serving targets adding worth, Q20 baseline harms counterfactual semantics) and lists remaining open questions ordered by consequence. Every choice the brief left open is listed; none is decided silently.

---

## 1. Problem and target

**Problem.** Today's ranking (`ranking.md`) has three layers:

- a tuple `(severity_gate, cost_of_delay, tie_breakers)` (`ranking.md:30`);
- eleven terms with separate scales (`ranking.md:82-98`);
- several propagation mechanisms that disagree about direction:
  - urgency relaxation (`ranking.md:333-342`);
  - the effective-intent blocker and ancestor channels (`ranking.md:389-393`);
  - one-hop value lineage with a conduit pass (`ranking.md:427`);
  - the downstream cone (`ranking.md:299-309`).

It mixes calendar arithmetic into the maths (`ranking.md:148-151`, `ranking.md:339-341`). It gives blocked work zero inherited value (`ranking.md:427`), which is the opposite of S4. It also cannot tell worthwhile from worthless work (U7): a stakeholder name alone adds 2,000 to 8,000 points (`ranking.md:215-221`).

**Target.** The target has three parts:

- one edge type carrying a label, a quantum and a probability (S2);
- one rule by which worth flows across every edge (S3);
- every categorical choice in display and agent logic (S1).

The rule must satisfy I1–I17 and serve U1–U20.

---

## 2. Architecture and data flow

```text
inputs (frontmatter)                 the maths (no dates, no types)          display and agent logic
──────────────────────               ───────────────────────────────          ───────────────────────
target worth   (Nic, ±)   ─┐
edge: label, quantum,      ├──►  realisation y (fixed point, §3)  ──►  gain, loss_averted   ──►  ready-leaf filter
      probability, effect  │     knockout per open node                   per node, per target       cliff lane (hard deadlines)
node state (open/done/gone)┘     decision rule (§6)                        routes (explanation)       grouping, menus, ordering
                                                                                                      benefit per effort
```

- **The maths reads:**
  - node state;
  - target worths;
  - edge fields.
- **The maths never reads:**
  - dates;
  - deadline classes;
  - effort;
  - node type;
  - tags;
  - stakeholder;
  - severity;
  - intent.
- **Display reads what the maths wrote.** It may read anything else as well, and it never writes back into the maths (I6, I11).

---

## 3. The flow rule

### 3.1. In plain words (86 words)

> A piece of work is worth what would be lost if it were never done, assuming everything else gets done. Each link says how much of the thing it points at depends on this work: its quantum, discounted by its probability. Losing the work loses that share of what it feeds, and the loss passes on in turn, until it reaches the targets Nic has priced. A target counts once, however many routes lead to it. Different targets add. Gains and losses averted are kept apart.

### 3.2. As a formula

For each node `v`, its realisation `y_v ∈ [0, 1]` is how far `v` is achieved. For a target Nic wants to avoid, it is how far that target is avoided:

```text
y_v = d_v · ∏_{e = (w → v)} φ_e(y_w)

φ_e(y) = 1 − s_e · (1 − y)    if e helps   (losing w loses s_e of v)
φ_e(y) = 1 − s_e · y          if e harms   (doing w costs s_e of v)
s_e   = quantum_e × probability_e

d_v = 0 if v is the node knocked out, else 1
y_v = 1 if v is done
edges touching a cancelled node, and decision-label edges (§6), are not read
```

An edge's effect is read reversed at each end that is a target to avoid (worth < 0). This is how a negative target "flows like any other" (S12): it is computed as the positive target "avoid it" and reported in the loss-averted column.

**Baseline and harms semantics.** In the baseline counterfactual ($y^0$), open harmful edges do not fire: open harmful work is a hazard/risk that has not occurred. Only harms that are already completed in reality (`status: done`) or internal regulatory edges within feedback loops fire in the baseline. When evaluating an open node $u$, $u$'s harms are activated ($y^{\text{with } u}$) and compared to $u$ not done ($y^{\text{without } u}$). This guarantees that open harmful work never rescales or zeroes unrelated work (P4, I2, I9).

For each open node $u$, let $y^{\text{with } u}$ be the fixed point with $u$ done and its harms active, and $y^{\text{without } u}$ be the fixed point with $u$ knocked out:

```text
δ_t(u)          = y^{with u}_t − y^{without u}_t          share of target t at stake on u, in [−1, 1]
gain(u)         = Σ_{t : W_t > 0, δ_t(u) > 0}  W_t  · δ_t(u)
loss_averted(u) = Σ_{t : W_t < 0} |W_t| · δ_t(u) + Σ_{t : W_t > 0, δ_t(u) < 0} W_t · δ_t(u)
```

Gains toward positive targets and losses (whether averting negative targets, or causing losses to positive targets) are carried strictly side by side and never netted inside one column (S16, I15). A task that gains 0.6 on one target and harms another at 0.6 reads `(0.600, -0.600)`, never `(0, 0)`.

**Fixed point.**

- **Helps-only systems.** The map is monotone, and knocking out $u$ can only lower values. A greatest fixed point exists (Tarski 1955). Iteration uses Gauss-Seidel sweeps with damping 1.0, converging rapidly in topological order.
- **Full-strength loops are rejected.** A loop of open nodes in which the cycle product of edge strengths reaches 1.0 has no stable answer. Any loss collapses the whole loop. The rule rejects such loops (`saturated_loops`, `flow.py:208`, class `SaturatedLoop` at `:89`, S15, A19). An isolated saturated loop or failure in one component does not abort the ranking of independent clean components; nodes feeding a failed loop return `null` with the loop named (engine E3).
- **Loops containing a harms edge.** The map is not monotone. Iteration uses synchronous (Jacobi) damped steps with damping 0.5 (Krasnosel'skii 1955; Mann 1953; Bauschke & Combettes 2017). Synchronous updates guarantee that mutual harm results are strictly symmetric and independent of node ID ordering.
- **No fixed point within the iteration cap.** The run fails loudly (`NoConvergence`, class at `flow.py:85`, raised at `:158`, `:180`) for the affected component, while unaffected components remain ranked.

The live graph converges for every node (I13). The worst live-shaped harmful loop, a pure negative-feedback loop at quantum 1, also converges (I16).

**Reference implementation.** `worth_all`, `flow.py:216`; the fixed-point iteration, `_settle`, `flow.py:114`.

**Properties the invariants rely on**, each checked by `invariants.py`:

- **(P1) A source never counts twice.** `|δ_t(u)| ≤ 1`, so `u` carries at most `|W_t|` from `t` (I3).
- **(P2) Every route counts at least its own strength.** `δ_t(u) ≥` the strength of `u`'s strongest route to `t`, so no route is ever under-counted (I12).
- **(P3) Loop-free combination is bounded.** Without loops, `δ_t(u) ≤ 1 − ∏(1 − route strength)` over `u`'s routes to `t`. Anything above that bound comes from a loop and is reported as loop reinforcement (I4, I12).
- **(P4) No normalisation anywhere.** Adding or splitting a node changes no number outside its upstream (I2, I9).

---

## 4. Theory check

The brief's four questions are answered below. Every candidate is run on the same worked example by `flow-rule/theory.py`, and the numbers are reproduced exactly. The example graph is set out in that script's docstring.

| Example node | Role in the example |
|---|---|
| `T` | target worth +1.0 |
| `H` | target to avoid, worth −0.5 |
| `X` | serves `T` at 1.0 |
| `A`, `B` | each needed by `X` |
| `C` | serves `T` at 0.5 and supports `X` at 0.5 |
| `L1`, `L2` | each serve `T` at 0.4 and support each other at 0.5 |
| `F` | supports `L1` at 0.5 |
| `P` | protects against `H` at 0.8 |
| `Q` | serves `T` at 0.6 and brings about `H` at 0.6 |
| `D` | open decision between `O1` (p 0.4, serves `T` 0.6) and `O2` (p 0.7, serves `T` 0.3); `S` settles it |

### 4.1. Necessity or share

**Candidates.**

| Candidate | Theory | A | B | C | X after an unrelated contributor joins `T` |
|---|---|---|---|---|---|
| Share: parts split the whole | Shapley value, efficiency axiom (Shapley 1953); input–output requirement shares (Leontief 1936) | 0.105 | 0.105 | 0.184 | 0.263 → 0.208 |
| Necessity, strongest route | most-reliable path; the semiring framework of Mohri (2002) covers the (max, ×) semiring | 1.000 | 1.000 | 0.500 | 1.000 → 1.000 |
| Necessity, sum over walks | path coefficients (Wright 1934); Katz index (Katz 1953) | 1.000 | 1.000 | 1.000 | 1.000 → 1.000 |
| **Necessity, what is lost if never done** | but-for counterfactual (Pearl 2009, ch. 9); Birnbaum importance at the point where everything else works (Birnbaum 1969) | **1.000** | **1.000** | **0.750** | **1.000 → 1.000** |

**Recommendation: necessity, what is lost if never done.**

- **Share** halves parallel prerequisites (S10 fails). It also moves `X` when an unrelated contributor appears, which breaks I2 and I9.
- **Strongest route** ignores `C`'s second route.
- **Sum over walks** double-counts it: `C` gets 1.0 from two half routes.

**What it breaks.** Worths are not additive across tasks. `A + B = 2.0` against a target worth 1.0. This is the efficiency property Shapley's share keeps and the brief's S10 gives up. No display may sum worth across tasks (section 7).

**Direct answer to item 10: such a rule exists.**

- **Each prerequisite carries full worth.** Under the but-for rule, two parallel prerequisites each carry the full worth of what they unblock.
- **Nothing else is inflated, for two reasons:**
  - Worth flows only from priced targets towards work. A node's own worth never depends on how many predecessors it has (I1: `brain_448bb804` is 0.95 with and without its prerequisites).
  - The rule never normalises. Unrelated nodes do not move (I1: largest change elsewhere 0.0; I2: 0.0).

### 4.2. Loops and reinforcement

Worth of `F`, which feeds the loop `L1 ⇄ L2`. The loop is run at quantum 0.5, and again at 1.0:

| Candidate | Theory | F (loop at 0.5) | F (loop at 1.0) | Counts a source once? |
|---|---|---|---|---|
| Cut loops (drop the edge closing each loop) | no named theory; a baseline | 0.280 | 0.360 | yes |
| Strongest route | max-product path; loops never improve a product ≤ 1 | 0.200 | 0.200 | yes |
| Sum over walks | Katz 1953; the input–output inverse `(I − Q)⁻¹` (Miller & Blair 2009) | 0.400 | diverges (39.8 after 200 terms) | no |
| **What is lost if never done** | greatest fixed point of a monotone map (Tarski 1955); noisy-OR combination (Pearl 1988, ch. 4) | **0.317** | **rejected** (full-strength loop); 0.537 at quantum 0.9 | **yes** (≤ 1 × worth) |

**Recommendation: what is lost if never done.**

- **It models reinforcement.** The loop raises `F` from 0.200 (strongest route) to 0.317, which satisfies S8.
- **It stays bounded.** As the loop nears full strength the walk sum diverges, but this rule stays below `T`'s worth: 0.537 at quantum 0.9. That satisfies S15 and I4.
- **Real loops exist and show the reinforcement.** On the live graph:
  - On a real three-node loop, two nodes gain from it and one is unchanged (I4):
    - `aops_otel_full_text_container_spans`: 0.2975 → 0.34;
    - `aops_bootstrap_dogfood`: 0.3369 → 0.3391;
    - `aops_twin_cost_monitor`: unchanged at 0.3467.
  - One node (`n_d663317dd7`) upstream of a different real loop carries 0.971 of `targ-7d49f8a0`, against 0.945 from its simple routes alone (I12).
  - Whether these loops are genuine reinforcement or modelling artefacts is open (Q16). The first runs through a `part_of` edge.

**What it breaks.**

- **No closed form.** A node's number is a fixed point, not a closed-form sum. The explanation lists routes and states separately how much a loop adds (I12).
- **Strong loops amplify weak feeders.** Near full strength the response is continuous but steep. With the example loop at 0.9, a feeder at quantum 0.05 carries 0.158, against 0.020 by its strongest route. At strength 1 the answer jumps from 0 to the whole loop, which is why such loops are rejected (§3.2).
- **Harmful loops can oscillate.** A loop containing a harms edge can oscillate under plain iteration. Thomas (1981) conjectured that sustained oscillation needs a negative loop; Snoussi (1998) and Gouzé (1998) proved it for differential systems. Damped iteration settles every case tested, including a pure negative loop at quantum 1 (I16). A general convergence proof for non-monotone loops is not offered. Non-convergence fails loudly instead.

### 4.3. Decisions and value of information

Worth of `S`, the work that would settle `D`. The options are worth 0.6 (p 0.4) and 0.3 (p 0.7):

| Candidate | Theory | S open | S once D decided |
|---|---|---|---|
| None in the maths (agent logic only) | — | 0.000 | 0.000 |
| Stake: best minus worst option | no named theory; a baseline | 0.300 | 0.000 |
| **Expected value of perfect information** | Howard 1966 | **0.126** (informed 0.366 − blind 0.240) | **0.000** |

**Recommendation: expected value of perfect information, as a second small rule on decision nodes (section 6).**

- **The stake candidate ignores probabilities.** It pays 0.300 to settle a choice whose blind best already captures most of the value.
- **"None" fails I10 and U12.**

**What it breaks.**

- **It adds a second rule.** The only categorical input is two decision labels, `alternative` and `settles`.
- **It needs one scalar per option.** Comparing options needs one figure each. The reference sums gain and loss averted for this comparison only. Whether that is allowed is Q4. The result is reported in its own `decision_value` column, never folded into gain or loss averted.
- **Options are worth what they would deliver if chosen.** Alternatives are therefore not exclusive inside the flow. Exclusivity lives in display, which must not add up alternatives.

### 4.4. Negative targets and signed edges

| Candidate | Theory | P (protects) | Q (gains and harms) | X (linked to nothing bad) |
|---|---|---|---|---|
| One net figure | expected value | 0.400 | 0.300 | 1.000 |
| Two columns, occurrence form (harm happens unless a protection stops it) | layered defences (Reason 1990); the "Swiss cheese" model (Reason 2000) | (0, 0.400) alone; (0, 0.000) with a second, done, full protection | undefined: the form has no term for causes | (1.000, 0): no link to `H` |
| **Two columns, avoidance form** | gains and losses kept apart (Kahneman & Tversky 1979); the same rule as a positive target | **(0.000, 0.400)** | **(0.600, −0.300)** | **(1.000, 0)** |

**Recommendation: two columns, avoidance form.**

- **It is phrasing-invariant.** Restating `targ_safety` ("no one gets hurt") as a target to avoid ("someone gets hurt", −0.35, protections as harms edges) gives each protective task the same figure in the loss-averted column as it had in the gain column (I14).

  | Task | Gain column | Loss-averted column |
  |---|---|---|
  | `proj-db6ded3c` | 0.2625 | 0.2625 |
  | `proj-76fbc546` | 0.2625 | 0.2625 |
  | `personal_344a9ec6` | 0.2822 | 0.2822 |

- **The occurrence form is not.** On the live graph, two done "Certain" protections exist. Under the occurrence form they already stop the harm entirely, so every open protection would be worth zero. Whether a hazard needs every protection or just one is then decided by how the target is phrased.
- **The net figure fails I15.** A task with gain 0.95 and loss 0.95 nets to 0, the same as a task linked to nothing.

**Signed edges are needed.** The `harms` effect is used two ways:

- for work that puts a target at risk;
- for protections pointing at a target to avoid.

**What it breaks.**

- **Sorting needs a choice.** Ordering by two figures needs a rule (Q3).
- **Negative feeds into negative.** Edges between two targets to avoid are read with both ends reversed. The effect is "helps", which is right: avoiding one helps avoid the other.

---

## 5. Edge model

### 5.1. Edge fields (S2)

| Field | Type | Default | Read by |
|---|---|---|---|
| `from` | node id (the work) | required | maths |
| `to` | node id (what it serves) | required | maths |
| `label` | one of §5.2 | `serves` | display, linter; decision rule for `alternative`/`settles` |
| `quantum` | float `0.0..=1.0`, or a §5.4 word; the linter rejects values above 1 | the default quantum (§5.3) | maths |
| `probability` | float `0.0..=1.0`, or a §5.5 word | `1.00` | maths |
| `effect` | `helps` \| `harms` | `helps` | maths |
| `justification` | text | none | agents, Nic |
| `set_by` | `nic` \| `agent-proposed` \| `migrated` | `nic` | densify routine, linter (engine E17) |

**Strength.** Strength is `quantum × probability` (S2; `kb_edge_weight_quantum_vs_confidence`).

**The probability field is reinstated.** `ranking.md:487` rejected a separate quantum term as collinear. That holds only for the product. Separate fields are kept because Nic answers them as different questions: "how much" and "how likely" (S2). The maths reads only the product.

### 5.2. Label set

Labels name what the link is for. Two of them feed the decision rule. None of them changes how worth flows (S3).

| Label | Meaning | Default quantum | Display use |
|---|---|---|---|
| `serves` | advances what it points at | default (§5.3) | — |
| `needs` | the thing pointed at cannot proceed until this is done (hard block) | 1.0 | ready-leaf filter |
| `part_of` | a component of a larger piece of work | 0.0 (Q1 settled) | grouping, progress |
| `supports` | makes the thing pointed at easier or better | default (§5.3) | — |
| `alternative` | one option of an open decision | — (decision rule only) | menus; never summed |
| `settles` | finding this out would settle the decision | 1.0 (decision rule only) | — |

**Labels not carried over.**

- **Wikilinks and `supersedes`.** These are not edges of this model (section 8).
- **A separate `blocks` label.** Today's `blocks` and `soft_blocks` are inverses filled in from `depends_on` and `soft_depends_on` by the `compute_inverses` stage (`ranking.md:41`; `src/graph_store.rs:477`). They are not stored.

**Whether a hard block is its own label.** This model keeps it as its own label (`needs`) because display needs it for the ready filter. In the maths it is a contribution at full quantum (Q5).

### 5.3. Default quantum (S7)

- **Proposed default quantum: 0.0** (Q2 decides). An edge with no stated quantum moves no worth.
- **What 0.05 would change.** On today's graph only one typed edge states no quantum, so a default of 0.05 leaves 246 open nodes carrying worth either way. If every wikilink also became an edge at 0.05, 604 open nodes would carry worth instead of 246, and 932 nodes would sit on loops instead of 8 (I7). Whether "very low" beats zero is Q2.
- **Display behaviour.** Display shows an unvalued link as unvalued, never as weak.

### 5.4. Quantum scale

The question an agent asks is: "If this were never done and everything else were, how much of the thing it points at would be lost?" Five words map to numbers, convex like the standing-weight anchors:

| Word | Quantum | Meaning |
|---|---|---|
| all | 1.00 | it fails without this |
| most | 0.60 | most of its value goes |
| a good part | 0.30 | a substantial part goes |
| some | 0.10 | a noticeable part goes |
| a little | 0.03 | it is slightly worse |
| none | 0.00 | the default |

The old seven-word scale mixed amount and likelihood (`kb_edge_weight_quantum_vs_confidence`). Its words move to the probability scale (§5.5). The anchors above are a proposal (Q6).

### 5.5. Probability scale

The probability is the chance the work delivers its quantum. The Renooij–Witteman words already parsed by `numeric_weight()` (`src/graph.rs:292-318`; `ranking.md:466-473`) are reused unchanged:

| Word | Value |
|---|---|
| certain | 1.00 |
| probable | 0.85 |
| expected | 0.75 |
| fifty-fifty | 0.50 |
| uncertain | 0.25 |
| improbable | 0.15 |
| impossible | 0.00 |

The default is 1.00 (S2).

### 5.6. Target worth scale, positive and negative (S12, S14)

Nic prices every target. The five standing-weight anchors (`pkb-standing-weight-elicitation-instrument`, "Scale") are mirrored for targets to avoid:

| Positive anchor | Worth | Negative anchor | Worth |
|---|---|---|---|
| Critical | +1.00 | Catastrophic loss | −1.00 |
| High | +0.60 | Severe loss | −0.60 |
| Substantial | +0.35 | Substantial loss | −0.35 |
| Moderate | +0.15 | Moderate loss | −0.15 |
| Low | +0.05 | Minor loss | −0.05 |

**Units.** Gain and loss averted are in the same units. One unit is the worth of a Critical target.

**Unpriced targets.** An unpriced target has worth 0 and is never inferred. The linter lists it (U20).

**Open questions.** Whether these anchors stay is Q7. Severity's 0–4 field is not read by the maths. It is listed for migration as a pricing prompt (section 9).

---

## 6. Decision rule (value of information)

**Where it applies.** On each open node `D` that has at least two incoming `alternative` edges:

```text
w_i  = gain(O_i) + loss_averted(O_i)          worth of option i if chosen and it works
p_i  = probability on O_i's alternative edge   chance option i works out
EVPI(D) = E[ max(0, max_i X_i · w_i) ] − max(0, max_i p_i · w_i),   X_i ~ Bernoulli(p_i) independent
settle worth(u) += quantum(u → D) · EVPI(D)   for each open u with a settles edge to D
```

**Mechanics.**

- `E[max]` is computed exactly by sorting options by `w_i` (`expected_best`, `flow.py:271`).
- Once `D` is done, the term is zero (I10).
- `alternative` and `settles` edges are not read by the flow, so options are not treated as jointly necessary.

**Settle worth is shown as its own figure, `decision_value`, with the route "settles D".** It is not added to gain or loss averted, so neither column carries a mix of the other (S16). Section 4.3 covers why this is a second rule and what it breaks.

**Live finding.** The graph models an open decision (`brain_bf2be9d8`) by making its options `part_of` children. Under §3 this makes every option necessary to the decision. Migration should relabel such children as `alternative` and give each option the decision's own outgoing edges, since an option, if chosen, delivers what the decision delivers (Q8). Row 10 of section 10 does exactly that.

---

## 7. Rules outside the maths (S1, S5, S13)

Each rule below reads the maths' output and nothing it writes feeds back (I6). The reference versions are in `flow-rule/display.py`.

| Rule | What it does | Inputs | Reference |
|---|---|---|---|
| Deadline classes | Each `due` carries `deadline_class: fake \| soft \| hard`; proposed: an unclassed date is treated as `fake` (U15; Q18) | `due`, `deadline_class` | — |
| Cliff lane | A hard deadline surfaces its node at the top once `days_left ≤ effort_days + buffer`, whatever its worth; fake and soft never do (S13, U14, I17) | `due`, class, `effort`, today | `display.py:47` |
| Soft → hard | A soft deadline becomes hard only by a deliberate agent step after repeated extension, recorded on the node (S5) | extension history | skills spec (`epic_80ce44ae`) |
| Ripeness | Proposed: an opportunity stops pulling when it is set `cancelled` or its target repriced; nothing infers it (S6, U19). Who does it is Q13 | state, worth | — |
| Ready-leaf filter | Hide work that is not an open, actionable leaf with no open `needs`; targets are never listed as work (S4) | labels, state, type | `display.py:35` |
| Grouping and progress | Show `part_of` trees and the share of children done; never sum worths (§4.1) | `part_of` edges | — |
| Benefit per effort | `gain / effort_days` and `loss_averted / effort_days` shown side by side; the finish line stands out because the last step carries full worth (U16, I8). Ordered by their plain sum (settled Q3, S17); both figures stay shown separately (S16) | worth, `effort` | `display.py:55` |
| Ordering | Cliff lane first, then by the plain sum of gain and loss averted (settled Q3, S17) | — | `display.py:55` |
| Must-not-miss floor | Proposed: the floor is the cliff lane plus a list Nic keeps; obligations with a hard deadline reach it whatever their worth (U5; Q19) | class, list | — |
| Varied menu | Stuck mode picks across groups and effort, not top-N (U2) | worth, groups, effort | dashboard spec (`epic_adf10cd3`) |

---

## 8. Current edge kinds mapped to the new edge

**Snapshot used for these counts.** The live export of 2026-10-05, about 01:20 UTC. The counts are edges whose source is open and whose destination is not cancelled. The brief's counts (parent 555, `contributes_to` 81, `soft_depends_on` 48, `depends_on` 47) were taken earlier with a filter it does not record. The differences run both ways (parent 555 → 524, `depends_on` 47 → 64) and are not reconciled here.

| Today | Open-source count | New label | Quantum given | Probability | What is lost |
|---|---|---|---|---|---|
| `parent` | 524 | `part_of` | 0.0 (Q1 settled) | 1.00 | filing label; grouping/progress read it; carries no worth in the flow; class-level worth comes from prototype nodes used as unpriced hubs (settled Q1, S19, S20) |
| `depends_on` | 64 | `needs` | 1.0 | 1.00 | nothing |
| `soft_depends_on` | 51 | `supports` | 0.3, today's soft factor (`ranking.md:335`), `set_by: migrated` | 1.00 | the reading "soft = optional" becomes an explicit, re-elicitable quantum |
| `contributes_to` | 83 | `serves` | `numeric_weight()` of the stated word (`src/graph.rs:292`), capped at 1.0, `set_by: migrated`; an edge with no stated word gets the default | 1.00 | the old word mixed amount and likelihood; re-elicitation splits it (§5.4) |
| wikilink | 1,876 | not an edge | — | — | nothing: `ranking.md` never mentions wikilinks (searched for "wikilink" and "relates"), and the downstream cone expands only over `blocks`, `soft_blocks`, children and reverse `contributes_to` (`ranking.md:304`) |
| `supersedes` | not in the fixture | not an edge (agent logic) | — | — | nothing |

**Effect on the live run.** With these defaults, the live graph has 1,502 flow edges with nonzero strength:

| Label | Flow edges |
|---|---|
| `part_of` | 1,093 |
| `serves` | 141 |
| `needs` | 141 |
| `supports` | 127 |

These counts include edges from done nodes, which the flow reads as realised.

**Parent links and prototype hubs (settled Q1).** `part_of` carries quantum 0.0 in the flow model (S19). It is a filing label: grouping and progress within an epic stay in display reading `part_of` (section 7). Class-level worth comes from prototype nodes used as unpriced hubs (S20): each instance links to its prototype at quantum 1.0; the prototype's own `serves` links to priced targets decide what every instance is worth; the prototype stays unpriced and passes worth through. The migration spec (`epic_d1679d4b`) decides storage.

**What the parent default moves.** `part_of` is 1,093 of the 1,502 flow edges. At quantum 1.0, 246 open nodes carry worth; at quantum 0 (settled Q1), 49 do (`invariants.py`, row 7). The other 197 carry worth through explicitly valued links or prototype hubs.

**Inputs Nic must supply:**

- 19 unpriced targets;
- 51 migrated `supports` quanta and 83 migrated `serves` words to confirm;
- a deadline class for each open node with a `due` date (§7). Unclassed dates are read as `fake` until classed (Q18), so this is not needed on day one.

That is 153 inputs before deadline classes: about three batches of fifty, the target prices first.

**Densify routine.** Agents propose values and Nic approves them in batches of fifty, ordered by how much worth the batch would move. The skills spec (`epic_80ce44ae`) owns this routine. This spec fixes only the contract: densify always writes `set_by: agent-proposed` explicitly and a `justification`; an agent-proposed link reads at 0.0 whatever its label (so an unapproved proposal moves no number; display still reads the label for blocking; C5, E17).

---

## 9. Current engine measures mapped

Every measure in `ranking.md` is mapped to one of three outcomes:

- **produced by the rule**;
- **moved** to display or agent logic;
- **dropped**.

| Measure (ranking.md) | Outcome | Reason |
|---|---|---|
| `focus_tuple` (`:30`) | dropped | one rule replaces the tuple; ordering is display (Q3) |
| `focus_score` display number (`:33`) | dropped | gain and loss averted are the numbers shown |
| `severity_gate` (`:78`, `:124-137`) | moved | catastrophic obligations are hard-deadline cliffs or −1.00 targets; the gate itself is not maths (S13) |
| `cost_of_delay` (`:79`, `:141-143`) | dropped | replaced by gain and loss averted |
| `tie_breakers` (`:80`) | moved | ordering is display |
| `intent_pressure` (`:105-122`) | dropped | Nic's priority is expressed by pricing targets; a pinned task becomes display (Q10) |
| `deadline_pressure_multiplier` (`:139-154`) | moved | deadline classes and the cliff lane (S5, S13, I11) |
| courtesy-review decay (`:156-188`) | dropped | an unclassed date is `fake` and never presses (U15) |
| `age_staleness_bonus` (`:190-199`) | dropped | age is not worth (U8) |
| `downstream_weight` (`:201-209`, `:297-313`) | dropped | blast radius is replaced by worth reaching unblockers (brief, "Leaning") |
| `stakeholder_waiting` (`:211-226`) | moved | a waiting person is agent logic (triage, U1) or a priced target; a name alone adds no worth (U7) |
| `urgency`, `S_lex`, `f(slack)`, propagation, conduit (`:228-236`, `:325-346`) | dropped / moved | severity becomes pricing; slack becomes the cliff lane; propagation is replaced by §3 |
| `chain_slack` (`:348-356`) | moved | display may show the tightest hard deadline downstream |
| `voi_term`, `voi_value` (`:238-251`, `:358-373`) | produced | by the decision rule (§6) |
| `uncertainty`, `confidence` (`:375-385`) | produced | as edge probability; node-level confidence is dropped |
| `value_lineage` (`:253-259`, `:421-431`) | produced | it is the rule's gain, now multi-hop, reaching blocked work |
| sibling-contributor independence (`:428`) | produced | contributors to one target are scored by their own loss, never reduced by siblings |
| `effective_intent` (`:387-395`) | dropped | no intent in the maths |
| `scope` (`:397-401`) | moved | display (size of a `part_of` tree) |
| `pagerank`, `betweenness`, degrees (`:403-408`) | moved | already diagnostics only (`:435-446`); stay as gardening lenses |
| `criticality` (`:315-323`) | moved | diagnostic only |
| `unlock_breadth` (`:410-419`) | dropped | a blocker carries the full worth of what it unblocks (I5) |
| `affordable_loss` filter (`:94`) | moved | agent logic (effectuation), not maths |
| standing weight (`:421-431`) | produced | the target worth input, now signed |
| verbal contribution scale (`:461-476`) | produced | split into quantum (§5.4) and probability (§5.5) |
| `multiplier` / `x` (`:478-487`) | dropped | quantum is stated directly |
| `goal_type` gating (`:128`, `:330`, `:426`, `:453`, `:455`) | dropped | in `ranking.md`, `goal_type == "committed"` gated SEV4 lexicographic overrides (`severity_gate`, `S_lex`, overdue-pin guard); ordinary pricing was not gated on `goal_type` (`:426`). Under the new model, target pricing operates directly on priced targets without category gating, and hard deadlines form the cliff lane regardless of goal_type |
| `waiting_since` timestamp anchor (`:219`) | dropped | in `ranking.md`, `waiting_since` (or `created`) was the timestamp anchor for the stakeholder waiting clock; dropped because stakeholder waiting is removed from the maths (a waiting person is handled in agent triage or via explicit target pricing) |
| cone depth cap `MAX_CONE_DEPTH = 20` (`:304`, `:333`, `:354`) | dropped | the old engine bounded BFS downstream cone expansion and urgency relaxation to 20 hops; the flow rule computes global fixed points across all hops without an artificial depth cutoff |
| ready / blocked / roots (`:497-517`) | moved | ready-leaf filter (display); actionable views restricted to claimable task leaves |
| ready predicate: `ACTIONABLE_TYPES` (`["task", "learn", "pr"]`) and `CLAIMABLE_TYPES` (`["task"]`) (`:499-504`) | moved | display ready-leaf filter (Q31) |
| ready predicate: `has_acceptance_criteria` gate for `inbox` status (`:505`) | moved | display ready-leaf filter (Q31) |
| ready predicate: `COMPLETED_STATUSES` `{"done", "cancelled"}` (`:506`) | moved | node state mapping in maths (done nodes have $y=1$, cancelled nodes excluded) |
| ready predicate: transitive dependency checking across `blocks` chains (`:506`, `:514`) | moved / produced | replaced by `needs` edges where blocked work passes worth to unblockers (I5) |
| `focus_cmp`, `queue_rank` (`:519-530`) | moved | ordering is display |
| severity ladder (`:449-457`) | dropped | severity is not read; it prompts pricing (Q24) |
| `blocking_urgency` (pipeline stage `:46`; `src/graph_store.rs:503`) | dropped | a blocker carries the full worth of what it unblocks (I5); urgency by date is the cliff lane |
| `stakeholder_exposure` (input to criticality, `:318`) | moved | display, with `criticality` |
| `deadline_pressure_active` flag (`:152`) | dropped | no date term in the maths (I11); display reads `due` and `deadline_class` directly |
| `has_real_stakes`, `is_human_gate` gates (`:161-172`, `:213`) | dropped | they gate terms this spec removes (courtesy decay, stakeholder waiting); a human gate is agent logic |
| committed-SEV4 overdue pin guard (`:342`) | moved | the cliff lane for hard deadlines (S13) |
| `order`, `id` tie-breaks (`:80`, `:522`) | moved | ordering is display; a deterministic final tie-break stays there |
| `has_open_question`, `dep_resolution_ratio`, C_open / C_divergence gates (`:366-370`, `:381`) | dropped | value of information comes only from `alternative` edges (§6). An open question with no modelled alternatives earns nothing until migration or an agent adds them (Q8) |
| `focus_picks` / `pkb focus` surfacing (`:346`, `:553`) | moved | display, reading gain and loss averted; dashboard spec (`epic_adf10cd3`) |
| `export_graph` fields `cost_of_delay`, `severity_gate`, `queue_rank` (`:524-530`) | dropped | replaced by the outputs in §12; the dashboard's `/api/graph` must read them instead (`epic_adf10cd3`) |
| raw `intent` (P0–P4) and its consumers: `list_tasks` priority filter, ready sorting, `priority_weight` (`:306`, `:395`) | moved, pending Q10 | not read by the maths; whether the field survives as a filter or pin is Q10 |
| `detect_weight_divergence` (`src/graph_store.rs`, `compute_divergence_anomalies`) | moved | agent diagnostic; must read `quantum × probability`, since the old word mixed the two |
| `current_weight` (`src/graph.rs:262`, dormant) | dropped | no reader; quantum and probability replace it |

---

## 10. Invariants, demonstrated on real nodes

**Reproducing the numbers.** Every number below is printed by `python3 specs/flow-rule/invariants.py`, which runs in under a minute. The script works on the committed fixture `specs/flow-rule/fixtures/live-2026-10-05.json`:

- 3,710 nodes and 6,744 edges, taken from the live export of 2026-10-05;
- titles removed;
- node ids not cited here replaced by a keyed hash (the key is held privately);
- today's `focus_score` and `value_lineage` kept for the 539 ranked tasks.

**Assumed edits.** Some rows test a field the live graph does not carry yet, such as a split, a negative target or a deadline class. Those rows edit real nodes hypothetically and are marked **assumed**.

| # | Invariant | Real nodes | Numbers | Result |
|---|---|---|---|---|
| 1 | Two parallel prerequisites each carry full worth | `brain_448bb804`, prerequisites `brain_c57fbad6`, `personal_033d02b8` | each 0.95 = 0.95, and still 0.95 with every other route of theirs removed (control); `brain_448bb804` 0.95 with or without them; largest change elsewhere 0.0 | pass |
| 2 | Splitting into necessary parts changes nothing unrelated | `personal_033d02b8` split in two (**assumed**) | parts 0.95 and 0.95; largest change at any other node 0.0 | pass |
| 3 | One source by several routes counts once; two sources add | `proj-76fbc546` → `targ_4e2cc92a` directly (1.0) and through its parent (0.5) | share at stake 1.0, not 1.5; gain 0.60 × 1.0 + 0.35 × 1.0 = 0.95 | pass |
| 4 | A reinforcing loop converges and is worth at least as much | live loop `aops_twin_cost_monitor` → `aops_bootstrap_dogfood` → `aops_otel_full_text_container_spans` → back; a new feeder at 0.5 (**assumed**) | feeder 0.2965 with loop vs 0.2801 opened; otel spans 0.34 vs 0.2975; bootstrap 0.3391 vs 0.3369; twin monitor 0.3467 both | pass |
| 5 | A blocked task passes worth to its unblockers | `brain_448bb804` (not a ready leaf) | keeps 0.95; each open blocker 0.95, and 0.95 when blocking is its only route (control) | pass |
| 6 | Changing a display rule changes no number | all | 8 display configurations, 4 distinct orderings; worth table byte-identical; `flow.py` does not import display | pass |
| 7 | A node linked to nothing priced carries the default | 1,255 of 1,502 open nodes; e.g. `academic-b738bdc7`, `task_d5f610e6` | all 1,255 exactly 0.0; 1 unvalued typed edge, so default 0.05 still gives 246; wikilinks as edges at 0.05 would give 604 (932 nodes on loops); `part_of` at quantum 0 gives 49 (settled Q1) | pass |
| 8 | All but one necessary step done: the remaining step carries full worth | `proj-f8b942d5` (4 children done, 1 open) → `admin-3e02c20b` | both 1.11; under necessity every open part carries it, so the finish line shows through worth per day: 11th of 319 ready leaves | pass |
| 9 | An opportunity takes one node and its edges | new target 0.35 served by `admin-3e02c20b` (**assumed**) | measured: 1 node and 1 edge added, 0 existing inputs changed; 2 nodes upstream; largest change elsewhere 0.0; `admin-3e02c20b` 1.11 → 1.46 | pass |
| 10 | An open decision weights the work that settles it | `brain_bf2be9d8`; options `brain_7f772690` (p 0.4), `proj-f8b942d5` (p 0.3), relabelled from `part_of` and given the decision's own edges; settled by `personal_92d5909f` (**assumed**) | options 1.11 each; EVPI 0.1998 (informed 0.6438 − blind 0.444); prices doubled 0.3996; decided 0.0 | pass |
| 11 | No date arithmetic in the flow | all | no date token in the rule's code; every due date shifted by 400 days: largest change in any figure 0.0, while the cliff lane changes (16 → 0 dates on it if all were hard) | pass |
| 12 | Every number is explained as routes to priced sources | 269 (node, target) pairs | 0 below the strongest route; 0 above the combined routes without a loop; 1 above by loop reinforcement, shown as such (`n_d663317dd7`: 0.9712 against 0.9449); 162 single-route pairs equal their route. Example: `admin-3e02c20b`: `task_b3f01c80` × 1.00 and `targ_4e2cc92a` × 0.85, both via `proj-f8b942d5` → `brain_bf2be9d8` | pass |
| 13 | Runs over the live graph; differences explained | 3,710 nodes, 1,502 open, 1,502 flow edges (by coincidence equal), 7 priced targets | every open node in under 0.5 s; section 11 | pass |
| 14 | Protection against a negative target carries positive weight by the same rule | `targ_safety` priced +0.35 vs restated as −0.35 harm (**assumed**); `proj-db6ded3c`, `proj-76fbc546`, `personal_344a9ec6` | gain column 0.2625 / 0.2625 / 0.2822 = loss-averted column 0.2625 / 0.2625 / 0.2822 | pass |
| 15 | Large gain with equal loss ≠ linked to nothing | `proj-76fbc546` also brings about a −0.95 harm (**assumed**); `pos_harm_task` serves `pos_target1` (1.0) at 0.6 and harms `pos_target2` (1.0) at 0.6 vs `academic-b738bdc7` | (0.95, −0.95) and (0.60, −0.60) vs (0, 0); netted, all would read 0 | pass |
| 16 | A loop with a harmful edge settles | the live loop of row 4 with its closing edge as harms (**assumed**) | synchronous damped iteration (damping 0.5): feeder 0.0396, loop settles at 0.3878 / 0.3878 / 0.8163; pure negative loop at quantum 1: 0.0156, settles symmetrically at 0.2308 / 0.2308 / 0.7692 | pass |
| 17 | A hard deadline surfaces its work as the date nears; fake or soft never | `task_d5f610e6` hard, due 2026-10-05, worth 0; `n_c2e542fd02` fake, worth 0; `n_2dcb93a9c8` soft, worth 0.35 (**assumed** classes) | hard: 1,493rd on 1 Sep, 1st from 25 Sep; fake: never on the cliff (1,169th–1,170th); soft: never on the cliff (51st–52nd) | pass |

---

## 11. Live run compared with today's ranking (I13)

**Today's order.** "Today" means the order returned by `list_tasks` on 2026-10-05: 539 tasks, sorted by `focus_cmp` (`ranking.md:519-522`).

**New order.** "New" orders the same 539 tasks by gain plus loss averted. Ties are broken by today's order. Summing nets the two figures, so this ordering is a comparison device only, not a proposal (Q3).

Of the 539 tasks, 235 carry worth.

| Node | Today | New | Worth | Why it differs |
|---|---|---|---|---|
| `proj-76fbc546` | 1 | 8 | 0.95 | Today (score 14,215) it is a container, so its own value lineage is passed down (`ranking.md:427`); it is lifted by a named stakeholder multiplied by an overdue date (`ranking.md:141-151`, `:215-218`). New: dates are outside the maths; it carries two priced targets (0.60 + 0.35), below seven nodes carrying 0.60 + 0.51. |
| `task_e79f1a57`, `personal_92d5909f`, `brain_7f772690` | 2–4 | 1–3 | 1.11 | Same nodes near the top; today's value lineage for each is 11,100, which is 10,000 × 1.11. They carry `task_b3f01c80` (0.60, share 1.0) and `targ_4e2cc92a` (0.60, share 0.85) through `brain_bf2be9d8`. |
| `task_d5f610e6` | 9 | 236 | 0 | Today (score 10,011): a named stakeholder (2,000) times a deadline multiplier of 5 on its due date (`ranking.md:148-151`, `:216-218`), plus urgency 10. New: no priced target on its chain, so worth 0. As a hard deadline it is first in the cliff lane (I17). |
| `admin_3fcff4d3` | 10 | 220 | 0.1275 | Today (score 9,335): value lineage 1,275 plus stakeholder waiting. New: the same lineage (0.15 × 0.85); a name alone adds nothing (U7). |
| `admin-3e02c20b` (calibration case 4) | 283 | 7 | 1.11 | Today: the priced target's value lands only on the nearest ready leaf below the edge holder (`ranking.md:427`), and this node has a note child. New: as a necessary part of its parent, and its last open one, it carries the full worth (S9, I8). |
| `brain_448bb804` (case 6) | 247 | 13 | 0.95 | Today: blocked, so zero inherited value (`ranking.md:427`). New: blocked work keeps its worth (S4); its blockers carry it too (I5). |
| `task_model_ten_tasks_properly` (case 3) | 188 | 267 | 0 | Not linked to any priced target. The case's premise no longer holds: the blocker it names is done. |
| `trustcon_1c23b18d` (case 4) | 228 | 288 | 0 | Not linked to any priced target. |
| `academic-b738bdc7` (case 2) | 301 | 331 | 0 | Not linked to any priced target. |
| `admin_1fece2e0` (case 1), `admin-59965524` and `aops_polecat_mcp_server_build` (case 5) | — | — | — | Cancelled or done. These calibration cases describe a state that is no longer true, as the brief noted. |

**What this shows.**

- **The new order is a function of what is priced.** With 7 of 26 targets priced, obligations held up today by severity, stakeholder or date alone fall to zero. They reappear only through the cliff lane or once their targets are priced.
- **Pricing comes first.** Pricing the 19 remaining targets is the first input this design needs (S14).

---

## 12. Interface contracts

**Engine outputs, per node.**

| Field | Type | Meaning |
|---|---|---|
| `gain` | float, signed | Σ over positive targets; negative when the work harms a positive target |
| `loss_averted` | float, signed | Σ over negative targets; negative when the work brings a harm about, displayed as "loss caused" |
| `stake` | map target → `δ_t` | share of each priced target at stake |
| `decision_value` | float | settle worth from §6, its own column, never added to `gain` or `loss_averted` |
| `loop_extra` | map target → float | share above the combined loop-free routes |

**Contract on these fields.**

- **No other ranking number is emitted.**
- **Determinism.** The output is a pure function of node states, target worths and edge fields. It is byte-identical across rebuilds from identical input (as `ranking.md:64-69` requires today).

**Explanation contract.** `explain(u)` returns, per priced target:

- the share at stake;
- the strongest route, as a node list with its strength;
- the other routes, up to a cap;
- any loop extra.

This yields the one-sentence form U18 asks for. Example: "`admin-3e02c20b` serves `task_b3f01c80` × 1.00 and `targ_4e2cc92a` × 0.85, both via `proj-f8b942d5` → `brain_bf2be9d8`."

**Inputs.**

| Input | Where it lives | Notes |
|---|---|---|
| Target worth | `worth:` on a target, ±, anchors §5.6 | replaces `standing_weight` |
| Edge fields | §5.1 | — |
| `deadline_class` | on any node with `due` | read by display only |

**Failure.** If a loop is saturated or no fixed point is reached within the iteration cap, nodes feeding the failed loop return `null` for `gain` and `loss_averted` with `flow_status ≠ ok` (`saturated_loop` or `no_convergence`) naming the loop. The linter reports it (`epic_fc1de9ec`). Unaffected components remain ranked as normal (§3.2, lines 156, 158; engine E3).

---

## 13. Acceptance criteria and tests

Each criterion is something an observer can check. The test named beside it is the one the engine build must carry. Its reference version runs in `flow-rule/invariants.py` or `flow-rule/theory.py`.

| # | Observable criterion | Traces to | Test |
|---|---|---|---|
| A1 | Two open prerequisites of a node each show that node's worth, and the node's own worth is the same with or without them | S10, I1, U17 | `inv1` |
| A2 | Replacing a node by two necessary parts leaves every other node's figures unchanged | I2 | `inv2` |
| A3 | A node's share of any one target is never above 1; serving two targets shows both | I3, U11 | `inv3` |
| A4 | Feeding a loop is worth at least as much as with the loop opened, and the run terminates | S8, S15, I4 | `inv4` |
| A5 | A blocked node shows worth, and each open blocker shows at least as much | S4, I5, U3 | `inv5` |
| A6 | The worth table is byte-identical under every display setting | S1, I6 | `inv6` |
| A7 | Every open node with no route to a priced target shows exactly 0 | S7, I7, U8, U9 | `inv7` |
| A8 | The last open part of a node shows that node's full worth | S9, I8, U16 | `inv8` |
| A9 | Adding a priced target and one edge changes figures only upstream of it | I9, U4, U10 | `inv9` |
| A10 | Work that settles an open decision shows EVPI × quantum; doubling every price doubles it; deciding sets it to 0 | I10, U12 | `inv10` |
| A11 | The flow code reads no date field; shifting every date changes no figure | S5, I11 | `inv11` |
| A12 | For every node and target, the share at stake is between the strongest route and the combined routes, or the excess is reported as loop extra | I12, U18 | `inv12` |
| A13 | A full run over the live graph completes, and the comparison table in section 11 regenerates | I13 | `inv13` |
| A14 | Restating a positive target as a negative one moves each protective task's figure from gain to loss averted unchanged | S12, I14, U13 | `inv14` |
| A15 | A task that gains and harms equally shows two non-zero figures | S16, I15 | `inv15` |
| A16 | A loop containing a harms edge, including a pure negative loop at quantum 1, reaches a fixed point | I16 | `inv16` |
| A17 | A hard deadline enters the cliff lane at `effort + buffer` days whatever its worth; fake and soft never do | S13, I17, U14, U15 | `inv17` |
| A18 | Every candidate in section 4 reproduces its table numbers | brief §A | `theory.py` |
| A19 | A loop of open nodes whose every edge is a full-strength helps edge is rejected by name, never ranked | S15 | `theory.py` (loop quantum 1.0: "rejected") |

---

## 14. Behaviour removed

The following leave the maths once this spec is approved and built. Some reappear in display or agent logic; section 9 says which.

- **Ranking machinery:**
  - the sort tuple;
  - `focus_score`;
  - `cost_of_delay`;
  - `severity_gate`;
  - `intent_pressure`;
  - `effective_intent`;
  - `blocking_urgency`;
  - the `affordable_loss` filter (moves to agent logic);
  - node-level `confidence` as a ranking input (edge probability replaces it).
- **Date arithmetic in the maths:**
  - the deadline multiplier;
  - courtesy decay;
  - age staleness;
  - urgency and its slack curve.
- **Graph-shape scores:**
  - `downstream_weight` and `unlock_breadth` as ranking inputs;
  - the conduit pass that zeroes containers and blocked leaves.
- **Stakeholder points:** the stakeholder-waiting bonus.
- **Exported fields:** `cost_of_delay`, `severity_gate` and `queue_rank` in `export_graph` JSON.
- **Edge-weight fields:**
  - the `multiplier` edge field;
  - the seven-word contribution scale as a single number.

Section 9 gives the reason for each.

---

## 15. Questions for Nic

The questions are ordered by consequence. Brief gap numbers are given where the brief raised the question.

1. **Q1. Parent links (gap 3). Settled (Nic, 2026-10-05, points 19–20):** `part_of` quantum 0 (filing label; grouping/progress read it) and prototype nodes as unpriced hubs (instance→prototype 1.0, prototype serves priced targets; prototype stays unpriced and passes worth through). There is no tree, and a task may relate to several projects; not every task is necessary to the project it belongs to.
2. **Q2. Default quantum (gap 2). Settled (Nic, 2026-10-04, point 7):** Default quantum 0. Unvalued links carry a quantum of zero, because many ideas and incoming opportunities are worth nothing. A recurring job densifies the graph fifty tasks at a time.
3. **Q3. Ordering. Settled (Nic, 2026-10-05, point 17; Ida):** Ordering = plain sum of gain and loss averted (`total = gain + loss_averted`), both figures shown. Good and bad in equal measure cancel out for ordering; both figures stay shown separately (point 16).
4. **Q4. Decision options. Settled (Nic/Ida):** Yes. For the decision rule, an option's gain and loss averted are summed to compare options (same call as Q3).
5. **Q17. Targets serving targets. Settled (Nic, 2026-10-05, point 18):** Worth passes between priced targets and adds, with the pricing rule "a target is priced for its worth in itself, never for what it feeds, else left unpriced and passes through". The pricing sitting asks this for every target-to-target link.
6. **Q5. Hard blocks (gap 4).** Is a hard block its own label (`needs`), or a `serves` link at quantum 1.0 with the ready filter reading something else?
7. **Q6. Quantum words.** Are the five quantum words and numbers in §5.4 right?
8. **Q7. Worth anchors (gap 6).** Do the five standing-weight anchors stay, mirrored for losses (§5.6)?
9. **Q20. Harmful work in the baseline. Settled:** Confirmed. Baseline counterfactual semantics prevents open harms from deflating or zeroing unrelated work.
10. **Q8. Decisions modelled as parents. Settled (Ida/Nic):** Yes. Migration relabels a decision's children as `alternative`, each inheriting the decision's edges (section 6).
11. **Q15. Loop strength cap.** Full-strength loops are rejected (§3.2). Should the linter also cap loop strength lower, say 0.9, given how strongly a near-1 loop amplifies a weak feeder (§4.2)?
12. **Q16. Real reinforcing loops (gap 9).** Are the two loops on today's graph real reinforcement, or modelling artefacts? Calibration against real loops is low priority (S15).
13. **Q18. Unclassed dates.** Is a `due` with no deadline class treated as `fake`, as proposed?
14. **Q19. Must-not-miss floor.** Is the floor the cliff lane plus a list you keep, as proposed?
15. **Q9. Cliff buffer.** How many days before `due`, beyond effort, does a hard deadline enter the cliff lane? The reference uses 7.
16. **Q10. Priority band (gap 7).** Does the P0–P4 `intent` field survive, and for what: a display pin, a `list_tasks` filter, or nothing? This spec takes it out of the maths (§9), but does not decide whether the field itself goes.
17. **Q11. Fun and excitement (gap 5).** Is it a priced target, or a separate display signal?
18. **Q12. Effort (gap 8).** Does effort stay out of the maths, as here, used only for benefit per effort in display?
19. **Q13. Ripeness (gap 10).** Who tells the graph an opportunity is no longer ripe?
20. **Q14. Soft to hard (gap 11).** By what rule does a soft deadline become hard? Here it is a deliberate agent step, specified in the skills spec.
21. **Q21. Label set.** §5.2 proposes six labels (`serves`, `needs`, `part_of`, `supports`, `alternative`, `settles`). Is this the right canonical set, or should any label be added, renamed, or omitted?
22. **Q22. Migration values.** §8 defaults unvalued migrated edges: `part_of` → 0.0 (Q1 settled), `needs` → 1.0, `supports` → 0.3, unvalued `serves` → 0.0 (Q2 settled). Does Nic confirm these migration defaults?
23. **Q23. Wikilinks excluded from flow.** §8 excludes wikilinks from the flow model (1,876 wikilinks ignored). If treated as weak flow edges at 0.05, 604 nodes carry worth and 932 nodes sit on loops. Should wikilinks remain excluded from the flow model?
24. **Q24. Retiring the severity ladder.** §9 drops the 0–4 severity scale from ranking maths. Does Nic approve retiring severity from ranking, or should it be retained as a display badge or triage tag?
25. **Q25. Retiring stakeholder waiting points.** §9 drops the flat/ramped stakeholder bonus. Should stakeholder waiting be handled purely in agent triage and target pricing, or surfaced as a display sort/filter?
26. **Q26. Cliff trigger condition.** §7 defines the cliff lane as `days_left <= effort_days + buffer`. Is `days_left <= effort_days + buffer` the right cliff trigger, or should lead time or chain slack be incorporated?
27. **Q27. Probability scale words.** §5.5 adopts the 7 Renooij–Witteman words (`certain`: 1.0, `probable`: 0.85, `expected`: 0.75, `fifty-fifty`: 0.50, `uncertain`: 0.25, `improbable`: 0.15, `impossible`: 0.00). Does Nic approve this scale?
28. **Q28. Negative targets scale.** §5.6 proposes mirroring the 5 standing-weight anchors for losses (`Catastrophic` −1.0, `Severe` −0.6, `Substantial` −0.35, `Moderate` −0.15, `Minor` −0.05). Does Nic approve this negative worth scale?
29. **Q29. Blast radius (`downstream_weight`) removal.** §9 drops `downstream_weight`. The brief listed this under "Leaning, not settled". Does Nic confirm dropping downstream blast radius in favor of worth passing to unblockers?
30. **Q30. Retiring `supersedes` from the flow.** §8 excludes `supersedes` edges from the flow model. Does Nic agree that `supersedes` belongs purely to agent lifecycle logic?
31. **Q31. Actionable and claimable types in ready filter.** `ranking.md:499-504` restricted ready tasks to `CLAIMABLE_TYPES` (`["task"]`) and actionable types (`["task", "learn", "pr"]`) with an acceptance criteria gate on `inbox` status. Should display preserve these exact filter rules?

## 16. Files

| Path | What |
|---|---|
| `specs/flow-rule.md` | this spec |
| `specs/flow-rule/flow.py` | reference calculator: the rule, decision rule, routes, live adapter |
| `specs/flow-rule/display.py` | reference display rules outside the maths |
| `specs/flow-rule/theory.py` | section 4 candidates on the worked example |
| `specs/flow-rule/invariants.py` | section 10 and 11 numbers |
| `specs/flow-rule/extract_fixture.py` | builds the fixture from a private export and a private key |
| `specs/flow-rule/fixtures/live-2026-10-05.json` | live graph without titles; uncited ids replaced by a keyed hash |

---

## References

- Bauschke, H. H., & Combettes, P. L. (2017). *Convex Analysis and Monotone Operator Theory in Hilbert Spaces* (2nd ed.), Thm 5.15. Springer. https://doi.org/10.1007/978-3-319-48311-5
- Birnbaum, Z. W. (1969). On the importance of different components in a multicomponent system. In P. R. Krishnaiah (Ed.), *Multivariate Analysis II* (pp. 581–592). Academic Press. Technical-report version: https://doi.org/10.21236/AD0670563. The measure assumes independent components.
- Gouzé, J.-L. (1998). Positive and negative circuits in dynamical systems. *Journal of Biological Systems*, 6(1), 11–15. https://doi.org/10.1142/S0218339098000054
- Howard, R. A. (1966). Information value theory. *IEEE Transactions on Systems Science and Cybernetics*, 2(1), 22–26. https://doi.org/10.1109/TSSC.1966.300074
- Kahneman, D., & Tversky, A. (1979). Prospect theory: An analysis of decision under risk. *Econometrica*, 47(2), 263–291. https://doi.org/10.2307/1914185
- Katz, L. (1953). A new status index derived from sociometric analysis. *Psychometrika*, 18(1), 39–43. https://doi.org/10.1007/BF02289026
- Leontief, W. W. (1936). Quantitative input and output relations in the economic systems of the United States. *Review of Economics and Statistics*, 18(3), 105–125. https://doi.org/10.2307/1927837
- Krasnosel'skii, M. A. (1955). Two remarks on the method of successive approximations. *Uspekhi Matematicheskikh Nauk*, 10(1), 123–127.
- Mann, W. R. (1953). Mean value methods in iteration. *Proceedings of the American Mathematical Society*, 4(3), 506–510. https://doi.org/10.1090/S0002-9939-1953-0054846-3
- Miller, R. E., & Blair, P. D. (2009). *Input-Output Analysis* (2nd ed.). Cambridge University Press. https://doi.org/10.1017/CBO9780511626982
- Mohri, M. (2002). Semiring frameworks and algorithms for shortest-distance problems. *Journal of Automata, Languages and Combinatorics*, 7(3), 321–350. https://cs.nyu.edu/~mohri/pub/jalc.pdf
- Pearl, J. (1988). *Probabilistic Reasoning in Intelligent Systems*, ch. 4 (noisy-OR, assuming independent inhibitors and no leak). Morgan Kaufmann. https://doi.org/10.1016/B978-0-08-051489-5.50010-2
- Pearl, J. (2009). *Causality* (2nd ed.), ch. 9. Cambridge University Press. https://doi.org/10.1017/CBO9780511803161
- Reason, J. (1990). *Human Error*. Cambridge University Press. https://doi.org/10.1017/CBO9781139062367
- Reason, J. (2000). Human error: models and management. *BMJ*, 320(7237), 768–770. https://doi.org/10.1136/bmj.320.7237.768
- Shapley, L. S. (1953). A value for n-person games. In *Contributions to the Theory of Games II* (pp. 307–317). Princeton University Press. https://doi.org/10.1515/9781400881970-018
- Snoussi, E. H. (1998). Necessary conditions for multistationarity and stable periodicity. *Journal of Biological Systems*, 6(1), 3–9. https://doi.org/10.1142/S0218339098000042
- Tarski, A. (1955). A lattice-theoretical fixpoint theorem and its applications. *Pacific Journal of Mathematics*, 5(2), 285–309. https://doi.org/10.2140/pjm.1955.5.285
- Thomas, R. (1981). On the relation between the logical structure of systems and their ability to generate multiple steady states or sustained oscillations. In *Numerical Methods in the Study of Critical Phenomena* (Springer Series in Synergetics, vol. 9, pp. 180–193). https://doi.org/10.1007/978-3-642-81703-8_24
- Wright, S. (1934). The method of path coefficients. *Annals of Mathematical Statistics*, 5(3), 161–215. https://doi.org/10.1214/aoms/1177732676
