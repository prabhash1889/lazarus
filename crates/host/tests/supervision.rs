//! Real-process acceptance checks for Phase 2.2 Host supervision.

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use protocol_rs::manifest::{MANIFEST_METADATA_KEY, host_manifest_encoded};
use reqwest::{Client, Response};
use serde_json::{Value, json};

const TOKEN: &str = "phase22-supervision-test-token";
const FIRST_ID: &str = "0198e550-c9be-7000-8000-000000000010";
const CRASH_ID: &str = "0198e550-c9be-7000-8000-000000000011";
const DEFAULT_TIMEOUT_ID: &str = "0198e550-c9be-7000-8000-000000000012";
const HUGE_OUTPUT_BYTES: u64 = 9 * 1024 * 1024;

fn temp_root() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    std::env::temp_dir().join(format!(
        "lazarus-hostd-supervision-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn available_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve loopback port");
    listener.local_addr().expect("read reserved port")
}

fn spawn_host(root: &Path, addr: SocketAddr) -> Child {
    Command::new(env!("CARGO_BIN_EXE_lazarus-hostd"))
        .env("LAZARUS_DATA_DIR", root)
        .env("LAZARUS_LOCAL_TOKEN", TOKEN)
        .env("LAZARUS_HOST_ADDR", addr.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn Host")
}

fn request(
    client: &Client,
    method: reqwest::Method,
    addr: SocketAddr,
    path: &str,
) -> reqwest::RequestBuilder {
    client
        .request(method, format!("http://{addr}{path}"))
        .bearer_auth(TOKEN)
        .header(MANIFEST_METADATA_KEY, host_manifest_encoded())
}

async fn wait_for_host(client: &Client, child: &mut Child, addr: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        assert!(child.try_wait().expect("read Host status").is_none());
        if let Ok(response) = request(client, reqwest::Method::GET, addr, "/system/health")
            .send()
            .await
            && response.status().is_success()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("Host did not become healthy in time");
}

async fn response_json(response: Response) -> Value {
    let status = response.status();
    let body = response.text().await.expect("read response body");
    assert!(status.is_success(), "HTTP {status}: {body}");
    serde_json::from_str(&body).expect("JSON response")
}

#[cfg(windows)]
fn tree_command(huge_output: bool) -> (&'static str, Vec<String>) {
    let output = if huge_output {
        format!(
            "Start-Sleep -Milliseconds 1200; [Console]::Out.WriteLine(('x' * {HUGE_OUTPUT_BYTES}));"
        )
    } else {
        String::new()
    };
    let script = format!(
        "$child = Start-Process -FilePath powershell.exe -ArgumentList @('-NoProfile','-Command','Start-Sleep -Seconds 120') -PassThru; \
         Write-Output \"PARENT=$PID\"; Write-Output \"CHILD=$($child.Id)\"; {output} \
         while ($true) {{ Start-Sleep -Seconds 1 }}"
    );
    (
        "powershell.exe",
        vec!["-NoProfile".into(), "-Command".into(), script],
    )
}

#[cfg(unix)]
fn tree_command(huge_output: bool) -> (&'static str, Vec<String>) {
    let output = if huge_output {
        format!("sleep 1; head -c {HUGE_OUTPUT_BYTES} /dev/zero | tr '\\0' x; printf '\\n';")
    } else {
        String::new()
    };
    (
        "sh",
        vec![
            "-c".into(),
            format!(
                "sleep 120 & child=$!; echo PARENT=$$; echo CHILD=$child; {output} while true; do sleep 1; done"
            ),
        ],
    )
}

async fn start_tree(client: &Client, addr: SocketAddr, process_id: &str, huge_output: bool) {
    let (program, args) = tree_command(huge_output);
    let response = request(client, reqwest::Method::POST, addr, "/process/start")
        .json(&json!({
            "processId": process_id,
            "program": program,
            "args": args,
            "runMode": "PIPED",
            "dataDir": "phase22-test"
        }))
        .send()
        .await
        .expect("start request");
    let body = response_json(response).await;
    assert_eq!(body["status"], "RUNNING");
}

async fn output(client: &Client, addr: SocketAddr, process_id: &str, offset: u64) -> Value {
    let response = request(client, reqwest::Method::GET, addr, "/process/output")
        .query(&[("processId", process_id), ("offset", &offset.to_string())])
        .send()
        .await
        .expect("output request");
    response_json(response).await
}

async fn wait_for_process_ids(client: &Client, addr: SocketAddr, process_id: &str) -> (u32, u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let body = output(client, addr, process_id, 0).await;
        let text = decoded_output(&body);
        let parent = parse_pid(&text, "PARENT=");
        let child = parse_pid(&text, "CHILD=");
        if let (Some(parent), Some(child)) = (parent, child) {
            return (parent, child);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("process IDs were not replayed before the large bounded frame");
}

fn decoded_output(body: &Value) -> String {
    let mut text = String::new();
    for frame in body["frames"].as_array().expect("output frames") {
        let payload = frame["payload"].as_str().expect("base64 payload");
        text.push_str(&String::from_utf8_lossy(
            &BASE64.decode(payload).expect("decode output payload"),
        ));
    }
    text
}

fn parse_pid(text: &str, prefix: &str) -> Option<u32> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix(prefix)?.parse().ok())
}

async fn wait_for_truncation(client: &Client, addr: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let body = output(client, addr, FIRST_ID, 0).await;
        if body["truncated"] == true {
            assert!(body["nextOffset"].as_u64().unwrap_or(0) > 0);
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("bounded durable replay never reported truncation");
}

async fn list_processes(client: &Client, addr: SocketAddr) -> Value {
    response_json(
        request(client, reqwest::Method::GET, addr, "/process/list")
            .send()
            .await
            .expect("list request"),
    )
    .await
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    let output = Command::new("tasklist.exe")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .expect("query process table");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success())
}

async fn assert_process_dies(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if !process_is_alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("process {pid} survived tree termination");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn phase22_exit_gate_end_to_end() {
    let root = temp_root();
    let addr = available_addr();
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("HTTP client");
    let mut host = spawn_host(&root, addr);
    wait_for_host(&client, &mut host, addr).await;

    let unauthorized = client
        .post(format!("http://{addr}/process/start"))
        .send()
        .await
        .expect("unauthenticated request");
    assert_eq!(unauthorized.status(), reqwest::StatusCode::UNAUTHORIZED);

    start_tree(&client, addr, FIRST_ID, true).await;
    let (parent_pid, child_pid) = wait_for_process_ids(&client, addr, FIRST_ID).await;
    let replayed = decoded_output(&output(&client, addr, FIRST_ID, 0).await);
    assert!(replayed.contains(&format!("PARENT={parent_pid}")));
    assert!(replayed.contains(&format!("CHILD={child_pid}")));
    wait_for_truncation(&client, addr).await;

    let listed = list_processes(&client, addr).await;
    let first = listed
        .as_array()
        .expect("process list")
        .iter()
        .find(|process| process["processId"] == FIRST_ID)
        .expect("started process is listed");
    assert_eq!(first["status"], "RUNNING");
    assert!(first["resourceCounters"]["durationMs"].as_u64().is_some());
    assert!(
        first["resourceCounters"]["stdoutBytes"]
            .as_u64()
            .unwrap_or(0)
            >= HUGE_OUTPUT_BYTES
    );
    assert!(first["droppedOutputBytes"].as_u64().unwrap_or(0) >= HUGE_OUTPUT_BYTES);

    let stop_started = Instant::now();
    let stopped = response_json(
        request(&client, reqwest::Method::POST, addr, "/process/stop")
            .json(&json!({"processId": FIRST_ID, "gracefulTimeoutMs": 100}))
            .send()
            .await
            .expect("stop request"),
    )
    .await;
    assert_eq!(stopped["status"], "STOPPED");
    assert!(
        stop_started.elapsed() < Duration::from_millis(1_100),
        "100 ms graceful stop used the configured three-second default"
    );
    assert_process_dies(parent_pid).await;
    assert_process_dies(child_pid).await;

    for invalid_timeout in [0, 300_001] {
        let rejected = request(&client, reqwest::Method::POST, addr, "/process/stop")
            .json(&json!({
                "processId": FIRST_ID,
                "gracefulTimeoutMs": invalid_timeout
            }))
            .send()
            .await
            .expect("invalid stop request");
        assert_eq!(rejected.status(), reqwest::StatusCode::BAD_REQUEST);
        assert_eq!(
            rejected.json::<Value>().await.expect("typed error")["code"],
            "INVALID_ARGUMENT"
        );
    }

    start_tree(&client, addr, DEFAULT_TIMEOUT_ID, false).await;
    let (default_parent, default_child) =
        wait_for_process_ids(&client, addr, DEFAULT_TIMEOUT_ID).await;
    let stopped = response_json(
        request(&client, reqwest::Method::POST, addr, "/process/stop")
            .json(&json!({"processId": DEFAULT_TIMEOUT_ID}))
            .send()
            .await
            .expect("default stop request"),
    )
    .await;
    assert_eq!(stopped["status"], "STOPPED");
    assert_process_dies(default_parent).await;
    assert_process_dies(default_child).await;

    start_tree(&client, addr, CRASH_ID, false).await;
    host.kill().expect("kill Host without graceful shutdown");
    host.wait().expect("reap killed Host");

    let mut restarted = spawn_host(&root, addr);
    wait_for_host(&client, &mut restarted, addr).await;
    let listed = list_processes(&client, addr).await;
    let interrupted = listed
        .as_array()
        .expect("process list")
        .iter()
        .find(|process| process["processId"] == CRASH_ID)
        .expect("crashed process is listed");
    assert_eq!(interrupted["status"], "INTERRUPTED");

    let conn = rusqlite::Connection::open(root.join("state/lazarus.sqlite3"))
        .expect("open recovered database");
    let interruption_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM process_interruptions
             WHERE process_id = ?1 AND reason = 'host died'",
            [CRASH_ID],
            |row| row.get(0),
        )
        .expect("read interruption record");
    assert_eq!(interruption_count, 1);
    drop(conn);

    restarted.kill().expect("stop restarted Host");
    restarted.wait().expect("reap restarted Host");
    std::fs::remove_dir_all(root).expect("remove test data root");
}
