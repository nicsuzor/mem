# mem

Semantic search over personal knowledge base markdown files, exposed as an MCP server.

Uses [MiniLM-L6-v2](https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2) for 384-dimensional sentence embeddings via ONNX Runtime. Models and runtime are auto-downloaded on first run.

## Quick Start

```bash
cargo build --release --bin pkb-search

# Run with defaults (PKB at ~/brain or $ACA_DATA)
./target/release/pkb-search

# Custom PKB root
./target/release/pkb-search /path/to/notes

# Force full reindex
./target/release/pkb-search --reindex
```

## MCP Tools

| Tool               | Description                                                                 |
| ------------------- | --------------------------------------------------------------------------- |
| `semantic_search`  | Find documents by meaning. Params: `query` (string), `limit` (int, default 10) |
| `get_document`     | Read full contents of a file. Params: `path` (string)                      |
| `list_documents`   | Browse/filter documents. Params: `tag`, `type`, `status` (all optional)    |
| `reindex`          | Force a full re-scan of the PKB directory                                  |

## MCP Client Configuration

### Gemini CLI

Add to your extension or `settings.json`:

```json
{
  "mcpServers": {
    "pkb-search": {
      "command": "/home/nic/src/mem/target/release/pkb-search",
      "args": []
    }
  }
}
```

### Claude Code

Add to `.mcp.json`:

```json
{
  "mcpServers": {
    "pkb-search": {
      "command": "/home/nic/src/mem/target/release/pkb-search",
      "args": []
    }
  }
}
```

## Architecture

```text
MCP Client ◄──stdio──► pkb-search
                          │
                    ┌─────┴──────┐
                    │ MCP Server │  (rmcp 0.1, ServerHandler trait)
                    └─────┬──────┘
                          │
                    ┌─────┴──────┐
                    │VectorStore │  (bincode persistence, brute-force cosine search)
                    └─────┬──────┘
                          │
                    ┌─────┴──────┐
                    │  Embedder  │  (MiniLM-L6-v2 via ONNX Runtime)
                    └────────────┘
```

## Environment Variables

| Variable                    | Default     | Description                          |
| --------------------------- | ----------- | ------------------------------------ |
| `ACA_DATA`                 | `~/brain`   | PKB root directory                   |
| `RUST_LOG`                 | `info`      | Log level filter                     |
| `AOPS_OFFLINE`             | `false`     | Disable model/runtime auto-download  |
| `AOPS_MODEL_PATH`          | (auto)      | Override model directory path        |
| `ORT_DYLIB_PATH`           | (auto)      | Override ONNX Runtime library path   |

## Requirements

- Rust ≥ 1.88

## License

Private.
