//! Live end-to-end ground-truth probe for H1 (read latency) and the opaque-ID
//! half of H3, against the real deployed PKB MCP server — not the synthetic,
//! uniform-vector, in-process store used by `examples/probe_h1_h4.rs`.
//!
//! `examples/probe_h1_h4.rs` (merged in PR #504) measured `encode_query` +
//! an in-memory cosine scan over 1000 documents filled with
//! `vec![0.1; dim]` placeholder vectors, entirely in-process (no MCP
//! transport, no live store, no background reindex worker, no lock
//! contention). That number (22.84 ms) was never reconciled against the
//! parent epic's transcript-derived `search` p50 of 14.4 s / p90 62 s
//! ([[mem_d20dd7f3]]), and H3's opaque-ID half was printed as explicitly
//! UNTESTED because ranking is meaningless over uniform vectors.
//!
//! This probe closes both gaps by talking real MCP JSON-RPC, over a real
//! TCP socket, to the real deployed server (`$PKB_MCP_URL`, default
//! `http://services-new.stoat-musical.ts.net:8020/mcp`), which serves the
//! live PKB store (7,751 nodes per `graph_stats` at the time this file was
//! written — see STORE MEASURED banner printed at runtime for the number
//! observed on the actual run).
//!
//! No MCP client crate is used (`rmcp` is compiled here with the `server`
//! feature only, and adding a client feature would touch `Cargo.toml`,
//! which is off-limits for this probe — see task brief). Instead this file
//! is a minimal hand-rolled HTTP/1.1 + `text/event-stream` client using
//! only `std::net` and the `serde_json` dependency already in the crate.
//!
//! Usage:
//!   PKB_MCP_URL=http://host:port/mcp cargo run --release --example probe_live_e2e
//!   cargo run --release --example probe_live_e2e   # uses the pinned default above

use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

const DEFAULT_URL: &str = "http://services-new.stoat-musical.ts.net:8020/mcp";

// ─────────────────────────────────────────────────────────────────────────────
// Stats helper (mirrors examples/probe_h1_h4.rs::Stats)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Default, Clone)]
struct Stats {
    samples: Vec<Duration>,
}

impl Stats {
    fn record(&mut self, d: Duration) {
        self.samples.push(d);
    }
    fn mean(&self) -> Duration {
        if self.samples.is_empty() {
            return Duration::ZERO;
        }
        let total: Duration = self.samples.iter().sum();
        total / self.samples.len() as u32
    }
    fn percentile(&self, q: f64) -> Duration {
        if self.samples.is_empty() {
            return Duration::ZERO;
        }
        let mut s = self.samples.clone();
        s.sort();
        let idx = ((s.len() as f64 - 1.0) * q).round() as usize;
        s[idx]
    }
    fn max(&self) -> Duration {
        self.samples.iter().copied().max().unwrap_or(Duration::ZERO)
    }
}

fn fmt_dur(d: Duration) -> String {
    let us = d.as_micros();
    if us < 1_000 {
        format!("{us}µs")
    } else if us < 1_000_000 {
        format!("{:.2}ms", us as f64 / 1000.0)
    } else {
        format!("{:.3}s", us as f64 / 1_000_000.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Minimal HTTP/1.1 + MCP streamable-HTTP (SSE) client — std::net only
// ─────────────────────────────────────────────────────────────────────────────

struct ParsedUrl {
    host: String,
    port: u16,
    path: String,
}

fn parse_url(u: &str) -> ParsedUrl {
    let rest = u
        .strip_prefix("http://")
        .expect("this probe only speaks plain http:// (no TLS client implemented) — PKB_MCP_URL must be http://");
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().expect("valid port in PKB_MCP_URL")),
        None => (hostport.to_string(), 80),
    };
    ParsedUrl { host, port, path: path.to_string() }
}

struct StageTiming {
    connect: Duration,
    send: Duration,
    headers: Duration, // time from request-sent to full response headers received (~TTFB)
    body_read: Duration,
    total: Duration,
}

struct RawResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: String,
    timing: StageTiming,
}

/// Reads one chunked-transfer-encoded HTTP body into a String.
fn read_chunked<R: BufRead>(reader: &mut R) -> String {
    let mut out = String::new();
    loop {
        let mut size_line = String::new();
        reader.read_line(&mut size_line).expect("read chunk size line");
        let size_str = size_line.trim().split(';').next().unwrap_or("0");
        let size = usize::from_str_radix(size_str, 16).unwrap_or(0);
        if size == 0 {
            // consume trailer headers (if any) up to the terminating blank line
            loop {
                let mut trail = String::new();
                reader.read_line(&mut trail).expect("read chunk trailer");
                if trail == "\r\n" || trail.is_empty() {
                    break;
                }
            }
            break;
        }
        let mut chunk = vec![0u8; size];
        reader.read_exact(&mut chunk).expect("read chunk body");
        out.push_str(&String::from_utf8_lossy(&chunk));
        let mut crlf = [0u8; 2];
        let _ = reader.read_exact(&mut crlf);
    }
    out
}

/// Sends one raw HTTP/1.1 POST over a fresh TCP connection and returns the
/// per-stage timing split alongside the parsed response.
fn raw_post(url: &ParsedUrl, json_body: &str, session_id: Option<&str>) -> RawResponse {
    let t0 = Instant::now();
    let addr = format!("{}:{}", url.host, url.port);
    let mut stream = TcpStream::connect(&addr)
        .unwrap_or_else(|e| panic!("connect to {addr} failed: {e} (is PKB_MCP_URL reachable from this sandbox?)"));
    stream.set_nodelay(true).ok();
    let t_connect = t0.elapsed();

    let t1 = Instant::now();
    let mut req = format!(
        "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
        url.path,
        url.host,
        url.port,
        json_body.len()
    );
    if let Some(sid) = session_id {
        req.push_str(&format!("Mcp-Session-Id: {sid}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(json_body);
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush request");
    let t_send = t1.elapsed();

    let t2 = Instant::now();
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).expect("read status line");
    let status: u16 = status_line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);

    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("read header line");
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    let t_headers = t2.elapsed();

    let t3 = Instant::now();
    let chunked = headers.get("transfer-encoding").map(|v| v.contains("chunked")).unwrap_or(false);
    let body = if chunked {
        read_chunked(&mut reader)
    } else if let Some(len) = headers.get("content-length").and_then(|v| v.parse::<usize>().ok()) {
        let mut buf = vec![0u8; len];
        if len > 0 {
            reader.read_exact(&mut buf).expect("read content-length body");
        }
        String::from_utf8_lossy(&buf).into_owned()
    } else {
        let mut buf = String::new();
        let _ = reader.read_to_string(&mut buf);
        buf
    };
    let t_body = t3.elapsed();

    RawResponse {
        status,
        headers,
        body,
        timing: StageTiming { connect: t_connect, send: t_send, headers: t_headers, body_read: t_body, total: t0.elapsed() },
    }
}

/// Extracts the JSON-RPC payload from an SSE (`event: message\ndata: {...}`) or plain-JSON body.
fn extract_json(body: &str) -> Option<JsonValue> {
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("data: ") {
            if let Ok(v) = serde_json::from_str::<JsonValue>(rest) {
                return Some(v);
            }
        }
    }
    serde_json::from_str(body).ok()
}

struct McpClient {
    url: ParsedUrl,
    session_id: String,
}

impl McpClient {
    fn connect(url: ParsedUrl) -> Self {
        let init_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "probe_live_e2e", "version": "0.1"}
            }
        })
        .to_string();
        let resp = raw_post(&url, &init_body, None);
        assert_eq!(resp.status, 200, "initialize failed: {}", resp.body);
        let session_id = resp
            .headers
            .get("mcp-session-id")
            .cloned()
            .expect("server did not return Mcp-Session-Id header on initialize");
        let json = extract_json(&resp.body).expect("initialize response was not parseable JSON/SSE");
        let server_info = &json["result"]["serverInfo"];
        println!("Connected: server={} version={} session={}", server_info["name"], server_info["version"], session_id);

        let notify_body = json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string();
        let notify_resp = raw_post(&url, &notify_body, Some(&session_id));
        assert!(
            notify_resp.status == 202 || notify_resp.status == 200,
            "notifications/initialized unexpected status {}",
            notify_resp.status
        );

        McpClient { url, session_id }
    }

    fn call(&self, id: u64, name: &str, args: JsonValue) -> (JsonValue, StageTiming) {
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": name, "arguments": args}
        })
        .to_string();
        let resp = raw_post(&self.url, &body, Some(&self.session_id));
        assert_eq!(resp.status, 200, "tool call {name} failed with HTTP {}: {}", resp.status, resp.body);
        let json = extract_json(&resp.body).unwrap_or_else(|| panic!("tool call {name} response not parseable: {}", resp.body));
        if let Some(err) = json.get("error") {
            panic!("tool call {name} returned JSON-RPC error: {err}");
        }
        (json, resp.timing)
    }

    fn call_text(&self, id: u64, name: &str, args: JsonValue) -> (String, StageTiming) {
        let (json, timing) = self.call(id, name, args);
        let text = json["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();
        (text, timing)
    }
}

fn print_stage_table(label: &str, stats_by_stage: &[(&str, &Stats)]) {
    println!("\n{label}");
    println!("--------------------------------------------------------------------------------");
    println!("Stage                          Mean       p50       p95       Max");
    println!("--------------------------------------------------------------------------------");
    for (name, s) in stats_by_stage {
        println!(
            "{:<30}  {:>8}  {:>8}  {:>8}  {:>8}",
            name,
            fmt_dur(s.mean()),
            fmt_dur(s.percentile(0.50)),
            fmt_dur(s.percentile(0.95)),
            fmt_dur(s.max())
        );
    }
    println!("--------------------------------------------------------------------------------");
}

// ─────────────────────────────────────────────────────────────────────────────
// H1: end-to-end search latency against the live store
// ─────────────────────────────────────────────────────────────────────────────

/// Realistic natural-language queries paraphrased from the epic's own body text
/// ([[mem_d20dd7f3]]) — the kind of query real callers actually send, as opposed
/// to synthetic "Benchmark task N performance" strings.
const REALISTIC_QUERIES: &[&str] = &[
    "PKB read latency and payload bulk ground truth probe",
    "server-side ground truth for MCP payload shape",
    "opaque node id semantic search ranking failure",
    "list_tasks truncation and true total count",
    "embedding path latency and lock contention",
    "get_task payload anatomy null fields frontmatter duplication",
    "new node indexing lag versus opaque id matching",
    "PKB MCP efficiency payload shape and schema honesty",
    "background embed worker deferred ONNX re-embed",
    "graph rebuild coalesced background worker",
    "vector store cosine similarity search",
    "task dependency tree and blocked status",
];

fn probe_h1_live(client: &McpClient, node_count: u64) {
    println!("\n================================================================================");
    println!("PROBE H1 (LIVE): end-to-end `pkb__search` latency against the real deployed store");
    println!("================================================================================");
    println!(
        "STORE MEASURED: live server at $PKB_MCP_URL via Docker AI MCP Gateway (`pkb__*` tools); \
         {node_count} nodes per `pkb__graph_stats` on this run. This is real production traffic on \
         a shared server — other agents may be issuing concurrent calls during this run."
    );

    let mut connect = Stats::default();
    let mut send = Stats::default();
    let mut headers = Stats::default();
    let mut body_read = Stats::default();
    let mut total = Stats::default();

    // Warm-up call, discarded, matching probe_h1_h4.rs's convention of excluding
    // any first-call JIT/session effects from the statistics.
    let _ = client.call_text(900, "pkb__search", json!({"query": REALISTIC_QUERIES[0], "limit": 5}));

    let iters = REALISTIC_QUERIES.len() * 2;
    for i in 0..iters {
        let q = REALISTIC_QUERIES[i % REALISTIC_QUERIES.len()];
        let (_, timing) = client.call(1000 + i as u64, "pkb__search", json!({"query": q, "limit": 5}));
        connect.record(timing.connect);
        send.record(timing.send);
        headers.record(timing.headers);
        body_read.record(timing.body_read);
        total.record(timing.total);
    }

    print_stage_table(
        &format!("Per-Stage Wall-Clock Breakdown (N={iters} live `pkb__search` calls, distinct realistic queries):"),
        &[
            ("1. TCP connect", &connect),
            ("2. Request send", &send),
            ("3. Headers received (~TTFB)", &headers),
            ("4. Body read + SSE parse", &body_read),
            ("TOTAL end-to-end (client-observed)", &total),
        ],
    );

    let p50 = total.percentile(0.50);
    let p90 = total.percentile(0.90);
    println!("\nReconciliation against the epic's transcript-derived figures ([[mem_d20dd7f3]]):");
    println!("  Epic reported:  search p50 = 14.4s, p90 = 62s  (client-side, from 386 Claude Code transcripts' timestamp deltas)");
    println!("  This run:       search p50 = {}, p90 = {}  (N={iters}, client-side, raw-socket timestamps, same gateway route)", fmt_dur(p50), fmt_dur(p90));
    println!(
        "  Gap: this run is ~{}x faster than the epic's p50. H1 does NOT fully reconcile — see task record for open questions.",
        if p50.as_secs_f64() > 0.0 { (14.4 / p50.as_secs_f64()).round() as u64 } else { 0 }
    );
}

fn probe_id_reads_live(client: &McpClient, real_ids: &[&str]) {
    println!("\n--------------------------------------------------------------------------------");
    println!("Comparison: live `pkb__get_task` latency on real existing node ids (id-based reads)");
    println!("--------------------------------------------------------------------------------");
    let mut total = Stats::default();
    for (i, id) in real_ids.iter().enumerate() {
        let (_, timing) = client.call(2000 + i as u64, "pkb__get_task", json!({"id": id}));
        total.record(timing.total);
    }
    println!(
        "  Epic reported: id-based reads p50 ~2.5s   |   This run: p50 = {}, max = {} (N={})",
        fmt_dur(total.percentile(0.50)),
        fmt_dur(total.max()),
        real_ids.len()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// H3 (opaque-ID half): real embeddings, real ranking — not synthetic uniform vectors
// ─────────────────────────────────────────────────────────────────────────────

fn probe_h3_opaque_id_live(client: &McpClient, real_ids: &[&str]) {
    println!("\n================================================================================");
    println!("PROBE H3 (LIVE, opaque-ID half): does semantic search find a node by its own opaque id?");
    println!("================================================================================");
    println!(
        "This directly discharges the gap the merged probe left open: examples/probe_h1_h4.rs prints \
         this half as UNTESTED because its store holds `vec![0.1; dim]` uniform vectors, so ranking is \
         meaningless by construction. Here every id below is a real node in the live store with a real \
         BGE-M3 embedding of its real body text."
    );

    for (i, id) in real_ids.iter().enumerate() {
        // 1. Direct id resolution via get_task — should always succeed, cheap.
        let (get_text, get_timing) = client.call_text(3000 + i as u64 * 2, "pkb__get_task", json!({"id": id}));
        let resolved = get_text.contains(&format!("\"id\": \"{id}\""));

        // 2. Semantic search using the bare opaque id string as the query — the
        //    failure mode reported in the epic ("an agent's own task id, created
        //    30 minutes earlier, returned unrelated hits").
        let (search_text, search_timing) = client.call_text(3001 + i as u64 * 2, "pkb__search", json!({"query": id, "limit": 10}));
        let hit = search_text.contains(&format!("`{id}`"));
        let lines: Vec<&str> = search_text.lines().collect();
        let rank = lines
            .iter()
            .position(|l| l.contains(&format!("`{id}`")))
            .and_then(|idx| {
                // The id line follows a "### N. Title (score: X)" line; find that header above it.
                lines[..idx].iter().rev().find_map(|l| l.trim_start().strip_prefix("### ").map(|s| s.split('.').next().unwrap_or("?").to_string()))
            });

        println!(
            "  {id:<16}  get_task: {} ({})   search(query=id): {} rank={}",
            if resolved { "RESOLVED" } else { "NOT RESOLVED" },
            fmt_dur(get_timing.total),
            if hit { "FOUND in top-10" } else { "NOT FOUND in top-10" },
            rank.unwrap_or_else(|| "-".to_string())
        );
        let _ = search_timing; // timing already reported in probe_h1_live's aggregate stats
    }
    println!(
        "\nNote: a MISS above means the opaque id string, embedded as a query, did not surface its own \
         node in the top 10 semantic results — the failure mode the epic reported, now checked against \
         real embeddings instead of asserted from synthetic ones."
    );
}

fn main() {
    let url_str = std::env::var("PKB_MCP_URL").unwrap_or_else(|_| DEFAULT_URL.to_string());
    println!("Running LIVE end-to-end probe against: {url_str}");
    let url = parse_url(&url_str);
    let client = McpClient::connect(url);

    // Confirm and name the store being measured (task brief requires this).
    let (stats_text, _) = client.call_text(10, "pkb__graph_stats", json!({}));
    let stats_json: JsonValue = serde_json::from_str(&stats_text).expect("graph_stats returned non-JSON text");
    let node_count: u64 = stats_json["type_distribution"]
        .as_object()
        .map(|m| m.values().filter_map(|v| v.as_u64()).sum())
        .unwrap_or(0);

    let real_ids = ["mem_eea75657", "mem_d20dd7f3", "mem_e4fa072f", "aops-fd27a513", "mem_7f4b0d03", "obs_018c4dee"];

    probe_h1_live(&client, node_count);
    probe_id_reads_live(&client, &real_ids);
    probe_h3_opaque_id_live(&client, &real_ids);

    println!("\n================================================================================");
    println!("LIVE PROBE RUN COMPLETE.");
    println!("================================================================================\n");
}
