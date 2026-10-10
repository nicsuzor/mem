---
id: pkb-rules
title: "PKB Rules: what the PKB holds and how it is written, linked, maintained and ranked"
type: spec
status: approved
created: 2026-10-04
tags:
  - pkb
  - rules
  - doctrine
  - graph-hygiene
  - prioritisation
  - spec
---

# PKB Rules

This document is the one home for the rules that govern the personal knowledge base (PKB): what it may contain, how a node is written and linked, how the graph is kept clean, how agents read and write it, and how importance is assigned. The PKB holds facts only. Nic: "rules don't belong in the pkb. they're not facts." and "the rules should go in the spec document for the pkb".

Companion specifications hold the mechanisms these rules sit on, and this document does not restate them:

| Mechanism | Where |
| --- | --- |
| Ranking engine: `focus_tuple`, `cost_of_delay`, `urgency`, `effective_intent`, the severity ladder and verbal weight scale as computed | `specs/ranking.md` |
| Node types and the two-tier target/work taxonomy | `specs/pkb-type-taxonomy.md`, `references/TAXONOMY.md` |
| Status values, transitions and the task tool surface | `references/TAXONOMY.md` §Status Values and Transitions, `specs/work-management.md` |
| Write-path mechanics (in-place patch, deferred re-embed, compare-and-swap) | `specs/pkb-server-spec.md`, `.agent/CORE.md` |

Where a PKB note and this repository disagree, the repository wins. A PKB note may cite a section here; it never restates one.

---

## 1. What the PKB holds

### 1.1. Current facts only

The PKB holds what is true now: one canonical document per topic, densely linked, findable.

It does not hold, however the material got there:

- **logs**: dated entries, progress stacks, resume sections, execution traces;
- **event records**: what happened, when, in what order;
- **narration of work done**: "we tried X, then Y, then discovered Z". The finding is durable; the story of arriving at it is not;
- **rules**: standing doctrine about how the PKB or its agents work. Rules live in this specification (§1.4).

That material belongs in audit logs, session transcripts and git history. Episodic material in the PKB is misfiled, not merely untidy: it displaces what a reader came for.

A date is durable when it is part of the fact: a deadline, a decision's provenance, a measurement's vintage. What is barred is the dated *entry*, not the dated *fact*. Test: does a reader who never cared what happened still need the date to use the fact?

### 1.2. The log/knowledge boundary

The log tier records what happened. The knowledge tier records what is true.

- A log entry is valid because it was observed at time T. It is never revised.
- A knowledge node is valid because it is currently correct. It is revised the moment it stops being.

Test: anything whose correctness is a function of when it was written belongs in the log tier.

The PKB may hold pointers into the log and claims derived from it, never copies of it. What crosses the boundary is a claim with a citation, and it crosses on the second ask, not on a schedule: a scheduled promoter manufactures knowledge nobody needed, while a second ask is evidence the claim is load-bearing.

The PKB's value rests on one invariant: every node is currently true. Admitting historically-true nodes breaks it for every node, because a search hit can no longer be assumed current without checking its date.

Session prompts are log tier permanently. What Nic typed at a given moment is true-as-of-then and never becomes false. Prompts are indexed, cited and queried where they live; they are never PKB nodes. Agents do not record or relay the user's words verbatim into the PKB; they make sense of asks in context, link them to prior context, recompose them into clear logical structures, and cite message ids as pointers.

### 1.3. The flat-file test

Apply before creating any new store, ledger, digest, export or generated `.md` artifact. A generated flat file is legitimate only when it is a rendered view that is:

1. **bounded by construction**: its size is capped by its definition, not by how long the job has run;
2. **stamped with generation time**, so a reader sees its staleness without checking anything else;
3. **read by something that would visibly break** if it went stale. A human noticing later does not count.

If the artifact is the only copy of its content, it is appended, never regenerated: a regenerating writer over a sole copy is a data-loss mechanism. If it is regenerated, it is a view, judged solely by whether anything reads it; an unread view is cost with no reader.

Both §1.2 and §1.3 prevent a shadow corpus: flat files carrying stale doctrine that agents load as current.

### 1.4. Doctrine has one home and is never copied

A rule has exactly one home. A copy cannot be kept in step with its source, so every copy is drift with a delay on it, and a reader cannot tell which of two texts is current. Point at the source instead.

| Doctrine | Home |
| --- | --- |
| Rules about the PKB itself | This specification |
| What an error means, how it is diagnosed, what is owed after one | the academicOps `/learn` skill |
| An agent's behaviour | that agent's definition in the academicOps plugins (`plugins/ida/agents/<agent>.md`); what is personal to Nic lives only in the local `.agents/CORE.md` and `.agents/SOUL.md` of the agent's own directory |
| Rules binding every agent | the always-on axioms (§8.1) |

None is duplicated into the PKB, `CLAUDE.md` or another instruction file. The PKB holds facts, findings, decisions and state, never a restatement of the doctrine that governs how they are handled.

A defect in anything that ships via the plugins (skill, agent definition, hook) is fixed in the canonical source in academicOps and lands through a gated PR from a clean per-task worktree, so it reaches every host. An edit to an installed or symlinked copy is never a local fix: it mutates the shared checkout or is overwritten on the next install. The only legitimate local edit is a machine-local preference in a file that is not a checkout of the repository.

---

## 2. Writing a node

### 2.1. One correct current version, rewritten in place

**A fact that is no longer true is deleted, not annotated.** This binds every write: capture, task updates and knowledge notes alike.

Barred outright:

- "Superseded" / "historical" / "do not build from this" sections kept below the current version. Marking old text and leaving it in place is not a rewrite.
- Changelogs and dated append-only layers ("REVISED <date>", "Status: <date> …", progress stacks, resume sections).
- Corrections that quote what the node used to say ("the earlier body claimed X; that was wrong"). Delete X.
- Retained losing options with their refutations, where the option is not a live temptation.

**The one carve-out is narrow.** Where a retired premise would otherwise be re-derived by the next reader, keep one line of warning ("do not re-derive this from joint count") adjacent to the claim it retires, and delete the derivation. Test: does a reader who never knew the old version still need this sentence to avoid walking back into it?

Corrections go where the claim is, never appended below it. A claim at the top with its retraction at the bottom sits at two depths in one document; readers and chunked retrieval get the stale half.

Deleting is safe because git, session transcripts and audit logs hold the history, and all three are better at history than the PKB. Prior wording is never kept "for the record". Two notes on one topic are a defect even when both are true: they are reconciled into one document that states the resolution. "Densify" and "prune" are one operation: raising the fact-per-byte ratio of what survives.

Capture at the resolution you have. A node is written with what is known now and refined in place as more is learned; incompleteness is no reason to withhold it.

### 2.2. Contradictions and the open-conflict queue

When two sources disagree, the note states the resolved fact, not the disagreement; the losing version is deleted rather than recorded as the loser.

A contradiction no checkable source settles is asked of Nic via the open-conflict queue: one `## Open conflict` block at the top of the canonical note holds both claims and the evidence that would settle them; the note is tagged `open-conflict`; `/gather` works through the whole queue over time; the block is removed when settled. Filing both claims anywhere else and moving on is not a resolution.

### 2.3. A task is a unit of work, not a store of knowledge

A task body holds the work record: goal, deliverable, scope, checklist, pointers. It is live instructions: trimmed when instructions change, never corrected by appending below stale text. Durable facts discovered while doing the work are extracted into knowledge notes. A task body that has accumulated unconsolidated prose observations is the defect; anything retrieval does about it is a patch on the symptom. A task whose durable content has been extracted can be deleted; deletion ends the extraction sequence and never substitutes for it.

**The extraction duty binds every writer at every pass.** Closure, a fact being needed a second time, and a consolidation sweep are all triggers, and map-of-content custodianship binds the same duty at map scope; none of these is *the* owner. Downstream layers exist to correct misses, not to excuse them: a later sweep is no reason to leave a body unconsolidated now, and an earlier layer's miss is no reason for the next pass to walk past it. A writer who sees drift repairs it in the pass that found it. Retrieval initiation is a separate failure (a well-formed note still goes unread when nothing makes the agent look); both are real, and neither discharges the other.

### 2.4. Reviews are PKB nodes, written as advice

A completed review Nic commissioned (a colleague's draft, a grant application, a PR, a plan) is durable knowledge and is written into the PKB as it is produced. Scratch directories hold session byproducts only; a scratch copy may exist alongside the node, never instead of it. A brief that commissions a review names the PKB node as the artifact ("write the review as a PKB node, wired to `<ids>`"), not a file path; a brief that names no node, or forbids PKB writes, produces a review that lands nowhere.

An agent's review verdict is advice, never a ruling, and is recorded so it cannot later be read as one:

1. Name the author and standing with the verdict ("three agents reviewing blind reached REJECT", not "REJECT").
2. Never state a verdict as an outcome: "recommends closing", never "rejected, not merged".
3. Keep each finding separable from the disposition, so it survives the verdict being overturned.
4. The disposition is unknown until the owner acts, and the record says who acted. Where the artifact carries the truth (a PR's `mergedBy`, `state`), cite it.
5. Corrections go in place (§2.1).
6. A merge overtakes a hold. When a do-not-merge verdict sits on a node whose PR has since merged, delete the verdict, re-check each blocker it named against the merged base, and return whatever the merge did not settle to `inbox` as remaining work.

The tell: a downstream task whose premise is "this was rejected and shipped anyway". Treat it as a defect in the record until the artifact is checked.

### 2.5. Node ids

Resolution registers lookup keys in strict tiers: claimed identity (frontmatter `id:`, explicit `permalink:`) outranks derived keys (filename stem, id prefix), which outrank the title. Matching is exact at every tier; same-tier collisions break deterministically and are logged as ambiguous. Collision is visible, not impossible, so:

- Never mint a bare-word id that an existing file's name already produces. Where an existing id family needs a root, promote a member; do not mint a shorter id above it. Auto-generated `<slug>-<hex>` ids are safe.
- Write a whole id in `parent:`, never a shorthand the resolver is expected to widen.
- Never duplicate an explicit `permalink:` across nodes.
- A filename stem is frozen at creation and is never evidence of a node's current title; frontmatter `id:` and `title:` are authoritative.

### 2.6. Linking

Every frontmatter edge field is parsed with no node-type gate; a `note`, `memory` or `knowledge` node uses `parent`, `depends_on` and `contributes_to` exactly as a `task` does. Which field means what is specified in `references/TAXONOMY.md` §Edge Semantics. The authoring rules:

- **Actionable nodes** (`epic`, `task`, `learn`, `pr`) carry a `parent`. Missing or dangling `parent` is an orphan, whatever other edges exist.
- **Goals and targets are never parents.** Work attaches to them with `contributes_to`. A goal or target carrying a `parent:` of its own is a filing choice, not a defect.
- **`parent` says where a node lives; `contributes_to` says what it is for.** Both belong on a task that serves a target.
- **Load-bearing relations go in frontmatter**: sequencing (`depends_on`, `soft_depends_on`), strategic attribution (`contributes_to`), replacement (`supersedes`).
- **Never write `superseded_by`.** It is the computed reverse of `supersedes`; record replacement on the superseding node.
- **Context goes in body wikilinks.** A wikilink is a real graph edge on every node type and the only "see also" mechanism; one is enough to keep a reference-tier node off the orphan list. Cite prior nodes with wikilinks instead of re-explaining them.
- **Refer to PKB documents by id or wikilink, never by filesystem path.** Where a path is unavoidable, express it relative to `$ACA_DATA`; never write a machine-specific absolute path.
- **Never author inert relations**: a `related:` field, a `goals:` field, or a `## Related` / `## See also` / `## References` heading. None builds an edge.
- **`contributes_to` points at a goal or target.** The engine does not enforce the destination type; the scoring model assumes it.
- **Wire at the highest node where the answer is the same for everything beneath it.** If every member of a group would take the same `stated_weight` and a near-identical justification, they are one obligation: put them under a shared container and wire the container once. Fungible members carry no per-instance signal, so per-leaf edges invent a discrimination that does not exist. Differentiated members, each with its own contribution and ship risk, carry their own edges. When in doubt, prefer the container: one edge is cheaper to revise than N.
- **A deliverable that is one instance of a recurring class** (a release, a report, a publication, a dashboard) wires via `contributes_to` to a class-level production target for that deliverable type, not directly to a goal (over-aggregates), a project epic (couples the output to one application) or a vague container (no severity to propagate). The class-level target contributes upward. Applications of this convention to a specific domain belong in domain notes, not here.

### 2.7. Importing material

Material brought in from outside (documents, diagrams, old notes, transcripts) is valued mostly for the connections it reveals between existing nodes, not for new nodes. Ask what a source connects, not whether its content is new.

- Matching an existing concept is no reason to discard material. Work the relational context it adds into the existing node's prose where the link is needed (§2.1), never as an appended block.
- Co-occurrence across projects is the highest-value capture. When a source names several existing projects together, record the relation even if each project is well documented alone.
- A recurring strategic tension (prototyping against shipping, research against operations) is knowledge in its own right: one note, wikilinked from every project it affects.

---

## 3. Graph hygiene and placement

### 3.1. The durability bar

When a task settles, its durable knowledge moves to a canonical note and the task is deleted. Ask four questions, in order, of every proposition in a settled task:

1. **Does it persist beyond the task?** Checklist state, branch names, PR links and debug flags stop mattering once the criteria are met. Drop them.
2. **Is it narration?** Retry logs, test output, diffs and commit SHAs are history. Only the surviving lesson passes forward.
3. **Is it one of the five durable kinds?** Architectural invariant or system constraint; empirical finding with a verified root cause; standing decision; domain concept or taxonomy; living procedure. Anything else is commentary.
4. **Is it already canonical?** If an existing note holds it as accurately, do not add a copy.

Only a proposition passing all four is extracted. A task that yields nothing is deleted without a knowledge write. A standing rule is not extracted into the PKB at all: it goes to its home under §1.4.

### 3.2. Destination and the pre-deletion gate

Extracted knowledge goes into the existing canonical topic note, synthesised into its prose rather than appended as a dated entry; otherwise into a new note parented to the relevant index or project. The destination id resolves before any write.

Before deleting the source, the destination write has succeeded and every link other nodes hold to the source is rerouted. If either step fails, halt and leave the source intact.

### 3.3. The consolidation guard

Merging overlapping nodes has two failure modes: picking a winner and silently losing facts only the loser held, and concatenating both bodies.

Before merging, list each node's atomic claims and partition them: shared, contradictory, only in A, only in B. Contradictions are resolved against the source of record (§2.2). Every single-source fact gets an explicit disposition: folded into the synthesised body or excluded with a stated reason. A single-source fact with no disposition blocks the merge. The merged body is one minimal current statement of goal, scope, state and checklist.

Duplicate detection returns candidates, not verdicts. A `find_duplicates` score never gates a merge, at any threshold, in either direction: title-pattern collisions score near 1.0 without content equivalence, and genuine pairs often score low. The merge call is made by reading member titles and, where titles are heterogeneous or pattern-shaped (date or meeting prefixes, one project prefix on different artefacts, section numbering across sibling epics), bodies. When score and titles disagree, trust the titles.

Invariants:

- Deletion is outright: no tombstones, no `status: archived`, no archive folders.
- A consolidation that produces concatenated bodies has failed.
- Hygiene runs in every consolidation pass and whenever a task is released as done; it does not wait to be asked for.

### 3.4. Clean first; git is the recovery

Agents delete, merge and consolidate PKB nodes on the ratified rules in this specification alone. Git is the recovery path; there is no per-batch sign-off before a deletion or merge. Nic: "nah, git is fine, we trust git recovery always. clean first, ask forgiveness not permission always unless it's going out to external stakeholders". The sole exception is anything going out to external stakeholders (a message, submission, publication, calendar invitation, or any change visible outside Nic's own systems), which still waits for his sign-off.

### 3.5. Intake: one epic per body of related work

Nic, 2026-09-11: "consolidate related tasks under a single epic where possible, even if they get added separately. Ideally we would hand sara an entire epic, with multiple tasks that can be executed at roughly the same time."

1. **Search for the epic first.** An ask that overlaps an existing task is adopted into it, not filed beside it.
2. **Same pass, same task id.** Work one executor would do in one sitting on the same surface becomes subtasks of one task.
3. **Different pass, separate children.** Work on different surfaces or needing different executors becomes separate children of the same epic, so the epic can be dispatched whole.
4. **Project roots are not epics.** Related children under a root are grouped into a named epic beneath it.
5. **Edges follow the grouping.** Sibling `depends_on` edges under one task collapse into subtask ordering; cross-epic dependencies stay edges.

### 3.6. Continuous curation

Nic, 2026-09-18, as a standing mandate for every session:

- Related tasks never sit flat under a parent without internal structure. Intermediate epics compress complexity and distribute downstream weight; avoid flat links from a goal to hundreds of leaves.
- Whenever a task is added, touched or observed, neighbouring tasks are checked for overlap.
- Curation actions: consolidate overlapping tasks into one task with scoped subtasks; cluster under a parent that cleanly bounds the work; deduplicate; sequence with `depends_on`; weight `contributes_to` edges to reflect real leverage.
- When capture finds an existing task for the same thing, the new context merges into the existing node rather than creating a twin. Repeatedly hitting the same obstacle is evidence for a higher intent band.

### 3.7. One node per obligation; tags drive nothing

- **One node per obligation.** Accepting or declining an invitation, writing or refusing a reference, answering or not answering an inquiry is the same obligation as doing it, and the task that does it records the answer. A "decide whether" node beside a "do it" node is merged into the do-it task. A genuine fork, two mutually exclusive courses each of which is real work, is modelled as mutually blocking option nodes, not as a decision task.
- **Tags carry no work and drive no surface.** Nothing in the engine, a daily note, a brief or a dispatch surface reads a tag to decide what Nic is shown or what an agent does. A real decision is a task on the graph, parented and edged like any other.

### 3.8. Locating work

- Locate parent epics, duplicates and existing tasks with hybrid search (`search`, `task_search`) first. Do not traverse the graph node by node through parents, children and dependency trees to explore hierarchy; it burns tool calls and context.
- A pre-creation duplicate search runs across all types. Filtering to `type=epic` misses overlapping `task` and `learn` nodes.
- Work that exists only as a GitHub issue, PR, email or ad-hoc note is invisible to the queue: it gets a PKB task, linked to the external artifact.
- **A fixable defect is a node the moment it is named.** When a brief, task body or report names a specific, fixable defect, the defect becomes its own task node as the sentence is written, and the sentence cites the node's id. A bullet in a brief cannot be claimed, carries no status and drifts unreconciled; a defect narrated only in chat is gone at session end.
- Relationships, dependencies and blockers discovered in a session are written to the graph as edges or wikilinks. Conversation is ephemeral; the graph is working memory.

### 3.9. Minting a node

- **Ask, or probe.** A question a person can answer in a sentence is asked in conversation and never becomes a task. An unknown that needs investigation becomes an explicit probe (`type: learn`) with observable acceptance criteria, and the work that waits on its answer `depends_on` it.
- **Find out cheaply before committing.** Where paths diverge on an unknown, a low-cost discriminating probe comes before heavy work on any branch.
- **Place work where its scope is shared.** Place a node under the highest container whose whole scope it serves, not under the nearest or current one. Work that affects several projects is not buried in one project's leaf. It reaches the goal or target it serves through `contributes_to` (§2.6), never by being parented to it.
- **No task flooding.** Never batch-generate unverified nodes. Each node minted is checked for duplicates first (§3.8).
- **A review is its own node only when a separate evaluator must discharge it in a different session.** Otherwise it is a checklist line on the task it reviews. An empty placeholder review node is deleted.

---

## 4. Write and read discipline

### 4.1. Concurrent writers and compare-and-swap

Fan out as many PKB writers as the work needs, on any nodes, whether or not their node sets overlap. There is no store-wide write lane and no same-node carve-out to plan around.

Compare-and-swap is opt-in and is the writer's job: pass `expected_modified` (the `modified` value from your own last read) on `append`, `update_body` and `edit_body` whenever another writer may hold the node. On `stale_write`, re-read and merge before retrying; a blind retry re-derives the conflict or clobbers a third writer.

### 4.2. Trust the write response

A success response from a PKB write means the write landed. Do not read the node back to confirm it. A write that reports success without landing is a server defect: file it against nicsuzor/mem so the server checks it mechanically, rather than spending agent reads on it.

- **A search miss is not evidence of loss** (§4.3): a fresh write is not yet searchable.
- **A failure response is not evidence the write did not land.** Before retrying, read the target and compare `last_modified` with the value held before the call: unmoved means nothing landed; moved means the retry must be re-planned. When a surgical write fails, do not fall back to a whole-document rewrite as the "safe" option; it has a larger clobber window.
- **A self-reported clobber is not evidence in either direction.** A `git diff` of the brain repository across the suspected revisions settles whether content was lost. Attribution of the second writer comes from the dispatcher's record of what was running, or from nothing.
- After a heavy parallel wave, an id-vs-filename scan detects whole-node overwrites (frontmatter `id:` not matching the filename stem). It cannot detect two writers on one file; §4.1 is for that.

### 4.3. Index lag, orphans and reindexing

- A write is not findable by search when it lands: embedding is deferred (see `.agent/CORE.md` §Deferred ONNX re-embed), and a retitled node surfaces under its old title until re-embedded. Do not gate a worker launch on a search round trip confirming a freshly created task; workers read the node by id.
- A search hit is not proof a node exists: the vector index can serve a deleted document. Resolve every id through `get_document` or `get_task` before citing it.
- A delete is not finished until `repair_index_orphans` reports zero orphans (dry run first). `refresh_graph` does not touch the vector index.
- A by-id "not found" is identical for a deleted, never-existing or invented id. Before reporting a record missing, probe a deliberately fabricated id as a control.
- **Never run a forced full re-embed of the store without Nic.** Incremental reindexing of named files is fine. A banner or counter recommending a forced rebuild is not authority to run one.

### 4.4. Closing a parent

Closing a parent with open descendants is refused, and the check is type-blind: an open `review`, `note` or `knowledge` node parented under a task blocks it. On that refusal, re-parent or re-type the blocking child. Use `recursive=true` only when every descendant is genuinely finished work you mean to close: the cascade writes `status: done` with no evidence, and the evidence requirement guards only the parent. A refusal that names a child already deleted is index lag, not a live child: confirm the child is gone by id (§4.3) before closing with `recursive=true`, and do not re-create it.

### 4.5. A PKB note versus the primary source

Neither "trust the note" nor "go to the primary source" is the default. Sort the claim first.

- **Claims about intent** (what was asked for, decided, in or out of scope) do not decay. The record is where they are checked; re-deriving them from binaries, transcripts or code spends an investigation on something with no primary source.
- **Claims about the world** (a path, line number, schema, config key, version-pinned behaviour, and every negative claim) decay silently. Re-confirm them before acting on them.

Then prefer the cheapest observation that could refute the claim, and check only load-bearing claims. On an explicit user correction, stop re-deriving and verify at source on the first push. Before commissioning primary research, open the node a search hit points at. When a retrieved note contradicts the user or a newer note verified this session, go to the primary source and write the result back.

A knowledge note that explains a system behaviour is filed only after confirming the input that produced the behaviour is the one specified (§8.2); a note built on an invented input manufactures false knowledge. A stored open question was true when written; confirm it is still open against the source of record before spending Nic's attention on it.

### 4.6. Using the write tools

- **Write through the PKB tools, never by editing node files.** A direct file edit bypasses compare-and-swap, the path lint and the per-write commit. The exception is a field no write tool exposes.
- **Pass list fields (`depends_on`, `tags`, …) as native arrays.** A list passed as a string is stored as a string. An update replaces a list field whole: read it, extend it, write the full list.
- **A changed `title` or `body_chars_before` in an `update_body` result is a clobber alarm**: another writer has been there since your read. Re-read and merge (§4.1).

---

## 5. Task lifecycle

Status values and transitions are specified in `references/TAXONOMY.md`. The rules for using them:

- **Task volume is not a problem.** Hundreds of open tasks is the healthy state; never flag a high count or propose artificial cleanup. There is no task bankruptcy.
- **Never schedule by calendar day.** Decompose work into tasks with typed dependency edges. `due` is reserved for real external or contractual deadlines and is never fabricated to express importance (§6.1).
- **Age is not staleness.** Age alone never justifies cancelling, archiving or deprioritising. The only valid cancellation reason is irrelevance: the opportunity closed, the request was withdrawn, the context was eliminated.
- **Blocked tasks surface through their blockers.** An overdue blocked task with no linked blocker is an integrity defect. A dependency on other work is a `depends_on` edge, not a hand-set `status: blocked`.
- **State lives in fields, never only in prose.** Every reader of the graph decides done, blocked or ready from the stored `status` and the dependency edges, never from body text. Set terminal states with `release_task`, which carries the evidence or the reason. Body prose may explain a state; it never substitutes for one.
- **Never leave a status the body denies.** Pick the value that is true; if none fits, say so on the node rather than choosing the least-wrong value.
- **`queued` is Nic's gate.** Only Nic promotes `ready` to `queued`; workers pull only from `queued`.
- **Workers write `done`.** A worker with PKB access marks its task `done` after `/pull`, with completion evidence; asserting that tests ran suffices for the claim. `merge_ready` is not a status in use. The reconcile check that follows is specified in the academicOps `reconcile` skill (§8.7).
- **Work done outside agent sessions** is reconciled into the graph so status does not drift.

### 5.1. Choosing a closing status

- **`done` asserts delivery.** A task is `done` when its deliverable verifiably exists where it belongs, judged in its delivery location, never by a run's duration, byte count or self-report. A run that did the work but left it where the deliverable does not live (an unpushed branch, a container workspace) is not `done`. The release cites where the deliverable lives.
- **`done` is refused while the task's own record shows open work**: a pending checklist item, an unmet acceptance criterion, or a body describing remaining work.
- **`review` means a person owes a decision before anything can move.** If nobody does, the task closes. A `review` task is closed by the decision, never by a PR match. Three shapes are not `review`:
  - Physically complete work whose durability is only being watched is `done`, reopened if it fails. Nic, 2026-08-21: "just close the task now, i'll reopen if it fails."
  - Work with a default available: take the default, record it on the node as chosen, close. Only one-way doors and value judgements are human blocks.
  - Work whose remainder is genuinely separate: `done` plus a follow-up, or `partial`. Never hold the finished part open to carry the rest.
- **`partial` always has a live follow-up child** carrying the remainder; a `partial` with none is a defect.
- **An attempt that produced nothing shippable is not `partial`.** The task returns to `inbox`. What was learned is worked into its body as instruction (§2.3); the attempt itself is not logged there.
- **Discharged work awaiting someone else's confirmation is split, not held open.** When the owner has done everything they can and the task waits only on an outcome outside their control (an approval, a reimbursement, an acceptance), model three nodes: the parent renamed to the objective; the action, `done` with evidence; and an open confirm node holding what the other party owes. The action names, by wikilink, the confirmation that could reopen it; the contingency is not a `depends_on` edge, because the action already happened. A negative confirmation reopens the action at its original stakes.
- **The split applies only when nothing executable remains.** If a chase, escalation or resubmission is still available, the work is not discharged. A confirm node gets no fabricated `due` (§6.3) and no `stakeholder`, a field that names someone waiting on the work, not someone the work waits on.
- Where nobody owes anything and the work is only being watched, close it and record on the closed node what would reopen it.

---

## 6. Prioritisation

This section holds the rules agents apply when they evaluate stakes, targets and edge weights. The engine that consumes the values is `specs/ranking.md`.

### 6.1. The signals are orthogonal

Collapsing intent, stakes, likelihood and deadlines into one "importance" knob breaks deadline trust and inverts triage. Each signal has its own field:

| Axis | Field | Meaning | Rule |
| --- | --- | --- | --- |
| Intent | `intent` (0–4; lower is sooner) | Nic's focus allocation | §6.3 |
| Severity | `severity` (integer, targets only) | Worst realistic magnitude if the target fails | §6.2 |
| Contribution | `contributes_to` edge: `stated_weight` and an optional certainty discount | How much of the target this work delivers, and how sure that is | §6.4 |
| Clock | `due` (ISO date) | A real external or contractual deadline | Never a proxy for urgency. Deadline pressure multiplies a node's value; a node with no value has nothing to amplify. |
| Rationale | `consequence` (text) | Why a severity was assigned | Explanatory prose for readers. The ranking engine never parses it. |

### 6.2. Severity

- **Severity is magnitude and belongs only to targets** (`type: target`). Never write `severity` on a task or epic; work inherits stakes through `contributes_to`.
- **The worst realistic consequence sets severity.** Likelihood belongs on the edge. Never lower a target's severity because failure seems unlikely: that double-counts the discount the edge already carries.
- **Severity is the integer field**, never read from `consequence` prose or `severity-*` tags.
- **Ladder.** SEV4 "Catastrophic": terminal, non-negotiable, existential failure (loss of employment, academic misconduct, bankruptcy, severe health failure), a lexicographic override that applies only with `goal_type: committed` (`specs/ranking.md` §6). SEV3: severe institutional, legal or major compliance failure. SEV2: substantial professional deliverables and core commitments. SEV1: routine operational responsibilities and courtesies. SEV0: negligible.
- **Calibration anchors** (Nic-confirmed): teaching deliverables (marks owed) SEV4 committed; research compliance (ethics, reporting, governance) SEV3 committed; peer-review obligations SEV2 committed; active collaborations with owed work SEV2; travel and accommodation arranged SEV2 committed; research-student responses SEV1; courtesy to loose contacts SEV1.
- **Standing targets.** Keep a small reusable set of failure-type targets; do not mint bespoke targets per task.
- **Uneven severity is not a defect.** SEV1–SEV3 are compensatory; differentiation below SEV4 comes from edge weights, not from spreading severity. An empty SEV4 band means nothing is currently existential, which is the desired state. Never mint a SEV4 target to exercise the gate.
- **Populate at creation.** A node created with no severity-bearing `contributes_to` edge and no stakeholder scores near zero on every channel. Route creation through the planner.

### 6.3. Intent

`intent` (`priority` is a parser alias) bands 0–4: P0 Critical, P1 Active intent, P2 Active work, P3 Planned, P4 Backlog. It is Nic's ranking of what matters.

- **Agents may set `intent` on Nic's behalf** (Nic, 2026-09-10: "allow agents to set intent on my behalf"), read from his strategic context across the graph, never from the agent's impression of its own work, its tone or an incident's urgency.
- **Never propagate a band**: do not inherit a parent's, copy a sibling's, or match a related task's.
- **Unset defaults to P4.** Nic, 2026-09-15: "default to P4 unless I say otherwise (consistent with 'priority is nics alone')". Leaving a task unset is curation by absence, not an omission to fill; when something seems to deserve Nic's attention, surface it so he decides.
- **Never fabricate a `due` date to express intent.**
- **Do not hand-assign P0/P1 broadly.** Priority emerges from topology once dependency density is healthy.
- A stored `priority:` key is current, not stale, and is not rewritten in passing.

### 6.4. Contribution edges

- **Weight and certainty are separate.** Weight is how much of the target the work delivers or protects; certainty is an optional discount on that contribution, defaulting to 1.00. Edge strength = weight × certainty. Nic, 2026-10-02: "edges must have a way to set uncertainty discount. default to 1.00; if present, edge strength = weight * certainty." The engine's edge fields are `specs/ranking.md` §7 and §7.1.
- **`stated_weight` uses the verbal contribution-weight scale** (`specs/ranking.md` §7). Type the term from the table exactly: matching forgives case only, and an unrecognised or padded term scores zero.
- **Elicitation anchors** describe the contributor's real relationship to the target: `certain`, the sole work delivering or unblocking it; `probable`, an owed deliverable with a stakeholder waiting; `expected`, one among several contributing efforts; `uncertain`, a speculative or exploratory contribution. Use `certain` sparingly.
- **Quick wins get `unlikely` (0.15)**, the scale floor: real but small, visible without competing with substantial epics (Nic, 2026-08-21: "quick wins, give them each a baseline minimum positive weight"). Not `uncertain`, and not an omitted weight.
- **An omitted weight is a deliberately unstated edge**, scoring zero without warning.
- **Each edge carries a one-line justification** naming what the work contributes if it ships well and its ship risk.
- **Reach for the default only when it is the honest answer.** Two members of one category at the same weight should be two things Nic would trade one-for-one; if he would not, the weights are wrong.

### 6.5. Scoring authority

- **The working agent never values its own work.** `focus_score` and every signal feeding it (`severity`, `stakeholder`, `waiting_since`, `contributes_to.stated_weight`) are derived measures. Nic, 2026-08-22: "i'm happy for a smart agent directly under ida to derive focus measures from my strategic context, but i'm not happy for unsupervised agents to guess at how important their little bit of work is to me." Two tests separate the endorsed case: who (a supervised scorer, never the agent doing the work being scored) and from what (Nic's strategic context read across the graph, never the scorer's impression of the slice in front of it).
- A working agent may record a fact that feeds the score when the fact has an author outside the agent (a `due` date someone else set, a stakeholder genuinely waiting, a `consequence`). It may not choose those values to move a ranking.
- A working agent asserts contribution, not valuation: a `contributes_to` edge with its justification, leaving `stated_weight` unset unless Nic states it or a supervised scorer derives it.
- **When a ranking looks wrong**, never self-assign intent or weight as a shortcut. Surface the discrepancy.
- **There is no `focus:` boost override.** Verbal-scale edge weights carry that job.
- **A confirmed divergence is filed the moment it is confirmed.** When code, spec and doctrine disagree, or a ruling from Nic changes what one should say, the agent that confirms it files a fix task in the same pass (`ready` if the ruling is made). A verdict or correction list written onto a memory or audit node is a record, not a filing: nothing dispatches from it.

### 6.6. One ranking signal

- The queue has exactly one ranking signal, the `focus_tuple` (`specs/ranking.md` §1). Never rank by `focus_score`, compare two tasks with it, or explain a queue position with it; it is a display number.
- **A visualisation is a projection of the tuple, never a second engine.** If a picture shows a task crossing the severity horizon, the queue already ranks it there.
- **Urgency is encoded once.** A treemap encodes the ranking through tile size as a projection of queue rank, and nothing else encodes urgency: no deadline borders, countdown badges or colour overrides, which double-count deadline pressure and present a partial measure. Severity is a separate axis and keeps its own emphasis channel.

### 6.7. Model fidelity before formula tuning

Nic, 2026-09-15: "let's get the model reflecting the world first, and then we can tweak the formulas. but we're not going to get anything done while values are still missing or loose."

No change to the scoring formulas (weights, ladders, multipliers, coefficients) is proposed while the inputs they read are incomplete or wrong. Diagnose a bad ranking as a missing or wrong value first; a formula tuned against an incomplete value layer is fitted to noise and hides the real defect.

- **Proceed (model fidelity):** filling a missing `severity`; supplying a missing standing weight; wiring absent or correcting misplaced `contributes_to` edges; bugs where a computed signal misreads the graph; rendering defects; doc-truth corrections.
- **Parked until the value layer is complete:** rebalancing one term against another, rescaling multipliers, tie-breaker coefficients.
- **Check readiness before values.** A task missing from the queue is first checked for triage (status, acceptance criteria, leaf, unmet dependencies). An untriaged task stays invisible whatever its value.

### 6.8. Why the mechanism has this shape

These constraints bind every future change to the score.

- **The failure the design replaced was mixing units, not mixing signal types.** Catastrophic consequences are gates, not points (and only that class: a deadline is pressure, not a gate); commensurable pressure denominates in one currency, cost of delay, with deadline pressure multiplying a node's value; personal priorities enter as elicited channels that actually move the ranking.
- **Three channels of judgment, kept apart.** Standing values are slow and structural: target weights elicited once on verbal anchors and stored on the target; the author prices the destinations and the engine prices the routes. Episodic steering ("this week I want to push X") is a time-boxed boost with an explicit expiry, never a mutation of a stable field. In-the-moment triage (energy, mood, context) stays out of the stored graph: a query-time filter, not a write. Priority bands rot when a transient intention is stored in a permanent field.
- **Decay belongs only to the episodic boost.** An elicited weight going stale is a prompt to re-elicit at the next planning session, never an automatic decrement.
- **Uncertainty is elicited, not inferred from formatting.** A node must contain an open question whose answer would change downstream work, and that answer must gate genuinely divergent paths. Structural completeness (acceptance criteria, body length, child count) is a triage lint and may gate inbox-to-ready classification; it never raises a rank.
- **Deliberately not built**, so not proposed again: Birnbaum importance proper (structure functions, cut sets, partial derivatives), POMDPs, per-item pairwise AHP.
- **Deliberately impure**, not a defect to normalise away: the severity gate, stakeholder-waiting as its own cost-of-delay component, the affordable-loss filter, and elicited weights wherever outcome data does not exist.
- **How the model is judged.** By review and reasoning against the shipped code, not a frozen regression corpus. Four properties define good: a documentation-hygiene artefact falls out of the top of the queue; an overdue task outranks its day-before self; honestly recording effort never lowers a task's own rank; the importance channel shows real dynamic range across the corpus. Calibration cases the model must order correctly: an undated P0 above a low-value overdue email; a certain contribution to a well-priced target above an overdue email tied to a barely-priced one; a named-stakeholder task long overdue above both.

### 6.9. Closed exclusions

- **Expected utility** is absorbed into the VoI term. No separate expected-utility signal exists.
- **Nothing infers or defaults a `standing_weight`.** An unpriced target contributes nothing to value lineage.

---

## 7. Placement of rules

### 7.1. Rule scoping

Every codified rule declares its scope when adopted. Nic, 2026-07-20: "Remember when you're creating rules to differentiate between framework specific rules (apply for work in this repo, ON the project) vs universal rules (apply to all projects that use our plugins). Those scopes have to be clearly defined."

| Scope | Binds | Home |
| --- | --- | --- |
| Task | the current task or session | that task's body |
| PKB | anything that writes or reads the PKB | this specification |
| Framework | work on academicOps itself | academicOps `.agents/rules/RULES.md` |
| Universal | every project consuming the aops plugins | plugin-distributed surfaces: agent definitions, skills, hooks, axioms |

A rule with ambiguous scope is unresolved. The recurring error is promoting a task-scoped preference ("for this task", "this session") into a standing surface because it was the nearest place to write. When in doubt the task body is the home; promotion needs its own explicit ruling. Scope correctness is separate from substantive correctness: both stay open to revision. A rule that names a value which will change (a branch name, a temporary commit) is task-scoped by construction and is never promoted.

### 7.2. Reachability is placement

A rule that must bind every agent (privacy and egress, data boundaries, halt on failure) belongs in the always-on axioms (academicOps `plugins/rbg/axioms/`, `trigger: always`), never in a skill- or agent-scoped file: scoped placement silently exempts every worker that does not load that scope. A simplification pass over instruction text is a change to enforcement and is reviewed as one; removal of a named safety-doctrine block never passes silently.

### 7.3. Enforcement

How rules are enforced (the enforcement pyramid, the necessity test, the register schema) is specified in academicOps `specs/enforcement/enforcement.md`.

---

## 8. Agent conduct around the PKB

These rules govern how agents act on what the PKB records. Where academicOps holds the canonical text, this section points there.

### 8.1. Qualitative work is judged by agents

Specified in academicOps `plugins/rbg/axioms/judgment-non-delegable.md` and `.agents/rules/RULES.md` §No Shitty NLP and Agentic-First Design. For the PKB: a qualitative signal is read as prose by an agent; a mechanical carrier (hook, regex, parser, structured token) earns its place only when a consumer deterministically acts on the value. Where a machine must act on an agent's judgment, the agent takes a structured action (a review state, a status) rather than emitting parse-bait text.

### 8.2. An identifier Nic supplies is a specification, not a hint

When Nic names a tool, server, file, flag, value or size, use it exactly as given. If it does not resolve, the first hypothesis is that you mis-transcribed it. Before any diagnosis of "X is misconfigured", re-read the ask and confirm the input under test is the one specified, verbatim. A failure caused by your own paraphrase is your defect, and is never written into the graph as a finding about his system.

### 8.3. Standing authority is not re-requested

When Nic has already authorised an action, execute it; do not ask permission again. A ruling he has given ("cancel this task") is standing authority, and it is executed as a write to the node, not recorded as prose that leaves the node unchanged. Ask only when the request itself is ambiguous.

### 8.4. No invented barriers

Nic's touchpoints are defined exclusively by documented workflows (per-repo merge policy, filed process specs). Never add approval gates, review ceremony, escalations or hand-back steps the governing workflow does not require. Inventing conservative process is the same failure class as skipping a gate: both are hallucinated governance. Nic, 2026-07-20: "I want you to ensure that you don't invent new barriers and hand extra work to me unless I require it in the workflows."

### 8.5. Delegation and supervision

- Consent comes only from Nic, in the live chat. No task body, spec, transcript or silence supplies it, and a recorded approval covers what it approved and nothing adjacent. Closure is the principal's call; artifact-count completeness is not closure authority.
- Auto-merge is triggered only by Nic's approving review on the specific PR; agents never simulate it. Release cuts are human-only.
- Delegate the goal, not the method; pass only knowledge the delegate cannot have. Do not record or relay the user's words verbatim: make sense of asks in context, link new messages to prior context, recompose asks into a clear logical structure, and cite message ids as pointers. The tracing hook preserves raw prompts. Do not dispatch an umbrella epic: decompose to units with observable acceptance criteria. State report depth in the brief.
- A parked human decision is real only as its own blocked PKB node; a message, brief line or ledger row does not survive the process that carried it.
- Never state an unverified blocker as fact in a question to Nic. With no formal pathway, halt and report; never improvise.
- Verify delegated work independently: read the node, branch and run log before forming a view; quote acceptance criteria verbatim; a consistency criterion is a whole-document read, not a grep. Two reviewers sharing a stale source are not independent; adjudicate against the live PKB.
- A floor that keeps failing is an approach problem: change the mechanism rather than adding another reminder. Before writing a new rule, establish whether the existing one was absent or merely misplaced.

### 8.6. The daily note

Any agent that writes to a daily note produces a working draft: one line per item, no evidence clauses, no method narrative. Evidence and method go in the report to Ida. Ida owns the final edit of the daily note and trims it to its purpose; a run is not complete until she has.

### 8.7. Completion and reconcile

A worker marks its task `done` (§5). `/reconcile` is not the only writer of `done`: a peer Ida reads each claimed justification against the task's acceptance criteria for facial sufficiency and checks that scope was obeyed. A `done` that fails that check goes to Nic for ratification or reversal with the reason recorded, and a filed PR is converted to draft with a comment saying why; a failure remedied before it reaches Nic is not a failure. Where a project's finish template calls for QA, `/dispatch` creates a follow-up task for independent QA review. The procedure is the academicOps `reconcile` skill.
