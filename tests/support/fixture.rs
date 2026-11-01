//! Local MCP fixture. State and diagnostics stay in the test-owned directory.
use serde_json::{Value, json};
use std::{
    env,
    fs::{self, OpenOptions},
    io::{self, BufRead, Write},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

struct Fixture {
    directory: PathBuf,
    value: i64,
    next_server_id: u64,
}

impl Fixture {
    fn record(&self, event: &str, fields: Value) {
        let mut item =
            json!({"event":event,"pid":std::process::id(),"pgid":unsafe { libc::getpgrp() }});
        item.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.directory.join("events.jsonl"))
            .unwrap();
        let mut record = serde_json::to_vec(&item).unwrap();
        record.push(b'\n');
        file.write_all(&record).unwrap();
    }

    fn control(&self) -> Value {
        fs::read(self.directory.join("control.json"))
            .ok()
            .map(|s| serde_json::from_slice(&s).unwrap())
            .unwrap_or_else(|| json!({}))
    }

    fn tools(&self) -> Value {
        let version = self.control()["catalog_version"].as_u64().unwrap_or(1);
        let mut tools: Vec<Value> = ["increment", "state", "duplex", "media", "spawn_child", "crash", "malformed"]
            .into_iter().map(|name| json!({"name":name,"description":format!("fixture {name} version {version}"),
                "inputSchema":{"type":"object","additionalProperties":true},"_meta":{"fixture/tool":name}})).collect();
        let mut schema = json!({"type":"object","additionalProperties":false,
            "properties":{"hook_event_name":{"type":"string","minLength":1},"session_id":{"type":"string","minLength":1},"turn_id":{"type":"string","minLength":1}},
            "required":["hook_event_name","session_id","turn_id"]});
        if self.control()["invalid_cleanup_schema"] == true {
            schema["required"] = json!(["hook_event_name"]);
        }
        tools.push(json!({"name":"turn_ended","description":"fixture cleanup","inputSchema":schema,"annotations":{"idempotentHint":true}}));
        json!(tools)
    }

    #[allow(
        clippy::zombie_processes,
        reason = "The lifecycle test requires a descendant that survives until wrapper group cleanup."
    )]
    fn child(&self) -> u32 {
        let child = Command::new(env::current_exe().unwrap())
            .arg("--term-child")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        thread::sleep(Duration::from_millis(100));
        self.record("child", json!({"child_pid":child.id()}));
        child.id()
    }

    fn client_request(&mut self, reader: &mut impl BufRead, method: &str, params: Value) -> Value {
        self.next_server_id += 1;
        let id = json!(format!("fixture-server-{}", self.next_server_id));
        send(
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params,"_meta":{"fixture/server-request":"preserve"}}),
        );
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap() > 0 {
            let response: Value = serde_json::from_str(&line).unwrap();
            line.clear();
            if response.get("method").is_none() && response.get("id") == Some(&id) {
                self.record(
                    "client_response",
                    json!({"method":method,"result":response["result"]}),
                );
                return response["result"].clone();
            }
            if response["method"] == "notifications/cancelled" {
                self.record(
                    "cancelled",
                    json!({"request_id":response["params"]["requestId"]}),
                );
            }
        }
        panic!("client disconnected during reverse request");
    }

    fn handle(&mut self, reader: &mut impl BufRead, request: Value) {
        let id = &request["id"];
        let params = &request["params"];
        match request["method"].as_str().unwrap_or("") {
            "initialize" => {
                self.record("initialize", json!({"protocol":params["protocolVersion"],"capabilities":params["capabilities"],"client_info":params["clientInfo"],"meta":params["_meta"]}));
                let control = self.control();
                thread::sleep(Duration::from_secs_f64(
                    control["initialize_delay"].as_f64().unwrap_or(0.0),
                ));
                if let Some(error) = control.get("initialize_error") {
                    send(json!({"jsonrpc":"2.0","id":id,"error":error}));
                } else {
                    result(
                        id,
                        json!({"protocolVersion":params["protocolVersion"],
                        "capabilities":{"tools":{"listChanged":true},"resources":{"subscribe":true,"listChanged":true},"prompts":{"listChanged":true}},
                        "serverInfo":{"name":"mcp-doze-fixture","version":"0.1.0"},"instructions":"Keep the fixture instructions intact.","_meta":{"fixture/initialize":true}}),
                    );
                }
            }
            "notifications/initialized" => self.record("initialized", json!({})),
            "tools/list" => {
                let tools = self.tools();
                let tools = tools.as_array().unwrap();
                if params.get("cursor").is_some() {
                    assert_eq!(params["cursor"], format!("page-two-{}", std::process::id()));
                    result(
                        id,
                        json!({"tools":tools[4..],"_meta":{"fixture/catalog":true}}),
                    );
                } else {
                    result(
                        id,
                        json!({"tools":tools[..4],"nextCursor":format!("page-two-{}", std::process::id()),"_meta":{"fixture/catalog":true}}),
                    );
                }
            }
            "resources/list" => result(
                id,
                json!({"resources":[{"uri":"fixture://one","name":"one"}]}),
            ),
            "resources/templates/list" => result(
                id,
                json!({"resourceTemplates":[{"uriTemplate":"fixture://{id}","name":"template"}]}),
            ),
            "prompts/list" => result(
                id,
                json!({"prompts":[{"name":"fixture-prompt","description":"fixture prompt"}]}),
            ),
            "resources/read" => {
                self.record("operation", json!({"name":"resources/read"}));
                result(
                    id,
                    json!({"contents":[{"uri":"fixture://one","text":"resource contents"}]}),
                );
            }
            "prompts/get" => {
                self.record("operation", json!({"name":"prompts/get"}));
                result(
                    id,
                    json!({"messages":[{"role":"user","content":{"type":"text","text":"prompt contents"}}]}),
                );
            }
            "ping" => result(id, json!({})),
            "notifications/cancelled" => {
                self.record("cancelled", json!({"request_id":params["requestId"]}))
            }
            "tools/call" => {
                let name = params["name"].as_str().unwrap();
                let args = &params["arguments"];
                self.record(
                    "operation",
                    json!({"name":name,"arguments":args,"meta":params["_meta"]}),
                );
                match name {
                    "increment" => {
                        self.value += args["delta"].as_i64().unwrap_or(1);
                        tool_result(id, json!({"value":self.value,"pid":std::process::id()}));
                    }
                    "state" => {
                        tool_result(id, json!({"value":self.value,"pid":std::process::id()}))
                    }
                    "turn_ended" => result(
                        id,
                        json!({"content":[{"type":"text","text":"{}"}],"isError":false}),
                    ),
                    "duplex" => {
                        let roots = self.client_request(reader, "roots/list", json!({}));
                        let answer = self.client_request(reader, "elicitation/create", json!({"message":"fixture question","requestedSchema":{"type":"object","properties":{"name":{"type":"string"}}}}));
                        send(
                            json!({"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":params["_meta"]["progressToken"].as_str().unwrap_or("fixture-progress"),"progress":1,"total":1,"_meta":{"fixture/progress":"preserve"}}}),
                        );
                        tool_result(id, json!({"roots":roots,"answer":answer}));
                    }
                    "media" => result(
                        id,
                        json!({"content":[
                        {"type":"image","mimeType":"image/png","data":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aJVQAAAAASUVORK5CYII="},
                        {"type":"audio","mimeType":"audio/wav","data":"UklGRgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=="}],
                        "structuredContent":{"value":"x".repeat(1048576)},"_meta":{"fixture/unknown":[1,{"preserve":true}]}}),
                    ),
                    "spawn_child" => tool_result(id, json!({"child_pid":self.child()})),
                    "crash" => std::process::exit(37),
                    "malformed" => {
                        println!("this is not JSON-RPC");
                        io::stdout().flush().unwrap();
                    }
                    _ => send(
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"unknown fixture tool"}}),
                    ),
                }
            }
            _ if !id.is_null() => result(
                id,
                json!({"echo":request,"_meta":{"fixture/unknown-method":true}}),
            ),
            _ => {}
        }
    }
}

fn send(value: Value) {
    let mut out = io::stdout().lock();
    writeln!(out, "{value}").unwrap();
    out.flush().unwrap();
}
fn result(id: &Value, value: Value) {
    send(json!({"jsonrpc":"2.0","id":id,"result":value}));
}
fn tool_result(id: &Value, value: Value) {
    result(
        id,
        json!({"content":[{"type":"text","text":value.to_string()}],"isError":false,"_meta":{"fixture/extension":{"kept":true}}}),
    );
}
fn main() {
    if env::args().any(|s| s == "--term-child") {
        unsafe {
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
        }
        thread::sleep(Duration::from_secs(600));
        return;
    }
    let mut fixture = Fixture {
        directory: PathBuf::from(env::var_os("MCP_DOZE_TEST_DIR").unwrap()),
        value: 0,
        next_server_id: 0,
    };
    fixture.record("start", json!({"ppid":unsafe {libc::getppid()}}));
    let input = io::stdin();
    let mut reader = input.lock();
    if env::args().any(|s| s == "--hanging") {
        fixture.child();
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap() > 0 {
            let request: Value = serde_json::from_str(&line).unwrap();
            line.clear();
            if request["method"] == "initialize" {
                fixture.record("initialize", json!({}));
                thread::sleep(Duration::from_secs(600));
            }
        }
        return;
    }
    let mut line = String::new();
    while reader.read_line(&mut line).unwrap() > 0 {
        let request = serde_json::from_str(&line).unwrap();
        line.clear();
        fixture.handle(&mut reader, request);
    }
    fixture.record("stdin_eof", json!({}));
}
