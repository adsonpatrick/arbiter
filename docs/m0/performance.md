# M0 performance evidence

Date: 2026-08-20

## Result

**PASS.** The release-build local benchmark satisfies the candidate M0
governor-added latency limits at every percentile. M1 is not blocked by the M0
proxy overhead gate.

| Local added latency | p50 | p95 | p99 | Limit | Result |
| --- | ---: | ---: | ---: | ---: | --- |
| First byte | 0.192 ms | 1.194 ms | 7.627 ms | 10 / 25 / 50 ms | PASS |
| Complete response | 0.254 ms | 1.705 ms | 15.056 ms | 10 / 25 / 50 ms | PASS |

The benchmark also asserts for every proxied response that its first chunk is
observed before its terminal chunk. All 1,000 measured requests passed, so
Arbiter did not withhold the first byte until completion.

## Reproduction

```powershell
cargo run --release -p arbiter-m0-bench -- local --requests 1000 --warmup 50
```

The deterministic fake upstream waits 2 ms before its first SSE chunk and 2 ms
before its terminal SSE chunk. The client reuses one connection pool, performs
50 warm-up pairs, then interleaves 1,000 direct and 1,000 proxied requests.
Added distributions are nearest-rank percentiles of the paired
`proxied - direct` observations. Times below are microseconds.

| Metric | Path | p50 | p95 | p99 |
| --- | --- | ---: | ---: | ---: |
| First byte | Direct | 15,443 | 16,061 | 16,277 |
| First byte | Through Arbiter | 15,653 | 16,280 | 16,561 |
| First byte | Added | 192 | 1,194 | 7,627 |
| Complete response | Direct | 31,162 | 32,016 | 32,267 |
| Complete response | Through Arbiter | 31,443 | 32,383 | 32,733 |
| Complete response | Added | 254 | 1,705 | 15,056 |

Windows timer granularity makes the fake upstream's nominal 2 ms sleeps appear
near 15 ms. This affects both paths and is why the gate uses the paired added
distribution.

## Checkpoint finding and remediation

The first two runs exposed repeatable p99 pauses of 110–127 ms while p50/p95
remained low. A diagnostic run with SQLite's commit-triggered WAL auto-checkpoint
disabled reduced first-byte p99 added latency to 1.687 ms, identifying the
checkpoint as the cause.

The production correction disables commit-triggered auto-checkpoints and
debounces passive checkpoint maintenance until writes have been idle for 250 ms.
It does not weaken SQLite's synchronous setting, append durability, WAL mode, or
close-time checkpoint. The final numbers above include this bounded maintenance
strategy; they are not the diagnostic no-maintenance result.

## Live TTFT diagnostic

```powershell
proxy-overhead.exe live --samples 3
```

The live probe ran Codex CLI `0.148.0-alpha.21` with ChatGPT-managed
authentication, `gpt-5.6-terra`, medium reasoning, and no `OPENAI_API_KEY` in the
child environment. It measured time to the first completed agent-message event
without recording message content.

| Path | p50 | p95 | p99 |
| --- | ---: | ---: | ---: |
| Direct Codex | 7,100 ms | 10,109 ms | 10,109 ms |
| Codex through Arbiter | 4,006 ms | 4,199 ms | 4,199 ms |
| Paired added | -2,977 ms | -2,901 ms | -2,901 ms |

Three remote samples are diagnostic only: provider load and network variance
dominate local proxy cost, so the negative delta is not treated as a performance
claim. The temporary Arbiter install completed with
`CONFIG_RESTORED=True`, `PROFILE_RESTORED=True`, and uninstall exit code 0.
Event history remains in the reported temporary evidence directory; no prompt,
response content, credential, or API key was stored.

## Environment

- AMD Ryzen 5 5500U, 6 cores / 12 logical processors
- 19,165,339,648 bytes physical memory (about 17.85 GiB)
- Windows 11 Pro 64-bit, version `10.0.26200`, build `26200`
- Rust/Cargo `1.97.1`, target `x86_64-pc-windows-msvc`
- Optimized Cargo `release` build from parent commit `900ba6d`
