use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, ExitStatus, Output, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

pub const BINARY: &str = env!("CARGO_BIN_EXE_mcp-doze");
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn init() -> Value {
    json!({"protocolVersion":"2025-11-25","capabilities":{"roots":{"listChanged":true},"elicitation":{}},"clientInfo":{"name":"mcp-doze-e2e","version":"1"},"_meta":{"fixture/client":"keep"}})
}
pub fn hook() -> Value {
    json!({"hook_event_name":"Stop","session_id":"fixture-session","turn_id":"fixture-turn"})
}
pub fn text_value(value: &Value) -> Value {
    serde_json::from_str(value["content"][0]["text"].as_str().unwrap()).unwrap()
}
pub fn until(mut predicate: impl FnMut() -> bool, seconds: u64) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !predicate() {
        assert!(Instant::now() < deadline, "condition timed out");
        thread::sleep(Duration::from_millis(25));
    }
}
pub fn group_members(pgid: u32) -> Vec<u32> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,pgid="])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cannot inspect test-owned process groups"
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let group: u32 = fields.next()?.parse().ok()?;
            (group == pgid).then_some(pid)
        })
        .collect()
}
pub fn kill_group(pgid: u32) {
    assert_ne!(pgid, unsafe { libc::getpgrp() } as u32);
    unsafe {
        libc::kill(-(pgid as i32), libc::SIGKILL);
    }
}
pub fn signal(pid: u32, signal: i32) {
    assert_eq!(unsafe { libc::kill(pid as i32, signal) }, 0);
}
pub fn wait_child(child: &mut Child, seconds: u64) -> ExitStatus {
    let mut status = None;
    until(
        || {
            status = child.try_wait().unwrap();
            status.is_some()
        },
        seconds,
    );
    status.unwrap()
}

pub struct Directory(pub PathBuf);
impl Directory {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mcp-doze-test-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub struct Context {
    pub directory: Directory,
    pub cache: PathBuf,
    pub fixture: PathBuf,
}
impl Context {
    pub fn new() -> Self {
        let directory = Directory::new();
        let cache = directory.0.join("cache");
        let fixture = Path::new(BINARY)
            .parent()
            .unwrap()
            .join("examples/mcp-fixture");
        assert!(
            fixture.is_file(),
            "Cargo must build the fixture example before integration tests: {}",
            fixture.display()
        );
        let context = Self {
            directory,
            cache,
            fixture,
        };
        context.control(json!({}));
        context
    }
    pub fn path(&self, name: &str) -> PathBuf {
        self.directory.0.join(name)
    }
    pub fn control(&self, value: Value) {
        fs::write(self.path("control.json"), value.to_string()).unwrap();
    }
    pub fn clear_events(&self) {
        fs::write(self.path("events.jsonl"), "").unwrap();
    }
    pub fn events(&self) -> Vec<Value> {
        fs::read_to_string(self.path("events.jsonl"))
            .unwrap_or_default()
            .split_inclusive('\n')
            .filter(|line| line.ends_with('\n'))
            .map(|line| serde_json::from_str(line).expect("fixture event must be valid JSON"))
            .collect()
    }
    pub fn events_named(&self, name: &str) -> Vec<Value> {
        self.events()
            .into_iter()
            .filter(|v| v["event"] == name)
            .collect()
    }
    pub fn starts(&self) -> Vec<Value> {
        self.events_named("start")
    }
    pub fn operations(&self) -> Vec<Value> {
        self.events_named("operation")
    }
    pub fn cache_files(&self) -> Vec<PathBuf> {
        fs::read_dir(&self.cache)
            .map(|entries| {
                entries
                    .map(|e| e.unwrap().path())
                    .filter(|p| p.extension().is_some_and(|s| s == "json"))
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn command(
        &self,
        mode: &[&str],
        flags: &[&str],
        fixture: &Path,
        backend_args: &[&str],
    ) -> Command {
        let mut command = Command::new(BINARY);
        command
            .args(mode)
            .arg("--cache-dir")
            .arg(&self.cache)
            .args(flags)
            .arg("--")
            .arg(fixture)
            .args(backend_args)
            .env("MCP_DOZE_TEST_DIR", &self.directory.0)
            .current_dir(env!("CARGO_MANIFEST_DIR"));
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        command
    }
    pub fn refresh(&self, flags: &[&str]) {
        self.refresh_fixture(flags, &self.fixture, true);
    }
    pub fn refresh_fixture(&self, flags: &[&str], fixture: &Path, success: bool) -> Output {
        let result = self
            .command(&["cache", "refresh"], flags, fixture, &[])
            .output()
            .unwrap();
        assert_eq!(
            result.status.success(),
            success,
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        self.clear_events();
        result
    }
    pub fn client(&self, flags: &[&str]) -> Client {
        self.client_fixture(flags, &self.fixture, &[])
    }
    pub fn auto(&self, flags: &[&str]) -> Client {
        let mut auto = vec!["--auto-refresh", "--timeout-ms", "15000"];
        auto.extend(flags);
        self.client(&auto)
    }
    pub fn client_fixture(&self, flags: &[&str], fixture: &Path, backend_args: &[&str]) -> Client {
        Client::new(
            self.command(&["run"], flags, fixture, backend_args),
            self.path(&format!(
                "client-{}.stderr",
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            )),
        )
    }
    pub fn collector_stopped(&self) {
        let starts = self.starts();
        assert!(!starts.is_empty());
        for start in starts {
            assert!(
                group_members(start["pgid"].as_u64().unwrap() as u32).is_empty(),
                "collector survived initialization response"
            );
        }
    }
    pub fn copied_fixture(&self) -> PathBuf {
        let copied = self.path("fixture-copy");
        fs::copy(&self.fixture, &copied).unwrap();
        copied
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        for start in self.starts() {
            let pgid = start["pgid"].as_u64().unwrap() as u32;
            if !group_members(pgid).is_empty() {
                kill_group(pgid);
            }
        }
    }
}

pub fn change_entrypoint_metadata(path: &Path) {
    let permissions = fs::metadata(path).unwrap().permissions().mode();
    // Changing executable metadata preserves the Mach-O signature of the test binary.
    fs::set_permissions(path, fs::Permissions::from_mode(permissions ^ 0o010)).unwrap();
}

pub struct Client {
    pub child: Child,
    input: Arc<Mutex<Option<ChildStdin>>>,
    receiver: Receiver<Result<Value, String>>,
    responses: HashMap<String, Value>,
    pub received: Vec<Value>,
    pub parse_errors: Arc<Mutex<Vec<String>>>,
    stderr_path: PathBuf,
    reader: Option<thread::JoinHandle<()>>,
    sequence: u64,
    cleaned: bool,
}
impl Client {
    fn new(mut command: Command, stderr_path: PathBuf) -> Self {
        let stderr = File::create(&stderr_path).unwrap();
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .unwrap();
        let input = Arc::new(Mutex::new(child.stdin.take()));
        let stdout = child.stdout.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        let writer = Arc::clone(&input);
        let parse_errors = Arc::new(Mutex::new(Vec::new()));
        let errors = Arc::clone(&parse_errors);
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(e) => {
                        let _ = sender.send(Err(e.to_string()));
                        return;
                    }
                };
                let message: Value = match serde_json::from_str(&line) {
                    Ok(message) => message,
                    Err(_) => {
                        errors.lock().unwrap().push(line);
                        let _ = sender.send(Err("invalid JSON-RPC received".into()));
                        continue;
                    }
                };
                if message.get("method").is_some() && message.get("id").is_some() {
                    let answer = match message["method"].as_str().unwrap_or("") {
                        "roots/list" => {
                            json!({"roots":[{"uri":"file:///fixture","name":"fixture"}]})
                        }
                        "elicitation/create" => {
                            json!({"action":"accept","content":{"name":"fixture answer"}})
                        }
                        "ping" => json!({}),
                        _ => json!({"fixture":"answer"}),
                    };
                    let response = json!({"jsonrpc":"2.0","id":message["id"],"result":answer});
                    if let Some(input) = writer.lock().unwrap().as_mut() {
                        if writeln!(input, "{response}")
                            .and_then(|_| input.flush())
                            .is_err()
                        {
                            break;
                        }
                    }
                }
                if sender.send(Ok(message)).is_err() {
                    return;
                }
            }
            let _ = sender.send(Err("transport closed".into()));
        });
        Self {
            child,
            input,
            receiver,
            responses: HashMap::new(),
            received: vec![],
            parse_errors,
            stderr_path,
            reader: Some(reader),
            sequence: 0,
            cleaned: false,
        }
    }
    pub fn send(&self, value: Value) {
        let mut guard = self.input.lock().unwrap();
        let input = guard.as_mut().expect("input already closed");
        writeln!(input, "{value}")
            .and_then(|_| input.flush())
            .unwrap();
    }
    pub fn begin(&mut self, method: &str, params: Value) -> u64 {
        self.sequence += 1;
        let id = self.sequence;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        id
    }
    pub fn wait(&mut self, id: u64, seconds: u64) -> Result<Value, String> {
        let key = id.to_string();
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            if let Some(response) = self.responses.remove(&key) {
                return if let Some(error) = response.get("error") {
                    Err(error.to_string())
                } else {
                    Ok(response["result"].clone())
                };
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!("MCP request timed out: {}", self.stderr()));
            }
            let message = self
                .receiver
                .recv_timeout(remaining)
                .map_err(|e| format!("{e}: {}", self.stderr()))??;
            if message.get("method").is_none() {
                if let Some(id) = message.get("id") {
                    self.responses.insert(id.to_string(), message.clone());
                }
            }
            self.received.push(message);
        }
    }
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.begin(method, params);
        self.wait(id, 6)
    }
    pub fn initialize(&mut self) -> Value {
        let id = self.begin("initialize", init());
        let result = self.wait(id, 20).unwrap();
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        result
    }
    pub fn tool(&mut self, name: &str, args: Value, meta: Option<Value>) -> Result<Value, String> {
        let mut params = json!({"name":name,"arguments":args});
        if let Some(meta) = meta {
            params["_meta"] = meta
        }
        let result = self.call("tools/call", params)?;
        if result["isError"] == true {
            Err(result.to_string())
        } else {
            Ok(result)
        }
    }
    pub fn stderr(&self) -> String {
        fs::read_to_string(&self.stderr_path).unwrap_or_default()
    }
    pub fn eof(&self) {
        self.input.lock().unwrap().take();
    }
    pub fn close(&mut self) {
        if self.cleaned {
            return;
        }
        self.cleaned = true;
        self.eof();
        let deadline = Instant::now() + Duration::from_secs(4);
        while self.child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(25));
        }
        if self.child.try_wait().unwrap().is_none() {
            kill_group(self.child.id());
            let _ = self.child.wait();
        }
        if !group_members(self.child.id()).is_empty() {
            kill_group(self.child.id());
            until(|| group_members(self.child.id()).is_empty(), 6);
        }
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}

pub fn json_file(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
pub fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}
pub fn private_file(path: &Path) -> File {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .unwrap()
}
