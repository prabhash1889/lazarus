//! Authenticated lifecycle control: `POST /system/shutdown` drains the
//! Host through the same graceful path as a terminal signal.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use lazarus_hostd::{HostServices, HostState};
use protocol_rs::auth::{self, bearer_header};
use protocol_rs::deadline::{self, DEFAULT_RPC_BUDGET_MS, Deadline};
use protocol_rs::manifest::{MANIFEST_METADATA_KEY, host_manifest_encoded};

const TOKEN: &str = "shutdown-test-token";

async fn spawn_host() -> (SocketAddr, Arc<HostState>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let state = Arc::new(HostState::new());
    let services = HostServices::new(state.clone(), Arc::from(TOKEN));
    let app = lazarus_hostd::build_router(services);
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("host server runs");
    });
    (addr, state)
}

fn post_shutdown(token: Option<&str>, addr: SocketAddr) -> reqwest::RequestBuilder {
    let client = reqwest::Client::new();
    let mut request = client.post(format!("http://{addr}/system/shutdown"));
    if let Some(token) = token {
        request = request.header(auth::AUTH_METADATA_KEY, bearer_header(token));
    }
    request
        .header(MANIFEST_METADATA_KEY, host_manifest_encoded())
        .header(
            deadline::DEADLINE_HEADER,
            Deadline::header_from_budget(deadline::unix_now_ms(), DEFAULT_RPC_BUDGET_MS),
        )
}

async fn wait_until_not_serving(state: &HostState) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while state.is_serving() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!state.is_serving(), "Host must leave the serving state");
}

#[tokio::test]
async fn shutdown_refuses_unauthenticated_callers() {
    let (addr, state) = spawn_host().await;
    let response = post_shutdown(None, addr)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("request completes");
    assert_eq!(response.status().as_u16(), 401);
    assert!(state.is_serving());
}

#[tokio::test]
async fn authenticated_shutdown_flips_serving_off_and_reports_its_ack() {
    let (addr, state) = spawn_host().await;

    let response = post_shutdown(Some(TOKEN), addr)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("request completes");
    assert_eq!(response.status().as_u16(), 200);
    let body: serde_json::Value = response.json().await.expect("JSON ack");
    assert_eq!(body["status"], "SHUTDOWN_REQUESTED");

    wait_until_not_serving(&state).await;

    let client = reqwest::Client::new();
    let health = client
        .get(format!("http://{addr}/system/health"))
        .header(auth::AUTH_METADATA_KEY, bearer_header(TOKEN))
        .header(MANIFEST_METADATA_KEY, host_manifest_encoded())
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("health still answers during drain");
    let health: serde_json::Value = health.json().await.expect("health JSON");
    assert_eq!(health["status"], "NOT_SERVING");
}
