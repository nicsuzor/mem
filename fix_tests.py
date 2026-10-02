with open('src/mcp_server/tests/task_mutation_tests.rs', 'r') as f:
    text = f.read()

def remove_test(test_name):
    global text
    idx = text.find(f"fn {test_name}")
    if idx == -1: return
    # Find #[test]
    start_idx = text.rfind('#[test]', 0, idx)
    # Find {
    brace_idx = text.find('{', idx)
    depth = 1
    i = brace_idx + 1
    while depth > 0 and i < len(text):
        if text[i] == '{': depth += 1
        elif text[i] == '}': depth -= 1
        i += 1
    # Remove from start_idx to i
    text = text[:start_idx] + text[i:]

tests_to_remove = [
    "test_release_task_blocked_without_reason_or_blocker_is_rejected",
    "test_release_task_blocked_with_reason_is_accepted",
    "test_release_task_blocked_with_only_blocker_is_accepted",
    "test_update_task_bare_status_blocked_is_rejected",
    "test_update_task_status_blocked_with_blocker_is_accepted",
    "test_update_task_status_blocked_reuses_already_stored_blocker",
    "test_blocker_resolution_case_insensitive_mcp",
    "test_list_tasks_status_blocked_excludes_stored_ready_with_computed_block"
]

for t in tests_to_remove:
    remove_test(t)

# Fix test_release_task_reported_status_matches_persisted_status loop
text = text.replace('["cancelled", "blocked", "review", "partial", "done"]', '["cancelled", "review", "partial", "done"]')
text = text.replace('} else if status == "blocked" {\n                args["blocker"] = json!("declared explicitly for this test");\n            }', '}')

# Fix test_release_task_allows_blocked_and_cancelled_with_open_children
text = text.replace('"status": "blocked",\n                "summary": "Escalating as blocked.",\n                "blocker": "waiting on external decision",', '"status": "review",\n                "summary": "Escalating as review.",\n                "reason": "waiting on external decision",')
text = text.replace('release_task(status=blocked)', 'release_task(status=review)')

# Fix test_release_task_persists_reason_and_blocker_to_frontmatter
text = text.replace('"status": "blocked",\n                "summary": "Blocked.",\n                "blocker": "waiting on external API access",', '"status": "review",\n                "summary": "Blocked.",\n                "reason": "waiting on external API access",\n                "blocker": "waiting on external API access",')
text = text.replace('release to blocked', 'release to review')

# Fix test_list_tasks_per_status_totals_disjoint_and_sum_correctly
text = text.replace('["ready", "queued", "in_progress", "blocked", "review"]', '["ready", "queued", "in_progress", "review"]')
text = text.replace('status: blocked\\nblocker: x', 'status: review\\nreason: x')

# Fix test_release_task_missing_created
idx3 = text.find('fn test_release_task_missing_created')
if idx3 != -1:
    end_idx = text.find('}', idx3)
    part = text[idx3:end_idx]
    part = part.replace('"status": "review",', '"status": "in_progress",')
    part = part.replace('matches!(err.code, ErrorCode::INVALID_PARAMS)', 'err.message.contains("missing created")')
    text = text[:idx3] + part + text[end_idx:]

with open('src/mcp_server/tests/task_mutation_tests.rs', 'w') as f:
    f.write(text)

