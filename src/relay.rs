use crate::{
    cache::{self, Cache, CacheLock},
    config::Launch,
    process::{self, Group},
    wire::{self, Frame},
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    future::pending,
    io,
    process::ExitStatus,
};
use tokio::{
    io::BufReader,
    process::{Child, ChildStdin},
    signal::unix::{Signal, SignalKind, signal},
    sync::mpsc,
    time::{Instant, sleep_until},
};

struct Signals {
    term: Signal,
    interrupt: Signal,
    hangup: Signal,
}
impl Signals {
    fn new() -> Result<Self> {
        Ok(Self {
            term: signal(SignalKind::terminate())?,
            interrupt: signal(SignalKind::interrupt())?,
            hangup: signal(SignalKind::hangup())?,
        })
    }
    async fn next(&mut self) {
        tokio::select! { _ = self.term.recv() => {}, _ = self.interrupt.recv() => {}, _ = self.hangup.recv() => {} }
    }
}

fn result(id: &Value, value: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":value})
}
fn initialize() -> Value {
    json!({"jsonrpc":"2.0","id":"mcp-doze-cache-init","method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"mcp-doze","version":env!("CARGO_PKG_VERSION")}}})
}
fn initialized() -> Value {
    json!({"jsonrpc":"2.0","method":"notifications/initialized"})
}
fn methods(value: &Value) -> Vec<String> {
    let mut methods = Vec::new();
    for (capability, names) in [
        ("tools", &["tools/list"][..]),
        (
            "resources",
            &["resources/list", "resources/templates/list"][..],
        ),
        ("prompts", &["prompts/list"][..]),
    ] {
        if value["capabilities"].get(capability).is_some() {
            methods.extend(names.iter().map(|n| n.to_string()));
        }
    }
    methods
}

struct List {
    value: Value,
    cursors: HashSet<String>,
    bytes: usize,
}
impl List {
    fn new() -> Self {
        Self {
            value: Value::Null,
            cursors: HashSet::new(),
            bytes: 0,
        }
    }
    fn append(&mut self, method: &str, page: Value) -> Result<Option<String>> {
        let key = match method {
            "tools/list" => "tools",
            "resources/list" => "resources",
            "resources/templates/list" => "resourceTemplates",
            "prompts/list" => "prompts",
            _ => bail!("invalid cached discovery method"),
        };
        let items = page[key].as_array().context("invalid MCP discovery list")?;
        self.bytes += serde_json::to_vec(&page)?.len();
        if self.bytes > wire::LIMIT {
            bail!("discovery list exceeds 64 MiB");
        }
        if self.value.is_null() {
            self.value = page.clone();
            self.value
                .as_object_mut()
                .context("invalid discovery result")?
                .remove("nextCursor");
        } else {
            self.value[key]
                .as_array_mut()
                .context("invalid discovery accumulator")?
                .extend(items.iter().cloned());
        }
        match page.get("nextCursor") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(cursor)) if !cursor.is_empty() => {
                if !self.cursors.insert(cursor.clone()) {
                    bail!("MCP discovery repeated a cursor");
                }
                Ok(Some(cursor.clone()))
            }
            _ => bail!("invalid discovery cursor"),
        }
    }
}

async fn receive<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    input: &mut ChildStdin,
    id: &Value,
    launch: &Launch,
    signals: &mut Signals,
) -> Result<Value> {
    let deadline = Instant::now() + launch.timeout;
    loop {
        let frame = tokio::select! {
            _ = signals.next() => bail!("interrupted while collecting cache"),
            _ = sleep_until(deadline) => bail!("MCP discovery timed out"),
            frame = wire::read(reader) => frame?.context("MCP closed during discovery")?,
        };
        if frame.value.get("method").is_none() && frame.value.get("id") == Some(id) {
            if frame.value.get("error").is_some() {
                bail!("MCP rejected discovery request");
            }
            return frame
                .value
                .get("result")
                .cloned()
                .context("discovery response has no result");
        }
        if frame.value.get("method").is_some() && frame.value.get("id").is_some() {
            if frame.value["method"] != "ping" {
                bail!(
                    "cache collection requires a client capability; keep this MCP directly connected"
                );
            }
            wire::send(
                input,
                &result(&frame.value["id"], json!({})),
                launch.timeout,
            )
            .await?;
        }
    }
}

pub async fn refresh(launch: &Launch) -> Result<()> {
    let _lock = CacheLock::acquire(launch)?;
    let mut signals = Signals::new()?;
    let mut group = Group::collector();
    let mut child = process::spawn(launch, &mut group).await?;
    let mut input = child.stdin.take().context("backend stdin unavailable")?;
    let mut reader = BufReader::new(child.stdout.take().context("backend stdout unavailable")?);
    let collected: Result<Cache> = async {
        let init = initialize();
        wire::send(&mut input, &init, launch.timeout).await?;
        let initialize =
            receive(&mut reader, &mut input, &init["id"], launch, &mut signals).await?;
        if initialize["protocolVersion"].as_str().is_none()
            || !initialize["capabilities"].is_object()
            || !initialize["serverInfo"].is_object()
        {
            bail!("invalid MCP initialization result");
        }
        wire::send(&mut input, &initialized(), launch.timeout).await?;
        let mut lists = BTreeMap::new();
        for method in methods(&initialize) {
            let mut list = List::new();
            let mut params = json!({});
            loop {
                let id = json!(format!("mcp-doze-cache-{method}"));
                wire::send(
                    &mut input,
                    &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
                    launch.timeout,
                )
                .await?;
                let page = receive(&mut reader, &mut input, &id, launch, &mut signals).await?;
                if let Some(cursor) = list.append(&method, page)? {
                    params = json!({"cursor":cursor});
                } else {
                    break;
                }
            }
            lists.insert(method, list.value);
        }
        Ok(Cache {
            format: cache::FORMAT,
            identity: cache::identity(launch)?,
            initialize,
            lists,
        })
    }
    .await;
    drop(input);
    // No cache is published until its collector and descendants have stopped.
    group.terminate(Some(&mut child)).await?;
    let cached = collected?;
    if launch.repl_cleanup && !repl_schema(&cached) {
        bail!("--repl-cleanup requires the verified turn_ended schema");
    }
    cache::save(launch, &cached)?;
    eprintln!("mcp-doze: discovery cache refreshed");
    Ok(())
}

fn repl_schema(cache: &Cache) -> bool {
    let Some(tool) = cache
        .lists
        .get("tools/list")
        .and_then(|x| x["tools"].as_array())
        .and_then(|ts| ts.iter().find(|t| t["name"] == "turn_ended"))
    else {
        return false;
    };
    let schema = &tool["inputSchema"];
    let keys = ["hook_event_name", "session_id", "turn_id"];
    schema.as_object().is_some_and(|s| {
        s.keys().all(|k| {
            [
                "type",
                "additionalProperties",
                "required",
                "properties",
                "description",
                "title",
            ]
            .contains(&k.as_str())
        })
    }) && schema["type"] == "object"
        && schema["additionalProperties"] == false
        && schema["required"]
            .as_array()
            .is_some_and(|r| r.len() == 3 && keys.iter().all(|key| r.contains(&json!(key))))
        && schema["properties"].as_object().is_some_and(|p| {
            p.len() == 3
                && keys.iter().all(|key| {
                    p.get(*key).is_some_and(|s| {
                        s.as_object().is_some_and(|s| {
                            s.keys().all(|k| {
                                ["type", "minLength", "description", "title"].contains(&k.as_str())
                            })
                        }) && s["type"] == "string"
                            && s["minLength"] == 1
                    })
                })
        })
}

fn cold_cleanup(cache: &Cache, request: &Value) -> bool {
    request["method"] == "tools/call"
        && request["params"]["name"] == "turn_ended"
        && repl_schema(cache)
        && request["params"]["arguments"].as_object().is_some_and(|a| {
            a.len() == 3
                && ["hook_event_name", "session_id", "turn_id"]
                    .iter()
                    .all(|k| {
                        a.get(*k)
                            .and_then(Value::as_str)
                            .is_some_and(|s| !s.is_empty())
                    })
        })
}

struct Startup {
    id: Value,
    method: Option<String>,
    rest: VecDeque<String>,
    list: List,
    deadline: Instant,
    sequence: u64,
}
impl Startup {
    fn next_id(&mut self) -> Value {
        self.sequence += 1;
        json!(format!("mcp-doze-{}-{}", std::process::id(), self.sequence))
    }
}

async fn next_list(
    startup: &mut Startup,
    input: &mut ChildStdin,
    launch: &Launch,
    cursor: Option<String>,
) -> Result<bool> {
    if cursor.is_none() {
        startup.method = startup.rest.pop_front();
        startup.list = List::new();
    }
    let Some(method) = startup.method.clone() else {
        return Ok(false);
    };
    startup.id = startup.next_id();
    let params = cursor.map_or(json!({}), |cursor| json!({"cursor":cursor}));
    wire::send(
        input,
        &json!({"jsonrpc":"2.0","id":startup.id,"method":method,"params":params}),
        launch.timeout,
    )
    .await?;
    Ok(true)
}

async fn exit(child: &mut Option<Child>) -> io::Result<ExitStatus> {
    match child {
        Some(child) => child.wait().await,
        None => pending().await,
    }
}
async fn expired(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => sleep_until(deadline).await,
        None => pending().await,
    }
}

pub async fn run(launch: &Launch) -> Result<()> {
    let cached = cache::load(launch).context(
        "cache unavailable; run cache refresh with the same options and backend command",
    )?;
    if launch.repl_cleanup && !repl_schema(&cached) {
        bail!("cache does not contain the verified REPL cleanup contract");
    }
    let mut signals = Signals::new()?;
    let mut group = Group::wrapper()?;
    let mut child = None;
    let mut input = None;
    let outcome = connection(
        launch,
        &cached,
        &mut signals,
        &mut group,
        &mut child,
        &mut input,
    )
    .await;
    if let Err(error) = &outcome {
        eprintln!("mcp-doze: {error:#}");
    }
    drop(input);
    group.terminate(child.as_mut()).await?;
    outcome
}

async fn connection(
    launch: &Launch,
    cached: &Cache,
    signals: &mut Signals,
    group: &mut Group,
    child: &mut Option<Child>,
    input: &mut Option<ChildStdin>,
) -> Result<()> {
    let (client_tx, mut client_rx) = mpsc::channel(1);
    tokio::spawn(wire::pump(BufReader::new(tokio::io::stdin()), client_tx));
    let (backend_tx, mut backend_rx) = mpsc::channel(1);
    let mut output = tokio::io::stdout();
    let mut init = None::<Value>;
    let mut ready = None::<Value>;
    let mut startup = None::<Startup>;
    let mut active = false;
    let mut queue = VecDeque::<Frame>::new();
    let mut queued_bytes = 0usize;
    loop {
        tokio::select! {
            _ = signals.next() => return Ok(()),
            status = exit(child) => { let _ = status?; bail!("backend exited; state was lost, reconnect explicitly (operation was not retried)"); },
            _ = expired(startup.as_ref().map(|s| s.deadline)) => bail!("backend initialization timed out; operation was not sent"),
            message = client_rx.recv() => {
                let Some(frame) = message.context("client reader stopped")?? else { return Ok(()); };
                let method = frame.value["method"].as_str();
                if active {
                    wire::write(input.as_mut().context("backend stdin missing")?, &frame.raw, launch.timeout).await?;
                    continue;
                }
                if method == Some("initialize") {
                    if init.is_some() || frame.value.get("id").is_none() || !frame.value["params"].is_object() { bail!("invalid or repeated initialize request"); }
                    wire::send(&mut output, &result(&frame.value["id"], cached.initialize.clone()), launch.timeout).await?;
                    init = Some(frame.value);
                    continue;
                }
                if method == Some("ping") {
                    if let Some(id) = frame.value.get("id") { wire::send(&mut output, &result(id, json!({})), launch.timeout).await?; }
                    continue;
                }
                if init.is_none() { bail!("initialize must be the first request"); }
                if method == Some("notifications/initialized") {
                    if ready.is_some() { bail!("repeated initialized notification"); }
                    ready = Some(frame.value);
                    continue;
                }
                if ready.is_none() { bail!("client has not completed initialization"); }
                if let Some(catalog) = method.and_then(|method| cached.lists.get(method)) {
                    if frame.value["params"].get("cursor").is_some_and(|x| !x.is_null()) { bail!("cached discovery has a single page; do not reuse backend cursors"); }
                    if let Some(id) = frame.value.get("id") { wire::send(&mut output, &result(id, catalog.clone()), launch.timeout).await?; }
                    continue;
                }
                if method == Some("notifications/cancelled") {
                    let id = &frame.value["params"]["requestId"];
                    queue.retain(|pending| pending.value.get("id") != Some(id));
                    queued_bytes = queue.iter().map(|f| f.raw.len()).sum();
                    continue;
                }
                if launch.repl_cleanup && child.is_none() && cold_cleanup(cached, &frame.value) {
                    if let Some(id) = frame.value.get("id") { wire::send(&mut output, &result(id, json!({"content":[{"type":"text","text":"{}"}],"isError":false})), launch.timeout).await?; }
                    continue;
                }
                if method.is_none() {
                    if let Some(input) = input { wire::write(input, &frame.raw, launch.timeout).await?; continue; }
                    bail!("unexpected client response before backend startup");
                }
                if queued_bytes + frame.raw.len() > wire::LIMIT { bail!("pending MCP input exceeds 64 MiB"); }
                let starts_backend = frame.value.get("id").is_some();
                queued_bytes += frame.raw.len();
                queue.push_back(frame);
                if child.is_none() && starts_backend {
                    let mut backend = process::spawn(launch, group).await?;
                    let mut pipe = backend.stdin.take().context("backend stdin missing")?;
                    let mut request = init.clone().context("initialize missing")?;
                    let id = json!(format!("mcp-doze-{}-0", std::process::id()));
                    request["id"] = id.clone();
                    request["params"]["protocolVersion"] = cached.initialize["protocolVersion"].clone();
                    let stdout = backend.stdout.take().context("backend stdout missing")?;
                    *child = Some(backend);
                    tokio::spawn(wire::pump(BufReader::new(stdout), backend_tx.clone()));
                    *input = Some(pipe);
                    pipe = input.take().unwrap();
                    let sent = wire::send(&mut pipe, &request, launch.timeout).await;
                    *input = Some(pipe);
                    sent?;
                    startup = Some(Startup { id, method: None, rest: cached.lists.keys().cloned().collect(), list: List::new(), deadline: Instant::now() + launch.timeout, sequence: 0 });
                }
            },
            message = backend_rx.recv() => {
                let Some(frame) = message.context("backend reader stopped")?? else { bail!("backend transport closed; reconnect explicitly (operation was not retried)"); };
                let expected = startup.as_ref().is_some_and(|s| frame.value.get("method").is_none() && frame.value.get("id") == Some(&s.id));
                if expected {
                    let stage = startup.as_mut().unwrap();
                    let reply = frame.value.get("result").context("backend rejected initialization or discovery")?;
                    let pipe = input.as_mut().context("backend stdin missing")?;
                    let more = if let Some(method) = &stage.method {
                        if let Some(cursor) = stage.list.append(method, reply.clone())? {
                            next_list(stage, pipe, launch, Some(cursor)).await?
                        } else {
                            if cached.lists.get(method) != Some(&stage.list.value) {
                                cache::invalidate(launch)?;
                                bail!("backend catalog changed; refresh cache and reconnect (operation was not sent)");
                            }
                            next_list(stage, pipe, launch, None).await?
                        }
                    } else {
                        if reply != &cached.initialize {
                            cache::invalidate(launch)?;
                            bail!("backend initialization changed; refresh cache and reconnect (operation was not sent)");
                        }
                        wire::send(pipe, ready.as_ref().context("initialized missing")?, launch.timeout).await?;
                        next_list(stage, pipe, launch, None).await?
                    };
                    if !more {
                        startup = None;
                        active = true;
                        while let Some(frame) = queue.pop_front() { wire::write(pipe, &frame.raw, launch.timeout).await?; }
                        queued_bytes = 0;
                    }
                } else {
                    if frame.value["method"].as_str().is_some_and(|m| ["notifications/tools/list_changed", "notifications/resources/list_changed", "notifications/prompts/list_changed"].contains(&m)) { cache::invalidate(launch)?; }
                    wire::write(&mut output, &frame.raw, launch.timeout).await?;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_requires_the_exact_validation_contract() {
        let mut cached = Cache {
            format: cache::FORMAT,
            identity: String::new(),
            initialize: json!({}),
            lists: BTreeMap::from([(
                "tools/list".to_owned(),
                json!({"tools":[{
                    "name":"turn_ended",
                    "inputSchema":{
                        "type":"object","additionalProperties":false,
                        "required":["hook_event_name","session_id","turn_id"],
                        "properties":{
                            "hook_event_name":{"type":"string","minLength":1},
                            "session_id":{"type":"string","minLength":1},
                            "turn_id":{"type":"string","minLength":1}
                        }
                    }
                }]}),
            )]),
        };
        assert!(repl_schema(&cached));
        let original = cached.lists["tools/list"].clone();
        for (restriction, value) in [
            ("enum", json!(["Stop"])),
            ("pattern", json!("^Stop$")),
            ("maxLength", json!(4)),
        ] {
            cached.lists.insert("tools/list".into(), original.clone());
            cached.lists.get_mut("tools/list").unwrap()["tools"][0]["inputSchema"]["properties"]
                ["hook_event_name"][restriction] = value;
            assert!(!repl_schema(&cached));
        }
        cached.lists.insert("tools/list".into(), original);
        cached.lists.get_mut("tools/list").unwrap()["tools"][0]["inputSchema"]["allOf"] = json!([]);
        assert!(!repl_schema(&cached));
    }
}
