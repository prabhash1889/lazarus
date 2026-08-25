use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use lazarus_hostd::ipc;
use lazarus_hostd::logging::init_structured_logging;
use lazarus_hostd::persistence::Store;
use lazarus_hostd::runtime::{CrashMarker, DataPaths, InstanceLock};
use lazarus_hostd::{
    HostServices, HostState, build_router, local_token_from_env, validate_loopback_addr,
};
use process_supervisor::{Supervisor, SupervisorConfig};
use tracing::{info, warn};

const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:50051";

#[tokio::main]
async fn main() -> Result<()> {
    init_structured_logging();

    let paths = DataPaths::resolve().context("resolving the Lazarus data directory")?;
    paths
        .prepare()
        .context("preparing the Lazarus data directory")?;
    let _instance_lock = InstanceLock::acquire(&paths)?;

    let listen_addr: SocketAddr = std::env::var("LAZARUS_HOST_ADDR")
        .unwrap_or_else(|_| DEFAULT_LISTEN_ADDR.to_owned())
        .parse()
        .context("invalid LAZARUS_HOST_ADDR")?;
    validate_loopback_addr(listen_addr)?;
    let token = local_token_from_env()?;

    let crash_marker = CrashMarker::begin(&paths, env!("CARGO_PKG_VERSION"))
        .context("creating the Host crash marker")?;
    let previous_unclean_shutdown = crash_marker.previous_unclean_shutdown();
    let mut store = Store::open(paths.database()).context("opening Host persistence")?;
    let supervisor = Supervisor::new(SupervisorConfig::new(paths.state.join("processes")))
        .context("starting process supervision")?;
    let previous_supervisor_unclean_shutdown = supervisor.previous_unclean_shutdown();
    let interrupted_processes = store
        .interrupt_active_processes("host died")
        .context("recovering interrupted supervised processes")?;
    store.set_meta(
        "host.previous_unclean_shutdown",
        if previous_unclean_shutdown {
            "true"
        } else {
            "false"
        },
    )?;
    store.set_meta(
        "process_supervisor.previous_unclean_shutdown",
        if previous_supervisor_unclean_shutdown {
            "true"
        } else {
            "false"
        },
    )?;

    let state = Arc::new(HostState::new());
    let store = Arc::new(Mutex::new(store));
    let services = HostServices::with_process_supervision(
        state.clone(),
        token,
        store.clone(),
        supervisor.clone(),
    );
    let app = build_router(services);

    info!(
        component = "hostd",
        event = "host.starting",
        version = env!("CARGO_PKG_VERSION"),
        data_root = %paths.root.display(),
        previous_unclean_shutdown,
        previous_supervisor_unclean_shutdown,
        interrupted_processes,
    );
    if previous_unclean_shutdown {
        warn!(
            component = "hostd",
            event = "host.startup_recovered",
            "the previous Host did not complete graceful shutdown; SQLite recovery checks passed"
        );
    }
    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .with_context(|| format!("binding {listen_addr}"))?;

    // Bind the Desktop's local IPC endpoint before publishing its discovery
    // record, so a visible record always names an accepting listener. The
    // instance lock is held, so any leftover socket file is crash debris.
    let ipc_endpoint = ipc::resolve_endpoint(&paths).map_err(|reason| anyhow!("{reason}"))?;
    ipc::IpcListener::remove_stale_socket(&ipc_endpoint);
    let ipc_listener = ipc::IpcListener::bind(&ipc_endpoint)
        .with_context(|| format!("binding the local IPC endpoint {ipc_endpoint}"))?;
    ipc::publish_endpoint_record(&paths, &ipc_endpoint)
        .context("publishing the local IPC endpoint record")?;

    store
        .lock()
        .map_err(|_| anyhow!("Host persistence lock is poisoned"))?
        .set_meta("host.lifecycle", "running")?;
    info!(
        component = "hostd",
        event = "host.listening",
        listen_addr = %listen_addr
    );

    tokio::spawn(ipc::serve_ipc(
        ipc_listener,
        app.clone(),
        state.subscribe_shutdown(),
    ));

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(state))
        .await?;

    ipc::retire_endpoint_record(&paths);

    supervisor
        .shutdown()
        .await
        .context("stopping supervised process trees")?;
    let mut store = store
        .lock()
        .map_err(|_| anyhow!("Host persistence lock is poisoned"))?;
    store
        .stop_active_processes()
        .context("finalizing supervised processes")?;
    store.set_meta("host.lifecycle", "stopped")?;
    drop(store);
    crash_marker
        .mark_clean()
        .context("removing the Host crash marker")?;
    info!(component = "hostd", event = "host.stopped");
    Ok(())
}

async fn shutdown_signal(state: Arc<HostState>) {
    tokio::select! {
        _ = wait_for_shutdown_signal() => {},
        // The authenticated lifecycle RPC requests the same drain as a
        // terminal signal; either source must stop the serve loop.
        _ = state.until_shutdown_requested() => {},
    }
    state.begin_shutdown();
    info!(component = "hostd", event = "host.draining");
}

#[cfg(unix)]
async fn wait_for_shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = terminate.recv() => {},
    }
}

#[cfg(windows)]
async fn wait_for_shutdown_signal() {
    use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, ctrl_shutdown};

    let mut ctrl_c = ctrl_c().expect("install CTRL_C handler");
    let mut ctrl_break = ctrl_break().expect("install CTRL_BREAK handler");
    let mut ctrl_close = ctrl_close().expect("install CTRL_CLOSE handler");
    let mut ctrl_shutdown = ctrl_shutdown().expect("install CTRL_SHUTDOWN handler");
    tokio::select! {
        _ = ctrl_c.recv() => {},
        _ = ctrl_break.recv() => {},
        _ = ctrl_close.recv() => {},
        _ = ctrl_shutdown.recv() => {},
    }
}

#[cfg(not(any(unix, windows)))]
async fn wait_for_shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
