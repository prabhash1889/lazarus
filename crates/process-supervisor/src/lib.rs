//! Tokio-friendly ownership of local process trees, including bounded output
//! replay and crash evidence for a future Host instance.

#![deny(unsafe_code)]

mod spool;
// Process groups and Windows Job Objects require small OS FFI calls. Keeping
// the exception here prevents unsafe code from spreading into core logic.
#[allow(unsafe_code)]
mod platform;

use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use portable_pty::{CommandBuilder, MasterPty};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};

use platform::ProcessTree;
use spool::Spool;

/// Which OS stream produced an output frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
    /// PTYs expose one terminal stream rather than separate stdout/stderr.
    Pty,
}

/// One lifecycle or line-oriented output event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessEvent {
    Started {
        pid: u32,
    },
    Output {
        stream: OutputStream,
        bytes: Vec<u8>,
    },
    Exited {
        code: Option<i32>,
        signal: Option<String>,
    },
}

/// An event with a process-local, monotonically increasing replay offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FramedEvent {
    pub offset: u64,
    pub event: ProcessEvent,
}

impl FramedEvent {
    fn output_len(&self) -> usize {
        match &self.event {
            ProcessEvent::Output { bytes, .. } => bytes.len(),
            ProcessEvent::Started { .. } | ProcessEvent::Exited { .. } => 0,
        }
    }
}

/// Replay result. A requested offset below `oldest_offset` exposes a spool gap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replay {
    pub requested_offset: u64,
    pub oldest_offset: u64,
    pub next_offset: u64,
    pub dropped_frames: u64,
    pub dropped_bytes: u64,
    pub frames: Vec<FramedEvent>,
}

impl Replay {
    pub fn was_truncated(&self) -> bool {
        self.requested_offset < self.oldest_offset
    }
}

/// Final or running exit information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessExit {
    pub code: Option<i32>,
    pub signal: Option<String>,
}

/// Cheap counters owned by the supervision boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceCounters {
    pub wall_time: Duration,
    pub exit: Option<ProcessExit>,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub pty_bytes: u64,
    /// Job-wide user + kernel time on Windows; unavailable on Unix.
    pub cpu_time: Option<Duration>,
    /// Peak Job Object memory on Windows; unavailable on Unix.
    pub peak_memory_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub rows: u16,
    pub cols: u16,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self { rows: 24, cols: 80 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdioMode {
    Piped,
    Pty(TerminalSize),
}

/// An OS command with no shell interpolation.
#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(OsString, OsString)>,
    pub stdio: StdioMode,
}

impl CommandSpec {
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            stdio: StdioMode::Piped,
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn pty(mut self, size: TerminalSize) -> Self {
        self.stdio = StdioMode::Pty(size);
        self
    }
}

/// Persistable evidence emitted when a Supervisor disappears with live work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterruptionRecord {
    pub id: String,
    pub pid: u32,
    pub started_unix_ms: u64,
}

pub type InterruptionHook = Arc<dyn Fn(InterruptionRecord) + Send + Sync>;

pub struct SupervisorConfig {
    pub data_dir: PathBuf,
    pub spool_bytes_per_process: usize,
    pub event_channel_capacity: usize,
    pub stop_timeout: Duration,
    pub interruption_hook: Option<InterruptionHook>,
}

impl SupervisorConfig {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            spool_bytes_per_process: 1024 * 1024,
            event_channel_capacity: 256,
            stop_timeout: Duration::from_secs(3),
            interruption_hook: None,
        }
    }
}

#[derive(Debug)]
pub enum SupervisorError {
    DuplicateId(String),
    NotFound(String),
    Io(io::Error),
}

impl fmt::Display for SupervisorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateId(id) => write!(f, "a supervised process named {id:?} already exists"),
            Self::NotFound(id) => write!(f, "no supervised process named {id:?}"),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for SupervisorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::DuplicateId(_) | Self::NotFound(_) => None,
        }
    }
}

impl From<io::Error> for SupervisorError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

enum ProcessInput {
    Piped(Mutex<Option<ChildStdin>>),
    Pty(Mutex<Option<PtyInput>>),
}

struct PtyInput {
    master: Box<dyn MasterPty + Send>,
    writer: Option<Box<dyn Write + Send>>,
}

struct ProcessState {
    id: String,
    pid: u32,
    started: Instant,
    started_unix_ms: u64,
    tree: Arc<ProcessTree>,
    input: ProcessInput,
    spool: Mutex<Spool>,
    events: broadcast::Sender<FramedEvent>,
    exit: watch::Sender<Option<ProcessExit>>,
    finished: AtomicBool,
    stdout_bytes: AtomicU64,
    stderr_bytes: AtomicU64,
    pty_bytes: AtomicU64,
}

impl ProcessState {
    fn new(
        id: String,
        pid: u32,
        tree: Arc<ProcessTree>,
        input: ProcessInput,
        spool_bytes: usize,
        event_capacity: usize,
    ) -> Arc<Self> {
        let (events, _) = broadcast::channel(event_capacity.max(1));
        let (exit, _) = watch::channel(None);
        let state = Arc::new(Self {
            id,
            pid,
            started: Instant::now(),
            started_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            tree,
            input,
            spool: Mutex::new(Spool::new(spool_bytes)),
            events,
            exit,
            finished: AtomicBool::new(false),
            stdout_bytes: AtomicU64::new(0),
            stderr_bytes: AtomicU64::new(0),
            pty_bytes: AtomicU64::new(0),
        });
        state.append(ProcessEvent::Started { pid });
        state
    }

    fn append(&self, event: ProcessEvent) {
        let frame = self
            .spool
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(event);
        let _ = self.events.send(frame);
    }

    fn output(&self, stream: OutputStream, bytes: Vec<u8>) {
        let counter = match stream {
            OutputStream::Stdout => &self.stdout_bytes,
            OutputStream::Stderr => &self.stderr_bytes,
            OutputStream::Pty => &self.pty_bytes,
        };
        counter.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        self.append(ProcessEvent::Output { stream, bytes });
    }

    fn finish(&self, exit: ProcessExit) {
        self.append(ProcessEvent::Exited {
            code: exit.code,
            signal: exit.signal.clone(),
        });
        self.finished.store(true, Ordering::Release);
        self.exit.send_replace(Some(exit));
    }

    fn interruption_record(&self) -> InterruptionRecord {
        InterruptionRecord {
            id: self.id.clone(),
            pid: self.pid,
            started_unix_ms: self.started_unix_ms,
        }
    }

    fn close_io_after_exit(&self) {
        match &self.input {
            ProcessInput::Piped(stdin) => {
                stdin
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .take();
            }
            ProcessInput::Pty(pty) => {
                pty.lock().unwrap_or_else(|error| error.into_inner()).take();
            }
        }
    }
}

/// A cloneable view of one supervised command.
#[derive(Clone)]
pub struct ProcessHandle {
    state: Arc<ProcessState>,
}

impl ProcessHandle {
    pub fn id(&self) -> &str {
        &self.state.id
    }

    pub fn pid(&self) -> u32 {
        self.state.pid
    }

    pub fn is_running(&self) -> bool {
        !self.state.finished.load(Ordering::Acquire)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<FramedEvent> {
        self.state.events.subscribe()
    }

    pub fn replay(&self, offset: u64) -> Replay {
        self.state
            .spool
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .replay(offset)
    }

    pub fn counters(&self) -> ResourceCounters {
        let platform = self.state.tree.counters();
        ResourceCounters {
            wall_time: self.state.started.elapsed(),
            exit: self.state.exit.borrow().clone(),
            stdout_bytes: self.state.stdout_bytes.load(Ordering::Relaxed),
            stderr_bytes: self.state.stderr_bytes.load(Ordering::Relaxed),
            pty_bytes: self.state.pty_bytes.load(Ordering::Relaxed),
            cpu_time: platform.cpu_time,
            peak_memory_bytes: platform.peak_memory_bytes,
        }
    }

    pub async fn wait(&self) -> ProcessExit {
        let mut exit = self.state.exit.subscribe();
        loop {
            if let Some(status) = exit.borrow_and_update().clone() {
                return status;
            }
            exit.changed()
                .await
                .expect("process state owns the exit sender");
        }
    }

    pub fn write_stdin(&self, bytes: &[u8]) -> io::Result<()> {
        match &self.state.input {
            ProcessInput::Piped(stdin) => {
                let mut stdin = stdin.lock().unwrap_or_else(|error| error.into_inner());
                let stdin = stdin
                    .as_mut()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "stdin is closed"))?;
                stdin.write_all(bytes)?;
                stdin.flush()
            }
            ProcessInput::Pty(pty) => {
                let mut pty = pty.lock().unwrap_or_else(|error| error.into_inner());
                let writer = pty
                    .as_mut()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "PTY is closed"))?
                    .writer
                    .as_mut()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "stdin is closed"))?;
                writer.write_all(bytes)?;
                writer.flush()
            }
        }
    }

    /// Closes the command's stdin/terminal input without stopping the process.
    pub fn close_stdin(&self) {
        match &self.state.input {
            ProcessInput::Piped(stdin) => {
                stdin
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .take();
            }
            ProcessInput::Pty(pty) => {
                if let Some(pty) = pty
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .as_mut()
                {
                    pty.writer.take();
                }
            }
        }
    }

    pub fn resize_pty(&self, size: TerminalSize) -> io::Result<()> {
        let ProcessInput::Pty(pty) = &self.state.input else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "process was not started in PTY mode",
            ));
        };
        let pty = pty.lock().unwrap_or_else(|error| error.into_inner());
        pty.as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "PTY is closed"))?
            .master
            .resize(portable_size(size))
            .map_err(|error| io::Error::other(error.to_string()))
    }
}

#[derive(Debug, Clone)]
pub struct ProcessSummary {
    pub id: String,
    pub pid: u32,
    pub running: bool,
    pub counters: ResourceCounters,
}

/// Registry intended to be owned by hostd and shared across Tokio handlers.
///
/// On Windows, piped children are attached to their Job Object before they are
/// resumed. `portable-pty` 0.9 exposes neither creation flags nor a primary
/// thread hook, so PTY Job assignment is best-effort after spawn. The platform
/// module seam allows a custom ConPTY backend to replace it without API churn.
#[derive(Clone)]
pub struct Supervisor {
    inner: Arc<SupervisorInner>,
}

struct SupervisorInner {
    processes: RwLock<HashMap<String, ProcessHandle>>,
    start_lock: Mutex<()>,
    marker_lock: Mutex<()>,
    marker_path: PathBuf,
    previous_unclean_shutdown: bool,
    previous_records: Vec<InterruptionRecord>,
    spool_bytes: usize,
    event_capacity: usize,
    stop_timeout: Duration,
    hook: Option<InterruptionHook>,
    clean_shutdown: AtomicBool,
}

impl Supervisor {
    pub fn new(config: SupervisorConfig) -> io::Result<Self> {
        fs::create_dir_all(&config.data_dir)?;
        let marker_path = config.data_dir.join("process-supervisor-running.json");
        let previous_unclean_shutdown = marker_path.exists();
        let previous_records = if previous_unclean_shutdown {
            File::open(&marker_path)
                .ok()
                .and_then(|file| serde_json::from_reader(file).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        Ok(Self {
            inner: Arc::new(SupervisorInner {
                processes: RwLock::new(HashMap::new()),
                start_lock: Mutex::new(()),
                marker_lock: Mutex::new(()),
                marker_path,
                previous_unclean_shutdown,
                previous_records,
                spool_bytes: config.spool_bytes_per_process,
                event_capacity: config.event_channel_capacity,
                stop_timeout: config.stop_timeout,
                hook: config.interruption_hook,
                clean_shutdown: AtomicBool::new(false),
            }),
        })
    }

    pub fn previous_unclean_shutdown(&self) -> bool {
        self.inner.previous_unclean_shutdown
    }

    pub fn previous_interruption_records(&self) -> &[InterruptionRecord] {
        &self.inner.previous_records
    }

    pub fn marker_path(&self) -> &Path {
        &self.inner.marker_path
    }

    pub async fn start(
        &self,
        id: impl Into<String>,
        spec: CommandSpec,
    ) -> Result<ProcessHandle, SupervisorError> {
        let id = id.into();
        let _start = self
            .inner
            .start_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if self
            .inner
            .processes
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(&id)
        {
            return Err(SupervisorError::DuplicateId(id));
        }

        let spawned = spawn_process(
            id.clone(),
            spec,
            self.inner.spool_bytes,
            self.inner.event_capacity,
        )?;
        let handle = ProcessHandle {
            state: Arc::clone(&spawned.state),
        };
        self.inner
            .processes
            .write()
            .unwrap_or_else(|error| error.into_inner())
            .insert(id.clone(), handle.clone());
        if let Err(error) = self.inner.update_marker() {
            self.inner
                .processes
                .write()
                .unwrap_or_else(|poison| poison.into_inner())
                .remove(&id);
            let _ = spawned.state.tree.terminate();
            spawn_waiter(spawned, Arc::downgrade(&self.inner));
            return Err(error.into());
        }
        spawn_waiter(spawned, Arc::downgrade(&self.inner));
        Ok(handle)
    }

    pub fn list(&self) -> Vec<ProcessSummary> {
        let mut processes: Vec<_> = self
            .inner
            .processes
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .map(|handle| ProcessSummary {
                id: handle.id().to_owned(),
                pid: handle.pid(),
                running: handle.is_running(),
                counters: handle.counters(),
            })
            .collect();
        processes.sort_by(|left, right| left.id.cmp(&right.id));
        processes
    }

    pub fn get(&self, id: &str) -> Option<ProcessHandle> {
        self.inner
            .processes
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .get(id)
            .cloned()
    }

    pub async fn stop(&self, id: &str) -> Result<ProcessExit, SupervisorError> {
        self.stop_with_timeout(id, None).await
    }

    pub async fn stop_with_timeout(
        &self,
        id: &str,
        grace: Option<Duration>,
    ) -> Result<ProcessExit, SupervisorError> {
        let handle = self
            .get(id)
            .ok_or_else(|| SupervisorError::NotFound(id.to_owned()))?;
        if !handle.is_running() {
            return Ok(handle.wait().await);
        }
        // EOF is the only graceful request shared by pipes and ConPTY. Unix
        // additionally signals the full process group below.
        handle.close_stdin();
        handle.state.tree.graceful()?;
        if let Ok(exit) =
            tokio::time::timeout(grace.unwrap_or(self.inner.stop_timeout), handle.wait()).await
        {
            return Ok(exit);
        }
        handle.state.tree.terminate()?;
        Ok(handle.wait().await)
    }

    pub async fn await_exit(&self, id: &str) -> Result<ProcessExit, SupervisorError> {
        let handle = self
            .get(id)
            .ok_or_else(|| SupervisorError::NotFound(id.to_owned()))?;
        Ok(handle.wait().await)
    }

    /// Stops all live trees and removes the marker only after every exit is observed.
    pub async fn shutdown(&self) -> Result<(), SupervisorError> {
        let ids: Vec<_> = self
            .inner
            .processes
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .filter(|handle| handle.is_running())
            .map(|handle| handle.id().to_owned())
            .collect();
        for id in ids {
            self.stop(&id).await?;
        }
        self.inner.clean_shutdown.store(true, Ordering::Release);
        let _marker = self
            .inner
            .marker_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match fs::remove_file(&self.inner.marker_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

impl SupervisorInner {
    fn active_records(&self) -> Vec<InterruptionRecord> {
        self.processes
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .filter(|handle| handle.is_running())
            .map(|handle| handle.state.interruption_record())
            .collect()
    }

    fn update_marker(&self) -> io::Result<()> {
        let _marker = self
            .marker_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let records = self.active_records();
        if records.is_empty() {
            return match fs::remove_file(&self.marker_path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            };
        }
        let mut file = File::create(&self.marker_path)?;
        serde_json::to_writer(&mut file, &records)?;
        file.write_all(b"\n")?;
        file.sync_all()
    }
}

impl Drop for SupervisorInner {
    fn drop(&mut self) {
        if self.clean_shutdown.load(Ordering::Acquire) {
            return;
        }
        let active = self.active_records();
        for record in active {
            if let Some(handle) = self
                .processes
                .read()
                .unwrap_or_else(|error| error.into_inner())
                .get(&record.id)
            {
                let _ = handle.state.tree.terminate();
            }
            if let Some(hook) = &self.hook {
                hook(record);
            }
        }
        // The already-written marker intentionally remains for the next Host.
    }
}

struct Spawned {
    state: Arc<ProcessState>,
    child: WaitTarget,
    readers: Vec<JoinHandle<()>>,
}

enum WaitTarget {
    Piped(Child),
    Pty(Box<dyn portable_pty::Child + Send + Sync>),
}

impl WaitTarget {
    fn wait(&mut self) -> io::Result<ProcessExit> {
        match self {
            Self::Piped(child) => std_exit(child.wait()?),
            Self::Pty(child) => {
                let status = child.wait()?;
                Ok(ProcessExit {
                    code: Some(status.exit_code() as i32),
                    signal: status.signal().map(str::to_owned),
                })
            }
        }
    }
}

fn spawn_process(
    id: String,
    spec: CommandSpec,
    spool_bytes: usize,
    event_capacity: usize,
) -> io::Result<Spawned> {
    match spec.stdio {
        StdioMode::Piped => spawn_piped(id, spec, spool_bytes, event_capacity),
        StdioMode::Pty(size) => spawn_pty(id, spec, size, spool_bytes, event_capacity),
    }
}

fn spawn_piped(
    id: String,
    spec: CommandSpec,
    spool_bytes: usize,
    event_capacity: usize,
) -> io::Result<Spawned> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    command.envs(spec.env.iter().map(|(key, value)| (key, value)));
    platform::prepare_command(&mut command);
    let mut child = command.spawn()?;
    let tree = match ProcessTree::attach_std(&child) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let pid = child.id();
    if let Err(error) = platform::resume_process(pid) {
        let _ = tree.terminate();
        let _ = child.wait();
        return Err(error);
    }
    let tree = Arc::new(tree);
    let stdin = child.stdin.take().expect("piped stdin was configured");
    let stdout = child.stdout.take().expect("piped stdout was configured");
    let stderr = child.stderr.take().expect("piped stderr was configured");
    let state = ProcessState::new(
        id,
        pid,
        tree,
        ProcessInput::Piped(Mutex::new(Some(stdin))),
        spool_bytes,
        event_capacity,
    );
    let readers = vec![
        spawn_reader(stdout, OutputStream::Stdout, Arc::clone(&state)),
        spawn_reader(stderr, OutputStream::Stderr, Arc::clone(&state)),
    ];
    Ok(Spawned {
        state,
        child: WaitTarget::Piped(child),
        readers,
    })
}

/// On Windows, portable-pty 0.9 cannot expose creation flags or the primary
/// thread, so Job assignment is best-effort after spawn. Piped spawning is
/// race-free; the platform seam permits a future custom ConPTY backend without
/// changing the public supervisor API.
fn spawn_pty(
    id: String,
    spec: CommandSpec,
    size: TerminalSize,
    spool_bytes: usize,
    event_capacity: usize,
) -> io::Result<Spawned> {
    let pair = portable_pty::native_pty_system()
        .openpty(portable_size(size))
        .map_err(|error| io::Error::other(error.to_string()))?;
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| io::Error::other(error.to_string()))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| io::Error::other(error.to_string()))?;
    let mut command = CommandBuilder::new(&spec.program);
    command.args(&spec.args);
    if let Some(cwd) = &spec.cwd {
        command.cwd(cwd.as_os_str());
    }
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| io::Error::other(error.to_string()))?;
    drop(pair.slave);
    let pid = child
        .process_id()
        .ok_or_else(|| io::Error::other("PTY backend did not expose a process ID"))?;
    let tree = match attach_pty_tree(child.as_ref(), pid) {
        Ok(tree) => Arc::new(tree),
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let state = ProcessState::new(
        id,
        pid,
        tree,
        ProcessInput::Pty(Mutex::new(Some(PtyInput {
            master: pair.master,
            writer: Some(writer),
        }))),
        spool_bytes,
        event_capacity,
    );
    let readers = vec![spawn_reader(reader, OutputStream::Pty, Arc::clone(&state))];
    Ok(Spawned {
        state,
        child: WaitTarget::Pty(child),
        readers,
    })
}

#[cfg(unix)]
fn attach_pty_tree(_child: &dyn portable_pty::Child, pid: u32) -> io::Result<ProcessTree> {
    ProcessTree::attach_pty(pid)
}

#[cfg(windows)]
fn attach_pty_tree(child: &dyn portable_pty::Child, _pid: u32) -> io::Result<ProcessTree> {
    let handle = child
        .as_raw_handle()
        .ok_or_else(|| io::Error::other("ConPTY child did not expose a process handle"))?;
    ProcessTree::attach_pty(handle)
}

fn portable_size(size: TerminalSize) -> portable_pty::PtySize {
    portable_pty::PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn spawn_reader(
    reader: impl Read + Send + 'static,
    stream: OutputStream,
    state: Arc<ProcessState>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        loop {
            let mut bytes = Vec::new();
            match reader.read_until(b'\n', &mut bytes) {
                Ok(0) => break,
                Ok(_) => state.output(stream, bytes),
                Err(_) => break,
            }
        }
    })
}

fn spawn_waiter(mut spawned: Spawned, supervisor: Weak<SupervisorInner>) {
    thread::spawn(move || {
        let exit = spawned.child.wait().unwrap_or_else(|error| ProcessExit {
            code: None,
            signal: Some(format!("wait failed: {error}")),
        });
        // The root process defines tree lifetime. Reap any descendant that
        // outlived it before publishing the final frame.
        let _ = spawned.state.tree.terminate();
        spawned.state.close_io_after_exit();
        for reader in spawned.readers {
            let _ = reader.join();
        }
        spawned.state.finish(exit);
        if let Some(supervisor) = supervisor.upgrade() {
            let _ = supervisor.update_marker();
        }
    });
}

fn std_exit(status: std::process::ExitStatus) -> io::Result<ProcessExit> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        Ok(ProcessExit {
            code: status.code(),
            signal: status.signal().map(|signal| format!("signal {signal}")),
        })
    }
    #[cfg(windows)]
    {
        Ok(ProcessExit {
            code: status.code(),
            signal: None,
        })
    }
}

fn _assert_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Supervisor>();
    assert_send_sync::<ProcessHandle>();
}
