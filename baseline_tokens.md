# Baseline MCP Token Usage (2026-04-19)

Estimated tokens for `plugin_aops-core_pkb` MCP server.
Method: Character count / 4.

| Tool Name | Tokens (est) | Chars |
|-----------|--------------|-------|
| batch_update | 581 | 2327 |
| create | 580 | 2321 |
| create_task | 460 | 1841 |
| decompose_task | 455 | 1823 |
| release_task | 427 | 1709 |
| batch_archive | 421 | 1686 |
| batch_create_epics | 384 | 1539 |
| batch_reparent | 374 | 1497 |
| list_tasks | 330 | 1323 |
| create_memory | 302 | 1208 |
| batch_reclassify | 261 | 1047 |
| merge_node | 255 | 1023 |
| search | 229 | 918 |
| bulk_reparent | 223 | 894 |
| pkb_orphans | 220 | 881 |
| create_subtask | 207 | 829 |
| find_duplicates | 204 | 816 |
| complete_task | 198 | 795 |
| append | 192 | 771 |
| batch_merge | 187 | 750 |
| get_document | 185 | 743 |
| retrieve_memory | 178 | 714 |
| list_documents | 177 | 709 |
| get_task | 176 | 706 |
| search_by_tag | 162 | 651 |
| pkb_trace | 162 | 649 |
| pkb_context | 158 | 633 |
| task_search | 156 | 625 |
| get_dependency_tree | 137 | 550 |
| get_task_children | 136 | 544 |
| list_memories | 125 | 503 |
| delete | 110 | 442 |
| delete_memory | 107 | 431 |
| task_summary | 107 | 429 |
| get_network_metrics | 93 | 372 |
| graph_stats | 78 | 315 |
| graph_json | 52 | 208 |
| **TOTAL** | **8789** | |

Target: < 2,000 tokens.
Actual observed in `/context`: 59.6k tokens across all MCP servers (PKB is ~15% of total MCP overhead).
