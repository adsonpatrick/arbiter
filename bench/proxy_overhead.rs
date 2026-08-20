use std::{
    io::{BufRead, BufReader},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use arbiter_daemon::{AppState, build_router};
use arbiter_provider_codex::provider::CodexUpstreamProvider;
use arbiter_storage_sqlite::SqliteEventStore;
use axum::{Router, body::Body, extract::State, http::header, response::Response, routing::post};
use bytes::Bytes;
use clap::{Parser, Subcommand};
use futures_util::{StreamExt, stream};
use reqwest::Client;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

const AUTHORIZATION: &str = "Bearer local-benchmark-only";
const FIRST_EVENT: &str = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\n";
const TERMINAL_EVENT: &str = concat!(
    "data: {\"type\":\"response.completed\",\"response\":{",
    "\"id\":\"resp_benchmark\",\"usage\":{",
    "\"input_tokens\":8,\"input_tokens_details\":{\"cached_tokens\":2},",
    "\"output_tokens\":3,\"output_tokens_details\":{\"reasoning_tokens\":1}",
    "}}}\n\n"
);

#[derive(Debug, Parser)]
#[command(about = "Reproducible Arbiter M0 overhead benchmark")]
struct Args {
    #[command(subcommand)]
    command: BenchCommand,
}

#[derive(Debug, Subcommand)]
enum BenchCommand {
    Local {
        #[arg(long, default_value_t = 1_000)]
        requests: usize,
        #[arg(long, default_value_t = 50)]
        warmup: usize,
        #[arg(long, default_value_t = 2)]
        first_byte_delay_ms: u64,
        #[arg(long, default_value_t = 2)]
        chunk_interval_ms: u64,
    },
    Live {
        #[arg(long, default_value_t = 3)]
        samples: usize,
    },
}

#[derive(Debug, Clone, Copy)]
struct FakeTiming {
    first_byte_delay: Duration,
    chunk_interval: Duration,
}

#[derive(Debug, Clone, Copy)]
struct Sample {
    first_byte: Duration,
    total: Duration,
}

struct LocalHarness {
    _temporary: TempDir,
    direct_url: String,
    proxy_url: String,
    fake_shutdown: Option<oneshot::Sender<()>>,
    proxy_shutdown: Option<oneshot::Sender<()>>,
    fake_task: JoinHandle<()>,
    proxy_task: JoinHandle<()>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Args::parse().command {
        BenchCommand::Local {
            requests,
            warmup,
            first_byte_delay_ms,
            chunk_interval_ms,
        } => {
            run_local(
                requests,
                warmup,
                FakeTiming {
                    first_byte_delay: Duration::from_millis(first_byte_delay_ms),
                    chunk_interval: Duration::from_millis(chunk_interval_ms),
                },
            )
            .await
        }
        BenchCommand::Live { samples } => run_live(samples),
    }
}

async fn run_local(requests: usize, warmup: usize, timing: FakeTiming) -> anyhow::Result<()> {
    if requests == 0 {
        bail!("requests must be greater than zero");
    }
    let harness = LocalHarness::start(timing).await?;
    let client = Client::builder().build()?;
    for _ in 0..warmup {
        measure_request(&client, &harness.direct_url, false).await?;
        measure_request(&client, &harness.proxy_url, true).await?;
    }

    let mut direct = Vec::with_capacity(requests);
    let mut proxy = Vec::with_capacity(requests);
    for _ in 0..requests {
        direct.push(measure_request(&client, &harness.direct_url, false).await?);
        proxy.push(measure_request(&client, &harness.proxy_url, true).await?);
    }
    if proxy.iter().any(|sample| sample.first_byte >= sample.total) {
        bail!("proxy withheld at least one first chunk until the terminal chunk");
    }

    print_local_report(requests, warmup, timing, &direct, &proxy);
    harness.shutdown().await;
    Ok(())
}

async fn measure_request(
    client: &Client,
    url: &str,
    through_proxy: bool,
) -> anyhow::Result<Sample> {
    let started = Instant::now();
    let mut request = client.post(url).json(&json!({
        "model": "gpt-5.6-terra",
        "reasoning": {"effort": "medium"},
        "input": "local benchmark",
        "stream": true
    }));
    if through_proxy {
        request = request.header(header::AUTHORIZATION, AUTHORIZATION);
    }
    let response = request.send().await?.error_for_status()?;
    let mut stream = response.bytes_stream();
    let first = stream
        .next()
        .await
        .context("response ended before first chunk")??;
    if first.is_empty() {
        bail!("first response chunk was empty");
    }
    let first_byte = started.elapsed();
    while let Some(chunk) = stream.next().await {
        chunk?;
    }
    Ok(Sample {
        first_byte,
        total: started.elapsed(),
    })
}

fn print_local_report(
    requests: usize,
    warmup: usize,
    timing: FakeTiming,
    direct: &[Sample],
    proxy: &[Sample],
) {
    println!(
        "os={} arch={}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let build = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    println!("build={build} requests={requests} warmup={warmup}");
    println!(
        "fake_first_byte_ms={} fake_chunk_interval_ms={}",
        timing.first_byte_delay.as_millis(),
        timing.chunk_interval.as_millis()
    );
    println!("metric,path,p50_us,p95_us,p99_us");
    print_distribution(
        "first_byte",
        "direct",
        direct.iter().map(|sample| sample.first_byte),
    );
    print_distribution(
        "first_byte",
        "proxy",
        proxy.iter().map(|sample| sample.first_byte),
    );
    print_delta("first_byte", direct, proxy, |sample| sample.first_byte);
    print_distribution("total", "direct", direct.iter().map(|sample| sample.total));
    print_distribution("total", "proxy", proxy.iter().map(|sample| sample.total));
    print_delta("total", direct, proxy, |sample| sample.total);
}

fn print_distribution(metric: &str, path: &str, durations: impl Iterator<Item = Duration>) {
    let values = durations.map(duration_micros).collect::<Vec<_>>();
    println!(
        "{metric},{path},{},{},{}",
        percentile(&values, 50),
        percentile(&values, 95),
        percentile(&values, 99)
    );
}

fn print_delta(metric: &str, direct: &[Sample], proxy: &[Sample], field: fn(&Sample) -> Duration) {
    let deltas = direct
        .iter()
        .zip(proxy)
        .map(|(direct, proxy)| duration_micros(field(proxy)) - duration_micros(field(direct)))
        .collect::<Vec<_>>();
    println!(
        "{metric},added,{},{},{}",
        percentile(&deltas, 50),
        percentile(&deltas, 95),
        percentile(&deltas, 99)
    );
}

fn duration_micros(duration: Duration) -> i64 {
    i64::try_from(duration.as_micros()).unwrap_or(i64::MAX)
}

fn percentile(values: &[i64], percentile: usize) -> i64 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = percentile.saturating_mul(sorted.len()).div_ceil(100);
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

impl LocalHarness {
    async fn start(timing: FakeTiming) -> anyhow::Result<Self> {
        let fake_listener = TcpListener::bind(loopback_ephemeral()).await?;
        let fake_address = fake_listener.local_addr()?;
        let (fake_shutdown, fake_shutdown_rx) = oneshot::channel();
        let fake_router = Router::new()
            .route("/responses", post(fake_response))
            .with_state(timing);
        let fake_task = tokio::spawn(async move {
            let _ = axum::serve(fake_listener, fake_router)
                .with_graceful_shutdown(async {
                    let _ = fake_shutdown_rx.await;
                })
                .await;
        });

        let temporary = tempfile::tempdir()?;
        let store = SqliteEventStore::open(temporary.path().join("bench.db")).await?;
        let provider = CodexUpstreamProvider::new_for_loopback_test(fake_address)?;
        let proxy_listener = TcpListener::bind(loopback_ephemeral()).await?;
        let proxy_address = proxy_listener.local_addr()?;
        let (proxy_shutdown, proxy_shutdown_rx) = oneshot::channel();
        let proxy_task = tokio::spawn(async move {
            let _ = axum::serve(proxy_listener, build_router(AppState::new(provider, store)))
                .with_graceful_shutdown(async {
                    let _ = proxy_shutdown_rx.await;
                })
                .await;
        });

        Ok(Self {
            _temporary: temporary,
            direct_url: format!("http://{fake_address}/responses"),
            proxy_url: format!("http://{proxy_address}/v1/responses"),
            fake_shutdown: Some(fake_shutdown),
            proxy_shutdown: Some(proxy_shutdown),
            fake_task,
            proxy_task,
        })
    }

    async fn shutdown(mut self) {
        if let Some(sender) = self.proxy_shutdown.take() {
            let _ = sender.send(());
        }
        if let Some(sender) = self.fake_shutdown.take() {
            let _ = sender.send(());
        }
        let _ = self.proxy_task.await;
        let _ = self.fake_task.await;
    }
}

async fn fake_response(State(timing): State<FakeTiming>) -> Response<Body> {
    let chunks = [
        (timing.first_byte_delay, FIRST_EVENT),
        (timing.chunk_interval, TERMINAL_EVENT),
    ];
    let stream = stream::iter(chunks).then(|(delay, content)| async move {
        tokio::time::sleep(delay).await;
        Ok::<_, std::convert::Infallible>(Bytes::from_static(content.as_bytes()))
    });
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(stream))
        .expect("static fake response")
}

const fn loopback_ephemeral() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
}

fn run_live(samples: usize) -> anyhow::Result<()> {
    if samples == 0 {
        bail!("samples must be greater than zero");
    }
    let mut direct = Vec::with_capacity(samples);
    let mut proxy = Vec::with_capacity(samples);
    for _ in 0..samples {
        direct.push(live_ttft(false)?);
        proxy.push(live_ttft(true)?);
    }
    println!("live_samples={samples} diagnostic_only=true");
    println!("path,p50_ms,p95_ms,p99_ms");
    print_live_distribution("direct", &direct);
    print_live_distribution("proxy", &proxy);
    let deltas = direct
        .iter()
        .zip(&proxy)
        .map(|(direct, proxy)| duration_micros(*proxy) - duration_micros(*direct))
        .collect::<Vec<_>>();
    println!(
        "added,{},{},{}",
        percentile(&deltas, 50) / 1_000,
        percentile(&deltas, 95) / 1_000,
        percentile(&deltas, 99) / 1_000
    );
    Ok(())
}

fn live_ttft(through_proxy: bool) -> anyhow::Result<Duration> {
    let mut command = Command::new("codex");
    command.args([
        "exec",
        "--ephemeral",
        "--skip-git-repo-check",
        "--sandbox",
        "read-only",
        "--json",
    ]);
    if through_proxy {
        command.args(["--profile", "arbiter"]);
    } else {
        command.args([
            "--ignore-user-config",
            "--model",
            "gpt-5.6-terra",
            "--config",
            "model_reasoning_effort=\"medium\"",
        ]);
    }
    command
        .arg("Reply with exactly one short word.")
        .env_remove("OPENAI_API_KEY")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let started = Instant::now();
    let mut child = command.spawn().context("start live Codex sample")?;
    let stdout = child.stdout.take().context("capture Codex JSONL")?;
    let mut ttft = None;
    for line in BufReader::new(stdout).lines() {
        let event: Value = serde_json::from_str(&line?)?;
        if event.get("type").and_then(Value::as_str) == Some("item.completed")
            && event.pointer("/item/type").and_then(Value::as_str) == Some("agent_message")
        {
            ttft.get_or_insert_with(|| started.elapsed());
        }
    }
    let status = child.wait()?;
    if !status.success() {
        bail!("live Codex sample failed");
    }
    ttft.context("live Codex sample emitted no terminal agent message")
}

fn print_live_distribution(path: &str, durations: &[Duration]) {
    let values = durations
        .iter()
        .copied()
        .map(duration_micros)
        .collect::<Vec<_>>();
    println!(
        "{path},{},{},{}",
        percentile(&values, 50) / 1_000,
        percentile(&values, 95) / 1_000,
        percentile(&values, 99) / 1_000
    );
}

#[cfg(test)]
mod tests {
    use super::percentile;

    #[test]
    fn nearest_rank_percentiles_are_deterministic() {
        let values = (1..=100).rev().collect::<Vec<_>>();
        assert_eq!(percentile(&values, 50), 50);
        assert_eq!(percentile(&values, 95), 95);
        assert_eq!(percentile(&values, 99), 99);
    }
}
