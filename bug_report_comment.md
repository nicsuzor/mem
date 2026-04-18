### Discovery Bloat Context (from task-07bd6225)

The agent was assigned `task-07bd6225` (mem: PKB Server & CLI). Despite having the task ID and project context, it performed global searches (`list_tasks(status="active")`) which injected hundreds of irrelevant tasks into the context.

#### Root Cause Analysis
- **Trigger**: Pre-flight instructions (Step 0) mandate verifying that a task is `active` or `in_progress`.
- **Failure**: When `get_task` returned `status: null` for the project-type task, the agent attempted to 'locate' it by surveying the entire active task landscape.
- **Discovery Gap**: The agent failed to use the recently added `project` filter or a targeted `get_task` follow-up, opting for an expensive global list instead.

#### Recommendation
Update **Pre-flight** instructions to explicitly mandate scoped verification. If a task status is unclear, agents should be directed to check immediate parent/child relationships or use `project` filters rather than global status listings.

#### Framework Reflection
**Prompts**: Session initiated with specific task `task-07bd6225`.
**Outcome**: Success in identifying discovery inefficiency.
**Issue**: https://github.com/nicsuzor/academicOps/issues/579
