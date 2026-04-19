# Final MCP Token Usage (2026-04-19)

Estimated tokens for `plugin_aops-core_pkb` MCP server after consolidation.
Method: Character count / 4.

| Tool Name | Tokens (est) | Chars |
|-----------|--------------|-------|
| search | 114 | 457 |
| get_document | 55 | 221 |
| list_documents | 76 | 305 |
| create_document | 122 | 489 |
| manage_task | 98 | 393 |
| pkb_explore | 101 | 405 |
| pkb_batch | 96 | 387 |
| pkb_stats | 54 | 219 |
| pkb_tool_help | 51 | 206 |
| **TOTAL** | **767** | |

**Reduction: 91.2%** (from 8789 to 767 tokens)
Cold-start footprint is now well under the 2,000 token target.
Full functionality is preserved via progressive disclosure (consolidated tools + `pkb_tool_help`) and backward compatibility (legacy aliases preserved in `call_tool`).
