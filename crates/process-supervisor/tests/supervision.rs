use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use process_supervisor::{
    CommandSpec, ProcessEvent, StdioMode, Supervisor, SupervisorConfig, TerminalSize,
};

fn temp_dir(tag: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    std::env::temp_dir().join(format!(
        "lazarus-process-supervisor-{tag}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn helper_spec(mode: &str) -> CommandSpec {
    CommandSpec::new(std::env::current_exe().expect("test executable path"))
        .args(["--exact", "helper_entry", "--ignored", "--nocapture"])
        .env("LAZARUS_PROCESS_SUPERVISOR_HELPER", mode)
}

#[test]
#[ignore]
// The root helper is deliberately killed with its child; it cannot wait first
// without defeating the process-tree test.
#[allow(clippy::zombie_processes)]
fn helper_entry() {
    match std::env::var("LAZARUS_PROCESS_SUPERVISOR_HELPER").as_deref() {
        Ok("output") => {
            print!("alpha\nbeta\n");
            io::stdout().flush().expect("flush stdout");
            eprintln!("error");
        }
        Ok("pty") => {
            println!("terminal-output");
            io::stdout().flush().expect("flush pty");
            std::process::exit(0);
        }
        Ok("tree-root") => {
            let pid_file =
                PathBuf::from(std::env::var_os("LAZARUS_CHILD_PID_FILE").expect("child PID file"));
            let child = Command::new(std::env::current_exe().expect("test executable path"))
                .args(["--exact", "helper_entry", "--ignored", "--nocapture"])
                .env("LAZARUS_PROCESS_SUPERVISOR_HELPER", "tree-child")
                .spawn()
                .expect("spawn tree child");
            fs::write(pid_file, child.id().to_string()).expect("write child PID");
            loop {
                thread::sleep(Duration::from_secs(1));
            }
        }
        Ok("tree-child" | "long") => loop {
            thread::sleep(Duration::from_secs(1));
        },
        mode => panic!("unknown helper mode: {mode:?}"),
    }
}

#[tokio::test]
async fn captures_replays_counts_and_starts_a_pty() {
    let data_dir = temp_dir("output");
    let mut config = SupervisorConfig::new(&data_dir);
    config.spool_bytes_per_process = 12;
    config.stop_timeout = Duration::from_millis(100);
    let supervisor = Supervisor::new(config).expect("create supervisor");

    let piped = supervisor
        .start("piped", helper_spec("output"))
        .await
        .expect("start piped helper");
    let exit = piped.wait().await;
    assert_eq!(exit.code, Some(0));
    let counters = piped.counters();
    assert!(counters.stdout_bytes >= b"alpha\nbeta\n".len() as u64);
    assert!(counters.stderr_bytes >= b"error\n".len() as u64);
    assert!(counters.wall_time > Duration::ZERO);
    assert_eq!(counters.exit, Some(exit));

    let replay = piped.replay(0);
    assert!(replay.was_truncated());
    assert!(replay.dropped_frames > 0);
    assert!(replay.dropped_bytes > 0);
    assert!(
        replay
            .frames
            .iter()
            .any(|frame| matches!(frame.event, ProcessEvent::Exited { .. }))
    );
    let retained_output: usize = replay
        .frames
        .iter()
        .map(|frame| match &frame.event {
            ProcessEvent::Output { bytes, .. } => bytes.len(),
            _ => 0,
        })
        .sum();
    assert!(retained_output <= 12);

    let pty = supervisor
        .start(
            "pty",
            helper_spec("long").pty(TerminalSize {
                rows: 30,
                cols: 100,
            }),
        )
        .await
        .expect("start PTY helper");
    assert!(matches!(
        supervisor.get("pty").expect("registered").replay(0).frames[0].event,
        ProcessEvent::Started { .. }
    ));
    supervisor.stop("pty").await.expect("stop PTY helper");
    assert!(!pty.is_running());
    assert_eq!(pty.counters().stdout_bytes, 0);
    assert_eq!(pty.counters().stderr_bytes, 0);
    assert!(matches!(helper_spec("output").stdio, StdioMode::Piped));

    supervisor.shutdown().await.expect("clean shutdown");
    fs::remove_dir_all(data_dir).expect("cleanup");
}

#[tokio::test]
async fn stop_terminates_the_descendant_process_tree() {
    let data_dir = temp_dir("tree");
    let pid_file = data_dir.join("child.pid");
    fs::create_dir_all(&data_dir).expect("create test directory");
    let mut config = SupervisorConfig::new(&data_dir);
    config.stop_timeout = Duration::from_millis(100);
    let supervisor = Supervisor::new(config).expect("create supervisor");
    let root = supervisor
        .start(
            "tree",
            helper_spec("tree-root").env("LAZARUS_CHILD_PID_FILE", pid_file.as_os_str().to_owned()),
        )
        .await
        .expect("start process tree");

    wait_for_file(&pid_file).await;
    let child_pid: u32 = fs::read_to_string(&pid_file)
        .expect("read child PID")
        .parse()
        .expect("parse child PID");
    assert!(process_is_alive(child_pid));

    supervisor.stop("tree").await.expect("stop process tree");
    assert!(!root.is_running());
    wait_until_dead(child_pid).await;

    supervisor.shutdown().await.expect("clean shutdown");
    fs::remove_dir_all(data_dir).expect("cleanup");
}

#[tokio::test]
async fn stop_with_timeout_overrides_the_configured_default() {
    let data_dir = temp_dir("stop-override");
    let supervisor = Supervisor::new(SupervisorConfig::new(&data_dir)).expect("create supervisor");
    supervisor
        .start("override", helper_spec("long"))
        .await
        .expect("start helper");

    let started = Instant::now();
    supervisor
        .stop_with_timeout("override", Some(Duration::from_millis(50)))
        .await
        .expect("stop helper with override");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "per-call stop timeout did not override the three-second default"
    );

    supervisor.shutdown().await.expect("clean shutdown");
    fs::remove_dir_all(data_dir).expect("cleanup");
}

#[tokio::test]
async fn stop_uses_the_configured_timeout() {
    let data_dir = temp_dir("stop-config");
    let mut config = SupervisorConfig::new(&data_dir);
    config.stop_timeout = Duration::from_millis(100);
    let supervisor = Supervisor::new(config).expect("create supervisor");
    supervisor
        .start("configured", helper_spec("long"))
        .await
        .expect("start helper");

    let started = Instant::now();
    supervisor.stop("configured").await.expect("stop helper");
    let elapsed = started.elapsed();
    // Windows has no provider-neutral graceful signal, so it waits out the
    // configured grace period. Unix sends SIGTERM and may exit immediately.
    #[cfg(windows)]
    assert!(elapsed >= Duration::from_millis(75));
    assert!(elapsed < Duration::from_secs(1));

    supervisor.shutdown().await.expect("clean shutdown");
    fs::remove_dir_all(data_dir).expect("cleanup");
}

#[tokio::test]
async fn abrupt_drop_keeps_marker_and_calls_interruption_hook() {
    let data_dir = temp_dir("marker");
    let records = Arc::new(Mutex::new(Vec::new()));
    let mut config = SupervisorConfig::new(&data_dir);
    config.stop_timeout = Duration::from_millis(50);
    config.interruption_hook = Some({
        let records = Arc::clone(&records);
        Arc::new(move |record| {
            records
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(record);
        })
    });
    let supervisor = Supervisor::new(config).expect("create supervisor");
    let process = supervisor
        .start("interrupted", helper_spec("long"))
        .await
        .expect("start helper");
    let marker = supervisor.marker_path().to_owned();
    assert!(marker.exists());

    drop(supervisor);
    let interrupted = records
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    assert_eq!(interrupted.len(), 1);
    assert_eq!(interrupted[0].id, "interrupted");
    assert_eq!(interrupted[0].pid, process.pid());
    process.wait().await;
    assert!(marker.exists());

    let recovered = Supervisor::new(SupervisorConfig::new(&data_dir)).expect("recover marker");
    assert!(recovered.previous_unclean_shutdown());
    assert_eq!(recovered.previous_interruption_records().len(), 1);
    assert_eq!(
        recovered.previous_interruption_records()[0].id,
        "interrupted"
    );
    recovered.shutdown().await.expect("acknowledge recovery");
    fs::remove_dir_all(data_dir).expect("cleanup");
}

async fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(Instant::now() < deadline, "timed out waiting for {path:?}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_until_dead(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while process_is_alive(pid) {
        assert!(Instant::now() < deadline, "PID {pid} survived tree stop");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .expect("run tasklist");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success())
}
