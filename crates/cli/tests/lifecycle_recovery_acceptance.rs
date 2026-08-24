//! Real-flow acceptance checks for the Phase 2.5 Host lifecycle recovery
//! exit gate: install the Host, crash it with live work, pull the network
//! during an update, reconnect, resume the download, promote atomically,
//! restart, preserve local state, and prove an interrupted agent is
//! reported and explicitly resumable.
//!
//! Lifecycle commands (`host start`, `host status`, `host stop`) run through
//! the real CLI binary; the updater steps drive the same library the CLI
//! calls, signed with a test trust root because the production release key
//! is deliberately unavailable to tests.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{SigningKey, VerifyingKey};
use lazarus_cli::updater::layout::InstallPaths;
use lazarus_cli::updater::manifest::{Artifact, Release, ReleaseManifest, sign_manifest};
use lazarus_cli::updater::{ApplyOutcome, Updater};
use lazarus_hostd::runtime::DataPaths;
use protocol_rs::generated_registry::wire;
use protocol_rs::manifest::{MANIFEST_METADATA_KEY, host_manifest_encoded};

const TOKEN_SEED: u8 = 0x42;
const ARTIFACT_NAME: &str = "lazarus-hostd";
const PROCESS_MARKER: &str = "PHASE25-PROCESS-MARKER";

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

fn temp_data_root(tag: &str) -> DataPaths {
    let root = std::env::temp_dir().join(format!(
        "lazarus-lifecycle-{tag}-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    DataPaths::at(root)
}

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn manifest_for(key: &SigningKey, version: &str, bytes: &[u8]) -> Vec<u8> {
    let manifest = ReleaseManifest {
        schema_version: 1,
        release: Release {
            version: version.to_owned(),
            artifact: Artifact {
                file_name: ARTIFACT_NAME.to_owned(),
                size_bytes: bytes.len() as u64,
                sha256: lazarus_cli::updater::download::sha256_hex(bytes),
            },
        },
    };
    sign_manifest(&manifest, key).expect("signing")
}

/// A freshly built daemon binary: the artifact under test must actually be
/// runnable, because the lifecycle below starts it twice.
fn built_daemon_bytes() -> Vec<u8> {
    let exe = if cfg!(windows) {
        "lazarus-hostd.exe"
    } else {
        "lazarus-hostd"
    };
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for profile in ["debug", "release"] {
        let candidate = manifest_dir.join("../../target").join(profile).join(exe);
        if candidate.exists() {
            return std::fs::read(candidate).expect("read built daemon");
        }
    }
    panic!(
        "lazarus-hostd is not in the workspace target directory yet; run `cargo build --workspace` first"
    );
}

fn run_cli(root: &Path, args: &[&str]) -> Output {
    let stdout_path = root.join("cli-test.stdout");
    let stderr_path = root.join("cli-test.stderr");
    let stdout = std::fs::File::create(&stdout_path).expect("create CLI stdout capture");
    let stderr = std::fs::File::create(&stderr_path).expect("create CLI stderr capture");
    let status = Command::new(env!("CARGO_BIN_EXE_lazarus-cli"))
        .args(args)
        .env("LAZARUS_DATA_DIR", root)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .status()
        .expect("run lazarus CLI");
    Output {
        status,
        stdout: std::fs::read(&stdout_path).expect("read CLI stdout"),
        stderr: std::fs::read(&stderr_path).expect("read CLI stderr"),
    }
}

fn cli_stdout(output: &Output, context: &str) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        output.status.success(),
        "{context} failed: {}{stdout}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

/// Reserves a loopback port for a later listener.
fn reserve_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("reserve loopback port")
        .local_addr()
        .expect("local addr")
        .port()
}

/// Serves one request: the full manifest at `/manifest.json`; elsewhere the
/// artifact with `Range` support, recording every request's Range header.
/// `truncate_body_at` cuts an artifact body short without completing its
/// promised Content-Length - the wire shape of the network dying mid-update -
/// after which the socket is dropped.
fn serve_release_request(
    stream: &mut std::net::TcpStream,
    manifest: &[u8],
    artifact: &[u8],
    ranges: &Mutex<Vec<Option<String>>>,
    truncate_body_at: Option<usize>,
) -> std::io::Result<()> {
    let mut buffer = [0u8; 8192];
    let mut head_end = 0usize;
    loop {
        let read = stream.read(&mut buffer[head_end..])?;
        if read == 0 {
            break;
        }
        head_end += read;
        if buffer[..head_end]
            .windows(4)
            .any(|window| window == b"\r\n\r\n")
        {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
    let wants_manifest = head.starts_with("GET /manifest.json");
    let range = head
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("range:"))
        .map(|line| line.split_once(':').expect("header").1.trim().to_owned());
    ranges.lock().expect("range log").push(range.clone());

    let payload = if wants_manifest { manifest } else { artifact };
    let total = payload.len();
    let start = match &range {
        Some(value) => value
            .strip_prefix("bytes=")
            .and_then(|rest| rest.split('-').next())
            .and_then(|prefix| prefix.parse::<usize>().ok())
            .unwrap_or(0),
        None => 0,
    };
    let body = payload.get(start..).unwrap_or_default();
    let delivered_len = match truncate_body_at {
        Some(cut) if !wants_manifest => body.len().min(cut),
        _ => body.len(),
    };
    let response = if start == 0 {
        format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n")
    } else {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{total}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            total.saturating_sub(1),
            body.len()
        )
    };
    stream.write_all(response.as_bytes())?;
    stream.write_all(&body[..delivered_len])?;
    stream.flush()?;
    // A truncated transfer deliberately never delivers the remaining bytes:
    // dropping `stream` here IS the network failure under test.
    Ok(())
}

fn spawn_release_server(
    manifest: Vec<u8>,
    artifact: Vec<u8>,
    ranges: Arc<Mutex<Vec<Option<String>>>>,
    truncate_body_at: Option<usize>,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr: SocketAddr = listener.local_addr().expect("local addr");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ =
                serve_release_request(&mut stream, &manifest, &artifact, &ranges, truncate_body_at);
        }
    });
    format!("http://{addr}")
}

struct AuthedClient {
    inner: reqwest::Client,
    base: String,
    token: String,
}

impl AuthedClient {
    fn new(addr: &str, token: &str) -> Self {
        Self {
            inner: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("HTTP client"),
            base: format!("http://{}", addr.trim_end_matches('/')),
            token: token.to_owned(),
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.inner
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .header(MANIFEST_METADATA_KEY, host_manifest_encoded())
    }

    async fn healthy(&self) -> bool {
        matches!(
            self.request(reqwest::Method::GET, "/system/health")
                .send()
                .await,
            Ok(response) if response.status().is_success()
        )
    }
}

async fn wait_healthy(client: &AuthedClient) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if client.healthy().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("Host did not become healthy in time");
}

async fn wait_unreachable(client: &AuthedClient) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if !client.healthy().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the Host still answers health probes");
}

async fn wait_stopped(paths: &DataPaths) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(conn) = rusqlite::Connection::open(paths.database()) {
            let lifecycle = conn.query_row(
                "SELECT value FROM runtime_meta WHERE key = 'host.lifecycle'",
                [],
                |row| row.get::<_, String>(0),
            );
            if lifecycle.as_deref() == Ok("stopped") && !paths.host.join("running.json").exists() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the Host did not persist a clean stop in time");
}

#[cfg(windows)]
fn force_kill(pid: u32) {
    let output = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output()
        .expect("run taskkill");
    assert!(output.status.success(), "taskkill of pid {pid} failed");
}

#[cfg(unix)]
fn force_kill(pid: u32) {
    let output = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .output()
        .expect("run kill");
    assert!(output.status.success(), "kill -9 of pid {pid} failed");
}

/// A standards-shaped UUIDv7 without a uuid dependency: current time in the
/// top 48 bits, version 7, RFC 4122 variant, counter-mixed randomness.
fn uuid_v7() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is sane")
        .as_millis() as u64;
    let mut state = COUNTER.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed) ^ millis;
    state = state
        .wrapping_mul(0xff51_afd7_ed55_8ccd)
        .rotate_left(31)
        .wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    let mut bytes = [0u8; 16];
    bytes[..6].copy_from_slice(&millis.to_be_bytes()[2..]);
    bytes[6] = 0x70 | ((state >> 60) & 0x0f) as u8;
    bytes[7] = (state >> 52) as u8;
    bytes[8] = 0x80 | ((state >> 42) & 0x3f) as u8;
    bytes[9] = (state >> 34) as u8;
    bytes[10..].copy_from_slice(&(state << 30).to_be_bytes()[..6]);
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

fn tree_command() -> (String, Vec<String>) {
    (
        std::env::current_exe()
            .expect("acceptance test executable")
            .to_string_lossy()
            .into_owned(),
        vec![
            "--exact".into(),
            "phase25_process_helper".into(),
            "--ignored".into(),
            "--nocapture".into(),
        ],
    )
}

#[test]
#[ignore]
fn phase25_process_helper() {
    println!("{PROCESS_MARKER}");
    std::io::stdout().flush().expect("flush marker");
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn decoded_output(body: &serde_json::Value) -> String {
    let mut text = String::new();
    for frame in body["frames"].as_array().expect("output frames") {
        text.push_str(&String::from_utf8_lossy(
            &BASE64
                .decode(frame["payload"].as_str().expect("base64 payload"))
                .expect("decode"),
        ));
    }
    text
}

async fn output_page(client: &AuthedClient, process_id: &str, offset: u64) -> serde_json::Value {
    client
        .request(reqwest::Method::GET, "/process/output")
        .query(&[("processId", process_id), ("offset", &offset.to_string())])
        .send()
        .await
        .expect("output request reaches the Host")
        .json::<serde_json::Value>()
        .await
        .expect("JSON output page")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn phase25_exit_gate_end_to_end() {
    let root = temp_data_root("gate");
    root.prepare().expect("data root");
    eprintln!("phase25: bootstrap");

    // ------------------------------------------------------------------
    // Bootstrap: install v0.1.0 from a local signed manifest whose
    // artifact is a genuinely runnable daemon binary.
    // ------------------------------------------------------------------
    let daemon_bytes = Arc::new(built_daemon_bytes());
    let trust_root: VerifyingKey = key(TOKEN_SEED).verifying_key();
    let updater = Updater::new(root.clone(), trust_root);

    let releases = root.root.join("releases");
    std::fs::create_dir_all(&releases).expect("release dir");
    let v1_manifest = releases.join("v1.json");
    std::fs::write(
        &v1_manifest,
        manifest_for(&key(TOKEN_SEED), "0.1.0", &daemon_bytes),
    )
    .expect("write v1 manifest");
    std::fs::write(releases.join(ARTIFACT_NAME), &*daemon_bytes).expect("write v1 artifact");
    assert_eq!(
        updater
            .apply(v1_manifest.to_str().expect("utf8"), false)
            .await
            .expect("bootstrap install"),
        ApplyOutcome::Installed {
            version: "0.1.0".to_owned()
        }
    );

    // ------------------------------------------------------------------
    // Start through the CLI and launch one supervised agent process.
    // ------------------------------------------------------------------
    let port_one = reserve_port();
    eprintln!("phase25: start host");
    let started = run_cli(
        &root.root,
        &["host", "start", "--addr", &format!("127.0.0.1:{port_one}")],
    );
    assert!(cli_stdout(&started, "first host start").contains("Host started"));

    let token = std::fs::read_to_string(root.root.join("auth/local-token"))
        .expect("provisioned token")
        .trim()
        .to_owned();
    let client = AuthedClient::new(&format!("127.0.0.1:{port_one}"), &token);
    wait_healthy(&client).await;

    let status = run_cli(&root.root, &["host", "status"]);
    cli_stdout(&status, "host status against a live Host");
    eprintln!("phase25: start supervised process");

    let process_id = uuid_v7();
    let marker = PROCESS_MARKER;
    let (program, args) = tree_command();
    let start_response = client
        .request(reqwest::Method::POST, "/process/start")
        .json(&serde_json::json!({
            "processId": process_id,
            "program": program,
            "args": args,
            "runMode": "PIPED",
            "dataDir": "phase25"
        }))
        .send()
        .await
        .expect("start request reaches the Host");
    assert_eq!(start_response.status(), reqwest::StatusCode::OK);
    let started: wire::ProcessStartResponse = start_response.json().await.expect("start decodes");
    assert_eq!(started.status, wire::ProcessStartResponseStatus::Running);

    let deadline = Instant::now() + Duration::from_secs(10);
    let pre_crash_output = loop {
        let page = output_page(&client, &process_id, 0).await;
        if decoded_output(&page).contains(marker) {
            break page;
        }
        assert!(Instant::now() < deadline, "agent never produced output");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    // ------------------------------------------------------------------
    // Crash: kill -9 the daemon with live work in flight.
    // ------------------------------------------------------------------
    #[derive(serde::Deserialize)]
    struct PidRecord {
        pid: u32,
    }
    let pid_record: PidRecord = serde_json::from_str(
        &std::fs::read_to_string(root.host.join("pid.json")).expect("pid record"),
    )
    .expect("pid record parses");
    force_kill(pid_record.pid);
    wait_unreachable(&client).await;
    eprintln!("phase25: interrupted update");

    // ------------------------------------------------------------------
    // Update across a dead network: the transfer dies mid-download,
    // resumable partial state survives, and nothing is promoted.
    // ------------------------------------------------------------------
    let empty_ranges = Arc::new(Mutex::new(Vec::new()));
    let flaky_base = spawn_release_server(
        manifest_for(&key(TOKEN_SEED), "0.2.0", &daemon_bytes),
        (*daemon_bytes).clone(),
        Arc::clone(&empty_ranges),
        Some(daemon_bytes.len() / 2),
    );
    let error = updater
        .apply(format!("{flaky_base}/manifest.json").as_str(), false)
        .await
        .expect_err("mid-download network death fails the update");
    assert!(
        error.to_string().contains("connection dropped"),
        "the failure must name the interrupted transfer: {error:#}"
    );
    assert_eq!(
        installed_version(&root).as_deref(),
        Some("0.1.0"),
        "a failed download must never change the installed release"
    );
    let cache_dir = root.host.join("download-cache");
    let partial_bytes = std::fs::metadata(cache_dir.join(format!("{ARTIFACT_NAME}.part")))
        .expect("interrupted downloads persist resumable partial bytes")
        .len();
    assert!(
        partial_bytes > 0 && partial_bytes < daemon_bytes.len() as u64,
        "partial state sits strictly between empty and complete: {partial_bytes}"
    );
    assert!(
        cache_dir
            .join(format!("{ARTIFACT_NAME}.part.json"))
            .exists(),
        "the partial sidecar describing the resume point survives"
    );

    // ------------------------------------------------------------------
    // Reconnect: the same update against a healthy release server resumes
    // from the persisted offset and promotes atomically.
    // ------------------------------------------------------------------
    let healthy_ranges = Arc::new(Mutex::new(Vec::new()));
    eprintln!("phase25: resume update");
    let healthy_base = spawn_release_server(
        manifest_for(&key(TOKEN_SEED), "0.2.0", &daemon_bytes),
        (*daemon_bytes).clone(),
        Arc::clone(&healthy_ranges),
        None,
    );
    assert_eq!(
        updater
            .apply(format!("{healthy_base}/manifest.json").as_str(), false)
            .await
            .expect("resumed update applies"),
        ApplyOutcome::Updated {
            from_version: Some("0.1.0".to_owned()),
            version: "0.2.0".to_owned(),
        }
    );
    let expected_range = format!("bytes={partial_bytes}-");
    assert!(
        healthy_ranges
            .lock()
            .expect("ranges")
            .iter()
            .any(|range| range.as_deref() == Some(expected_range.as_str())),
        "the retry must have resumed from the persisted offset: {:?}",
        healthy_ranges.lock().unwrap()
    );
    assert_eq!(
        installed_version(&root).as_deref(),
        Some("0.2.0"),
        "the update promoted atomically"
    );
    assert_eq!(
        InstallPaths::from_data_paths(&root)
            .retained_record()
            .expect("retained readable")
            .map(|record| record.version),
        Some("0.1.0".to_owned()),
        "the replaced installation stays retained for explicit rollback"
    );
    assert!(
        !cache_dir.join(format!("{ARTIFACT_NAME}.part")).exists(),
        "a completed download clears its partial"
    );

    // ------------------------------------------------------------------
    // Restart from the newly promoted installation; the interrupted agent
    // must be reported honestly and be explicitly resumable.
    // ------------------------------------------------------------------
    let port_two = reserve_port();
    eprintln!("phase25: restart host");
    let restarted = run_cli(
        &root.root,
        &["host", "start", "--addr", &format!("127.0.0.1:{port_two}")],
    );
    cli_stdout(&restarted, "restart after update");
    let client_two = AuthedClient::new(&format!("127.0.0.1:{port_two}"), &token);
    wait_healthy(&client_two).await;

    async fn list_processes(client: &AuthedClient) -> Vec<serde_json::Value> {
        client
            .request(reqwest::Method::GET, "/process/list")
            .send()
            .await
            .expect("list request reaches the Host")
            .json::<serde_json::Value>()
            .await
            .expect("JSON process list")
            .as_array()
            .expect("array")
            .clone()
    }

    let interrupted = list_processes(&client_two)
        .await
        .into_iter()
        .find(|process| process["processId"] == process_id.as_str())
        .expect("the interrupted agent survives crash, update, and restart");
    assert_eq!(
        interrupted["status"], "INTERRUPTED",
        "interrupted work is reported honestly, never silently restarted"
    );

    // Durable audit: exactly one interruption record names why the work died.
    {
        let conn = rusqlite::Connection::open(root.database()).expect("open recovered database");
        conn.busy_timeout(Duration::from_secs(5))
            .expect("configure SQLite wait");
        let interruptions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM process_interruptions
                 WHERE process_id = ?1 AND reason = 'host died'",
                [&process_id],
                |row| row.get(0),
            )
            .expect("read interruption records");
        assert_eq!(interruptions, 1);
        let stored_started_at: String = conn
            .query_row(
                "SELECT started_at_utc FROM supervised_processes WHERE id = ?1",
                [&process_id],
                |row| row.get(0),
            )
            .expect("read preserved row");
        assert!(!stored_started_at.is_empty());
        drop(conn);
    }

    // Local state preservation: everything the agent streamed before the
    // crash replays byte-for-byte after the update and restart.
    let replayed = output_page(&client_two, &process_id, 0).await;
    assert!(
        decoded_output(&replayed).contains(marker),
        "pre-crash output history survives"
    );
    assert_eq!(
        replayed["nextOffset"].as_u64(),
        pre_crash_output["nextOffset"].as_u64(),
        "the durable replay cursor is unchanged by the crash and update"
    );

    // Explicit resume re-runs the same command line from the durable spec.
    let unknown_resume = client_two
        .request(reqwest::Method::POST, "/process/resume")
        .json(&serde_json::json!({ "processId": uuid_v7() }))
        .send()
        .await
        .expect("unknown resume reaches the Host");
    assert_eq!(unknown_resume.status(), reqwest::StatusCode::NOT_FOUND);

    let resume = client_two
        .request(reqwest::Method::POST, "/process/resume")
        .json(&serde_json::json!({ "processId": process_id }))
        .send()
        .await
        .expect("resume reaches the Host");
    assert_eq!(resume.status(), reqwest::StatusCode::OK);
    let resumed: wire::ProcessResumeResponse = resume.json().await.expect("resume decodes");
    assert_eq!(
        resumed.status,
        wire::ProcessResumeResponseStatus::Running,
        "an interrupted agent resumes when its provider supports that operation"
    );
    eprintln!("phase25: resumed process");

    // The resumed run appends fresh output to the same durable history.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let occurrences = decoded_output(&output_page(&client_two, &process_id, 0).await)
            .matches(marker)
            .count();
        if occurrences >= 2 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the resumed agent never produced output"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let resumed_row = list_processes(&client_two)
        .await
        .into_iter()
        .find(|process| process["processId"] == process_id.as_str())
        .expect("resumed row listed");
    assert_eq!(resumed_row["status"], "RUNNING");
    assert_ne!(
        resumed_row["resourceCounters"]["durationMs"],
        serde_json::json!(null),
        "the resumed run reports live resource counters"
    );

    // A running process refuses further resumes with a typed error.
    let refused = client_two
        .request(reqwest::Method::POST, "/process/resume")
        .json(&serde_json::json!({ "processId": process_id }))
        .send()
        .await
        .expect("second resume reaches the Host");
    assert_eq!(refused.status(), reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(
        refused
            .json::<serde_json::Value>()
            .await
            .expect("typed error")["code"],
        "INVALID_ARGUMENT"
    );

    // ------------------------------------------------------------------
    // Graceful stop drains cleanly through the CLI.
    // ------------------------------------------------------------------
    let stopped = run_cli(&root.root, &["host", "stop"]);
    assert!(cli_stdout(&stopped, "graceful stop").contains("Host stopped"));
    wait_unreachable(&client_two).await;
    wait_stopped(&root).await;
    eprintln!("phase25: complete");

    std::fs::remove_dir_all(&root.root).ok();
}

fn installed_version(paths: &DataPaths) -> Option<String> {
    InstallPaths::from_data_paths(paths)
        .installed_record()
        .expect("install record readable")
        .map(|record| record.version)
}
