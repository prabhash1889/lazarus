//! Real-process acceptance checks for the Phase 2.1 Host lifecycle.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const TOKEN: &str = "phase21-lifecycle-test-token";

fn temp_root() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    std::env::temp_dir().join(format!(
        "lazarus-hostd-lifecycle-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn spawn_host(root: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_lazarus-hostd"))
        .env("LAZARUS_DATA_DIR", root)
        .env("LAZARUS_LOCAL_TOKEN", TOKEN)
        .env("LAZARUS_HOST_ADDR", "127.0.0.1:0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn Host")
}

fn wait_until_started(child: &mut Child, root: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        assert!(child.try_wait().expect("read child status").is_none());
        if root.join("state/lazarus.sqlite3").exists() && root.join("host/running.json").exists() {
            std::thread::sleep(Duration::from_millis(100));
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("Host did not finish startup in time");
}

fn force_stop(mut child: Child) -> Output {
    child.kill().expect("stop child");
    child.wait_with_output().expect("collect child output")
}

#[test]
fn singleton_restart_recovery_and_json_logs() {
    let root = temp_root();
    let mut first = spawn_host(&root);
    wait_until_started(&mut first, &root);

    let second = spawn_host(&root)
        .wait_with_output()
        .expect("second Host exits");
    assert!(!second.status.success());
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("already owns"),
        "second instance must fail at the data-directory lock: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    let _ = force_stop(first);

    let mut restarted = spawn_host(&root);
    wait_until_started(&mut restarted, &root);
    let restarted = force_stop(restarted);
    let logs = String::from_utf8(restarted.stdout).expect("logs are UTF-8");
    assert!(logs.contains("\"previous_unclean_shutdown\":true"));
    for line in logs.lines().filter(|line| !line.is_empty()) {
        serde_json::from_str::<serde_json::Value>(line).expect("each log line is JSON");
    }

    std::fs::remove_dir_all(root).expect("remove test data root");
}
