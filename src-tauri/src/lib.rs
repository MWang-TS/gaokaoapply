use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::Manager;

// shared types

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatSettings {
    #[serde(rename = "apiBaseUrl")]
    pub api_base_url: String,
    #[serde(rename = "apiKey")]
    pub api_key: String,
    pub model: String,
    pub temperature: f64,
    #[serde(rename = "maxTokens")]
    pub max_tokens: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: std::collections::HashMap<String, String>,
    pub enabled: bool,
}

// PDF Import types
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CutoffEntry {
    pub code: String,
    pub name: String,
    pub cutoff: serde_json::Value, // number or "580分及以上" string
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CutoffData {
    pub title: String,
    pub year: u32,
    pub province: String,
    pub note: Option<String>,
    pub data: Vec<CutoffEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(non_snake_case)]
pub struct FileDialogResult {
    pub filePath: String,
}

#[derive(Serialize, Clone)]
struct ChatChunk {
    #[serde(rename = "conversationId")]
    conversation_id: String,
    content: String,
}

fn app_data_dir(app: &tauri::AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn default_settings() -> serde_json::Value {
    serde_json::json!({
        "apiBaseUrl": "https://api.openai.com/v1",
        "apiKey": "",
        "model": "gpt-4o",
        "temperature": 0.7,
        "maxTokens": 2000,
        "activeSkillId": "zhangxuefeng",
        "mcpServers": [
            {
                "id": "mcp-gaokao-rank",
                "name": "gaokao rank",
                "description": "query gaokao score rank by province",
                "command": "uvx",
                "args": ["mcp-gaokao-rank"],
                "env": {},
                "enabled": true
            }
        ],
        "theme": "light"
    })
}

mod mcp {
    use std::collections::HashMap;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::process::{Child, ChildStdin, ChildStdout};

    pub struct McpProcess {
        pub child: Child,
        pub stdin: ChildStdin,
        pub stdout: BufReader<ChildStdout>,
        next_id: u64,
    }

    impl McpProcess {
        pub async fn spawn(
            command: &str,
            args: &[String],
            env: &HashMap<String, String>,
        ) -> Result<Self, String> {
            use tokio::process::Command;

            let mut cmd = Command::new(command);
            cmd.args(args)
                .envs(env)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null());

            let mut child = cmd.spawn().map_err(|e| format!("Failed to start MCP: {}", e))?;
            let stdin = child.stdin.take().ok_or("no stdin")?;
            let stdout = child.stdout.take().ok_or("no stdout")?;

            Ok(Self {
                child,
                stdin,
                stdout: BufReader::new(stdout),
                next_id: 1,
            })
        }

        async fn send_request(
            &mut self,
            method: &str,
            params: serde_json::Value,
        ) -> Result<serde_json::Value, String> {
            let id = self.next_id;
            self.next_id += 1;

            let req = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params
            });
            let mut line = serde_json::to_string(&req).map_err(|e| e.to_string())?;
            line.push('\n');

            self.stdin
                .write_all(line.as_bytes())
                .await
                .map_err(|e| format!("write stdin: {}", e))?;
            self.stdin
                .flush()
                .await
                .map_err(|e| format!("flush stdin: {}", e))?;

            loop {
                let mut resp_line = String::new();
                let n = self
                    .stdout
                    .read_line(&mut resp_line)
                    .await
                    .map_err(|e| format!("read stdout: {}", e))?;
                if n == 0 {
                    return Err("MCP process closed".to_string());
                }
                let resp_line = resp_line.trim();
                if resp_line.is_empty() {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(resp_line) {
                    if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
                        if let Some(err) = v.get("error") {
                            return Err(format!("MCP error: {}", err));
                        }
                        return Ok(v["result"].clone());
                    }
                }
            }
        }

        async fn send_notification(
            &mut self,
            method: &str,
            params: serde_json::Value,
        ) -> Result<(), String> {
            let notif = serde_json::json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params
            });
            let mut line = serde_json::to_string(&notif).map_err(|e| e.to_string())?;
            line.push('\n');
            self.stdin
                .write_all(line.as_bytes())
                .await
                .map_err(|e| format!("write stdin: {}", e))?;
            self.stdin.flush().await.map_err(|e| e.to_string())?;
            Ok(())
        }

        pub async fn initialize(&mut self) -> Result<(), String> {
            let _result = self
                .send_request(
                    "initialize",
                    serde_json::json!({
                        "protocolVersion": "2024-11-05",
                        "capabilities": { "roots": { "listChanged": false } },
                        "clientInfo": { "name": "gaokao-apply", "version": "0.1.0" }
                    }),
                )
                .await?;
            self.send_notification("notifications/initialized", serde_json::json!({}))
                .await?;
            Ok(())
        }

        pub async fn list_tools(&mut self) -> Result<serde_json::Value, String> {
            self.send_request("tools/list", serde_json::json!({})).await
        }

        pub async fn call_tool(
            &mut self,
            name: &str,
            arguments: serde_json::Value,
        ) -> Result<serde_json::Value, String> {
            self.send_request(
                "tools/call",
                serde_json::json!({ "name": name, "arguments": arguments }),
            )
            .await
        }

        pub async fn shutdown(&mut self) {
            let _ = self.child.kill().await;
        }
    }
}

mod commands {
    use super::{
        app_data_dir, default_settings, mcp::McpProcess, ChatChunk, ChatSettings,
        CutoffData, CutoffEntry, FileDialogResult, FileFilter, McpServerConfig,
    };
    use futures_util::StreamExt;
    use regex::Regex;
    use reqwest::Client;
    use tauri::Emitter;
    use pdf::content::{Op, TextDrawAdjusted};

    #[tauri::command]
    pub async fn mcp_get_tools(
        servers: Vec<McpServerConfig>,
    ) -> Result<serde_json::Value, String> {
        let mut all_tools: Vec<serde_json::Value> = Vec::new();

        for server in servers.iter().filter(|s| s.enabled) {
            let mut proc =
                match McpProcess::spawn(&server.command, &server.args, &server.env).await {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("MCP server {} failed to start: {}", server.id, e);
                        continue;
                    }
                };

            if let Err(e) = proc.initialize().await {
                eprintln!("MCP server {} init failed: {}", server.id, e);
                proc.shutdown().await;
                continue;
            }

            match proc.list_tools().await {
                Ok(result) => {
                    if let Some(tools) = result["tools"].as_array() {
                        for tool in tools {
                            let mut t = tool.clone();
                            if let Some(obj) = t.as_object_mut() {
                                obj.insert(
                                    "_serverId".to_string(),
                                    serde_json::json!(server.id),
                                );
                                obj.insert(
                                    "_serverName".to_string(),
                                    serde_json::json!(server.name),
                                );
                            }
                            all_tools.push(t);
                        }
                    }
                }
                Err(e) => eprintln!("MCP server {} tools/list failed: {}", server.id, e),
            }

            proc.shutdown().await;
        }

        Ok(serde_json::json!(all_tools))
    }

    #[tauri::command]
    pub async fn mcp_call_tool(
        server: McpServerConfig,
        tool_name: String,
        arguments: serde_json::Value,
    ) -> Result<String, String> {
        let mut proc = McpProcess::spawn(&server.command, &server.args, &server.env).await?;
        proc.initialize().await?;
        let result = proc.call_tool(&tool_name, arguments).await?;
        proc.shutdown().await;

        let text = if let Some(contents) = result["content"].as_array() {
            contents
                .iter()
                .filter_map(|c| {
                    if c["type"].as_str() == Some("text") {
                        c["text"].as_str().map(|s| s.to_string())
                    } else {
                        Some(serde_json::to_string(c).unwrap_or_default())
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            serde_json::to_string(&result).unwrap_or_default()
        };

        Ok(text)
    }

    // Resolve server_id for a tool call delta entry
    fn resolve_server_id(
        tc_name: &str,
        tc_server: &str,
        tools: &Option<Vec<serde_json::Value>>,
    ) -> String {
        if !tc_server.is_empty() {
            return tc_server.to_string();
        }
        tools
            .as_ref()
            .and_then(|tl| {
                tl.iter()
                    .find(|t| {
                        t.get("name")
                            .or_else(|| t.get("function").and_then(|f| f.get("name")))
                            .and_then(|n| n.as_str())
                            == Some(tc_name)
                    })
                    .and_then(|t| t["_serverId"].as_str().map(|s| s.to_string()))
            })
            .unwrap_or_default()
    }

    /// Returns the list of tool calls the model wants to make, or an empty list if done.
    /// Text chunks are streamed via "chat-chunk" events.
    #[tauri::command]
    pub async fn chat_stream(
        app: tauri::AppHandle,
        messages: Vec<serde_json::Value>,
        settings: ChatSettings,
        conversation_id: String,
        system_prompt: Option<String>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<Vec<serde_json::Value>, String> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| e.to_string())?;

        let mut all_messages: Vec<serde_json::Value> = Vec::new();

        if let Some(ref system) = system_prompt {
            if !system.trim().is_empty() {
                all_messages.push(serde_json::json!({
                    "role": "system",
                    "content": system
                }));
            }
        }

        for msg in messages {
            all_messages.push(msg);
        }

        let mut request_body = serde_json::json!({
            "model": settings.model,
            "messages": all_messages,
            "stream": true,
            "temperature": settings.temperature,
            "max_tokens": settings.max_tokens,
        });

        if let Some(ref tool_list) = tools {
            if !tool_list.is_empty() {
                let api_tools: Vec<serde_json::Value> = tool_list
                    .iter()
                    .map(|t| {
                        let mut tool = t.clone();
                        if let Some(obj) = tool.as_object_mut() {
                            obj.remove("_serverId");
                            obj.remove("_serverName");
                            // MCP uses "inputSchema", OpenAI expects "parameters"
                            if let Some(schema) = obj.remove("inputSchema") {
                                obj.insert("parameters".to_string(), schema);
                            }
                        }
                        if tool.get("type").is_none() {
                            serde_json::json!({
                                "type": "function",
                                "function": tool
                            })
                        } else {
                            tool
                        }
                    })
                    .collect();
                request_body["tools"] = serde_json::json!(api_tools);
            }
        }

        let base = settings.api_base_url.trim_end_matches('/');
        let url = format!("{}/chat/completions", base);

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", settings.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| format!("Connection failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("API error {}: {}", status, body));
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        // Accumulate streaming tool-call deltas: index → (id, name, args, serverId)
        let mut tool_call_map: std::collections::HashMap<
            usize,
            (String, String, String, String),
        > = std::collections::HashMap::new();
        // Fully assembled tool calls ready to return
        let mut pending_tool_calls: Vec<serde_json::Value> = Vec::new();

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    buffer.push_str(&text);

                    let mut remaining = String::new();
                    let lines: Vec<&str> = buffer.split('\n').collect();
                    let total = lines.len();

                    for (i, line) in lines.iter().enumerate() {
                        let line = line.trim();
                        if i == total - 1 && !buffer.ends_with('\n') {
                            remaining = line.to_string();
                            break;
                        }
                        if let Some(data) = line.strip_prefix("data: ") {
                            if data == "[DONE]" {
                                // Flush any tool calls still in the map (not yet emitted)
                                for (_, (tc_id, tc_name, tc_args, tc_server)) in &tool_call_map {
                                    let server_id =
                                        resolve_server_id(tc_name, tc_server, &tools);
                                    pending_tool_calls.push(serde_json::json!({
                                        "id": tc_id,
                                        "name": tc_name,
                                        "arguments": tc_args,
                                        "serverId": server_id,
                                    }));
                                }
                                return Ok(pending_tool_calls);
                            }

                            if let Ok(json) =
                                serde_json::from_str::<serde_json::Value>(data)
                            {
                                let delta = &json["choices"][0]["delta"];
                                let finish_reason =
                                    json["choices"][0]["finish_reason"].as_str();

                                if let Some(content) = delta["content"].as_str() {
                                    if !content.is_empty() {
                                        let _ = app.emit(
                                            "chat-chunk",
                                            ChatChunk {
                                                conversation_id: conversation_id.clone(),
                                                content: content.to_string(),
                                            },
                                        );
                                    }
                                }

                                if let Some(tcs) = delta["tool_calls"].as_array() {
                                    for tc_delta in tcs {
                                        let idx = tc_delta["index"]
                                            .as_u64()
                                            .unwrap_or(0)
                                            as usize;
                                        let entry =
                                            tool_call_map.entry(idx).or_insert_with(|| {
                                                (
                                                    String::new(),
                                                    String::new(),
                                                    String::new(),
                                                    String::new(),
                                                )
                                            });
                                        if let Some(id) = tc_delta["id"].as_str() {
                                            entry.0 = id.to_string();
                                        }
                                        if let Some(name) =
                                            tc_delta["function"]["name"].as_str()
                                        {
                                            entry.1 = name.to_string();
                                            entry.3 =
                                                resolve_server_id(name, "", &tools);
                                        }
                                        if let Some(args_delta) =
                                            tc_delta["function"]["arguments"].as_str()
                                        {
                                            entry.2.push_str(args_delta);
                                        }
                                    }
                                }

                                if finish_reason == Some("tool_calls") {
                                    for (_, (tc_id, tc_name, tc_args, tc_server)) in
                                        &tool_call_map
                                    {
                                        let server_id =
                                            resolve_server_id(tc_name, tc_server, &tools);
                                        pending_tool_calls.push(serde_json::json!({
                                            "id": tc_id,
                                            "name": tc_name,
                                            "arguments": tc_args,
                                            "serverId": server_id,
                                        }));
                                    }
                                    tool_call_map.clear();
                                }
                            }
                        }
                    }
                    buffer = remaining;
                }
                Err(e) => {
                    return Err(format!("Stream error: {}", e));
                }
            }
        }

        Ok(pending_tool_calls)
    }

    #[tauri::command]
    pub fn get_settings(app: tauri::AppHandle) -> serde_json::Value {
        let path = app_data_dir(&app).join("settings.json");
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(v) = serde_json::from_str(&content) {
                    return v;
                }
            }
        }
        default_settings()
    }

    #[tauri::command]
    pub fn save_settings(
        app: tauri::AppHandle,
        settings: serde_json::Value,
    ) -> Result<(), String> {
        let dir = app_data_dir(&app);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let content = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("settings.json"), content).map_err(|e| e.to_string())?;
        Ok(())
    }

    #[tauri::command]
    pub fn get_conversations(app: tauri::AppHandle) -> Vec<serde_json::Value> {
        let path = app_data_dir(&app).join("conversations.json");
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(v) = serde_json::from_str(&content) {
                    return v;
                }
            }
        }
        Vec::new()
    }

    #[tauri::command]
    pub fn save_conversations(
        app: tauri::AppHandle,
        conversations: Vec<serde_json::Value>,
    ) -> Result<(), String> {
        let dir = app_data_dir(&app);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let content =
            serde_json::to_string_pretty(&conversations).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("conversations.json"), content).map_err(|e| e.to_string())?;
        Ok(())
    }

    #[tauri::command]
    pub fn list_custom_skills(app: tauri::AppHandle) -> Vec<serde_json::Value> {
        let dir = app_data_dir(&app).join("skills");
        let mut skills = Vec::new();
        if !dir.exists() {
            return skills;
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "md") {
                    let id = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let content = std::fs::read_to_string(&path).unwrap_or_default();
                    let description = content
                        .lines()
                        .find(|l| l.starts_with('#'))
                        .map(|l| l.trim_start_matches('#').trim().to_string())
                        .unwrap_or_else(|| id.clone());
                    skills.push(serde_json::json!({
                        "id": id,
                        "name": id,
                        "description": description,
                        "isBuiltIn": false
                    }));
                }
            }
        }
        skills
    }

    #[tauri::command]
    pub fn read_skill_file(
        app: tauri::AppHandle,
        skill_id: String,
    ) -> Result<String, String> {
        let path = app_data_dir(&app)
            .join("skills")
            .join(format!("{}.md", skill_id));
        std::fs::read_to_string(&path).map_err(|e| e.to_string())
    }

    #[tauri::command]
    pub fn save_skill_file(
        app: tauri::AppHandle,
        skill_id: String,
        content: String,
    ) -> Result<(), String> {
        let dir = app_data_dir(&app).join("skills");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(format!("{}.md", skill_id)), content)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    #[tauri::command]
    pub fn delete_skill_file(
        app: tauri::AppHandle,
        skill_id: String,
    ) -> Result<(), String> {
        let path = app_data_dir(&app)
            .join("skills")
            .join(format!("{}.md", skill_id));
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    #[tauri::command]
    pub fn get_gaokao_data(
        app: tauri::AppHandle,
        data_type: String,
    ) -> Result<serde_json::Value, String> {
        let path = app_data_dir(&app)
            .join("gaokao")
            .join(format!("{}.json", data_type));
        if path.exists() {
            let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            serde_json::from_str(&content).map_err(|e| e.to_string())
        } else {
            Ok(serde_json::json!([]))
        }
    }

    #[tauri::command]
    pub fn save_gaokao_data(
        app: tauri::AppHandle,
        data_type: String,
        data: serde_json::Value,
    ) -> Result<(), String> {
        let dir = app_data_dir(&app).join("gaokao");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let content = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(format!("{}.json", data_type)), content)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // 上海2025年高考综合成绩一分一段表 (本科线=402)
    // 来源: sh.bendibao.com, 数据截止 2025年
    // 格式: (分数, 累计人数/位次)
    static SHANGHAI_2025_RANK: &[(u32, u32)] = &[
        (660, 52), (659, 52), (658, 52), (657, 52), (656, 52),
        (655, 52), (654, 52), (653, 52), (652, 52), (651, 52),
        (650, 52), (649, 52), (648, 52), (647, 52), (646, 52),
        (645, 52), (644, 52), (643, 52), (642, 52), (641, 52),
        (640, 52), (639, 52), (638, 52), (637, 52), (636, 52),
        (635, 52), (634, 52), (633, 52), (632, 52), (631, 52),
        (630, 52), (629, 52), (628, 52), (627, 52), (626, 52),
        (625, 52), (624, 52), (623, 52),
        (622, 64),  (621, 75),  (620, 95),  (619, 119), (618, 137),
        (617, 171), (616, 203), (615, 236), (614, 272), (613, 322),
        (612, 365), (611, 399), (610, 460), (609, 537), (608, 600),
        (607, 656), (606, 726), (605, 789), (604, 878), (603, 952),
        (602, 1048),  (601, 1146),  (600, 1250),  (599, 1346),  (598, 1465),
        (597, 1566),  (596, 1671),  (595, 1795),  (594, 1939),  (593, 2093),
        (592, 2237),  (591, 2371),  (590, 2503),  (589, 2641),  (588, 2789),
        (587, 2946),  (586, 3078),  (585, 3231),  (584, 3396),  (583, 3567),
        (582, 3757),  (581, 3934),  (580, 4096),  (579, 4285),  (578, 4467),
        (577, 4649),  (576, 4823),  (575, 5016),  (574, 5214),  (573, 5410),
        (572, 5610),  (571, 5815),  (570, 6004),  (569, 6182),  (568, 6379),
        (567, 6602),  (566, 6814),  (565, 7040),  (564, 7257),  (563, 7447),
        (562, 7663),  (561, 7891),  (560, 8117),  (559, 8336),  (558, 8579),
        (557, 8824),  (556, 9039),  (555, 9302),  (554, 9550),  (553, 9779),
        (552, 10002), (551, 10254), (550, 10506), (549, 10761), (548, 11041),
        (547, 11283), (546, 11525), (545, 11777), (544, 12020), (543, 12291),
        (542, 12532), (541, 12796), (540, 13049), (539, 13322), (538, 13615),
        (537, 13876), (536, 14128), (535, 14414), (534, 14696), (533, 14978),
        (532, 15236), (531, 15514), (530, 15808), (529, 16109), (528, 16393),
        (527, 16655), (526, 16940), (525, 17222), (524, 17497), (523, 17799),
        (522, 18065), (521, 18317), (520, 18580), (519, 18844), (518, 19117),
        (517, 19421), (516, 19710), (515, 19985), (514, 20273), (513, 20546),
        (512, 20807), (511, 21070), (510, 21367), (509, 21654), (508, 21944),
        (507, 22221), (506, 22512), (505, 22797), (504, 23089), (503, 23377),
        (502, 23673), (501, 23963), (500, 24251), (499, 24524), (498, 24798),
        (497, 25079), (496, 25365), (495, 25620), (494, 25917), (493, 26212),
        (492, 26498), (491, 26773), (490, 27085), (489, 27375), (488, 27683),
        (487, 27962), (486, 28229), (485, 28531), (484, 28826), (483, 29108),
        (482, 29358), (481, 29641), (480, 29907), (479, 30198), (478, 30490),
        (477, 30789), (476, 31096), (475, 31381), (474, 31654), (473, 31954),
        (472, 32224), (471, 32490), (470, 32768), (469, 33044), (468, 33357),
        (467, 33627), (466, 33926), (465, 34206), (464, 34489), (463, 34731),
        (462, 35017), (461, 35266), (460, 35524), (459, 35786), (458, 36057),
        (457, 36333), (456, 36580), (455, 36838), (454, 37099), (453, 37376),
        (452, 37654), (451, 37914), (450, 38181), (449, 38464), (448, 38707),
        (447, 38962), (446, 39191), (445, 39429), (444, 39679), (443, 39958),
        (442, 40203), (441, 40472), (440, 40711), (439, 40963), (438, 41209),
        (437, 41436), (436, 41677), (435, 41932), (434, 42167), (433, 42400),
        (432, 42668), (431, 42899), (430, 43131), (429, 43379), (428, 43622),
        (427, 43830), (426, 44086), (425, 44325), (424, 44546), (423, 44773),
        (422, 45012), (421, 45226), (420, 45426), (419, 45675), (418, 45887),
        (417, 46139), (416, 46346), (415, 46550), (414, 46738), (413, 46964),
        (412, 47200), (411, 47401), (410, 47630), (409, 47844), (408, 48030),
        (407, 48244), (406, 48458), (405, 48663), (404, 48868), (403, 49069),
        (402, 49276),
    ];

    /// 查询上海高考成绩对应的位次排名（本地数据，无需网络）
    #[tauri::command]
    pub fn get_local_rank(score: u32, year: Option<String>) -> Result<String, String> {
        let year = year.unwrap_or_else(|| "2025".to_string());
        match year.as_str() {
            "2025" => {
                const BENCH: u32 = 402;
                if score > 660 {
                    return Err(format!("分数{}超出上海高考满分(660分)范围", score));
                }
                if score < BENCH {
                    return Ok(serde_json::json!({
                        "score": score,
                        "year": "2025",
                        "province": "上海",
                        "rank": null,
                        "bench_score": BENCH,
                        "is_above_bench": false,
                        "message": format!("{}分低于2025年上海本科线({}分)，无位次数据", score, BENCH)
                    }).to_string());
                }
                match SHANGHAI_2025_RANK.iter().find(|(s, _)| *s == score) {
                    Some((_, rank)) => Ok(serde_json::json!({
                        "score": score,
                        "year": "2025",
                        "province": "上海",
                        "rank": rank,
                        "bench_score": BENCH,
                        "is_above_bench": true,
                        "total_students": 49276,
                        "message": format!("2025年上海高考{}分，位次第{}名（共约{}人在本科线以上）", score, rank, 49276)
                    }).to_string()),
                    None => Err(format!("未找到2025年上海高考{}分的位次数据", score)),
                }
            }
            _ => Err(format!("暂不支持{}年的上海高考位次查询，目前仅支持2025年", year)),
        }
    }

    /// Parse PDF text to extract cutoff data
    /// Expected format: 院校代码 院校名称(专业组) 投档线
    /// Examples: "10101 复旦大学(01) 580" or "10101 复旦大学(01) 580分及以上"
    fn parse_cutoff_lines(text: &str, _year: u32, _province: &str) -> Vec<CutoffEntry> {
        let mut entries = Vec::new();
        let lines: Vec<&str> = text.lines().collect();

        // Pattern 1: "代码 名称 分数" (space/tab separated)
        // Shanghai format typically: 10101 复旦大学(01) 580
        let re_line = Regex::new(r"^(\d{5,})\s+([^\d]+?)\s+(\d+分?及以上|\d+)$").unwrap();

        for line in lines {
            let line = line.trim();
            if line.is_empty() || line.len() < 10 {
                continue;
            }

            if let Some(caps) = re_line.captures(line) {
                let code = caps.get(1).unwrap().as_str().trim().to_string();
                let name = caps.get(2).unwrap().as_str().trim().to_string();
                let cutoff_str = caps.get(3).unwrap().as_str().trim();

                let cutoff: serde_json::Value = if cutoff_str.contains("分及以上") {
                    serde_json::json!(cutoff_str)
                } else {
                    // Extract numeric part
                    let num_str = cutoff_str.replace("分", "");
                    if let Ok(num) = num_str.parse::<u32>() {
                        serde_json::json!(num)
                    } else {
                        serde_json::json!(cutoff_str)
                    }
                };

                entries.push(CutoffEntry {
                    code,
                    name,
                    cutoff,
                });
            }
        }

        entries
    }

    /// Import cutoff data from PDF file
    #[tauri::command]
    pub async fn import_pdf_cutoffs(
        pdf_path: String,
        year: Option<u32>,
        province: Option<String>,
    ) -> Result<CutoffData, String> {
        let year_val = year.unwrap_or(2025);
        let province_val = province.unwrap_or_else(|| "上海".to_string());

        // Extract text from PDF using the `pdf` crate (more robust than pdf-extract)
        let text = extract_pdf_text(&pdf_path)
            .map_err(|e| format!("PDF解析失败: {}", e))?;

        // Parse cutoff entries from text
        let data = parse_cutoff_lines(&text, year_val, &province_val);

        if data.is_empty() {
            return Err("未能从PDF中解析出分数线数据，请确保PDF包含清晰的表格文本".to_string());
        }

        Ok(CutoffData {
            title: format!(
                "{}年{}普通高校招生本科普通批次平行志愿院校专业组投档分数线",
                year_val, province_val
            ),
            year: year_val,
            province: province_val,
            note: Some("580分及以上考生投档信息另行告知；部分院校Q组、中外合作办学院校专业组投档结果另行公布".to_string()),
            data,
        })
    }


    /// Extract plain text from a PDF using parsed content operators.
    /// This approach avoids font encoding issues by using the parsed text operators directly
    fn extract_pdf_text(path: &str) -> Result<String, String> {
        use pdf::file::FileOptions;

        let doc = FileOptions::uncached().open(path).map_err(|e| e.to_string())?;

        let mut texts: Vec<String> = Vec::new();
        let mut page_count = 0;

        for page_res in doc.pages() {
            let page = match page_res {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("[PDF] page error: {}", e);
                    continue;
                }
            };
            page_count += 1;
            if let Some(content) = &page.contents {
                match content.operations(&doc) {
                    Ok(ops) => {
                        for op in ops {
                            match op {
                                Op::TextDraw { text } => {
                                    let s = text.to_string_lossy();
                                    texts.push(s.to_string());
                                }
                                Op::TextDrawAdjusted { array } => {
                                    for item in array {
                                        if let TextDrawAdjusted::Text(txt) = item {
                                            let s = txt.to_string_lossy();
                                            texts.push(s.to_string());
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => eprintln!("[PDF] ops error on page {}: {}", page_count, e),
                }
            }
        }

        eprintln!("[PDF] pages: {}, texts: {}", page_count, texts.len());

        if texts.is_empty() {
            return Err("未能从PDF中提取到任何文本内容，该PDF可能是扫描件或图片".to_string());
        }

        let mut lines = Vec::new();
        let mut current = String::new();
        for s in texts {
            let s_trim = s.trim();
            if s_trim.is_empty() { continue; }
            if s_trim.chars().all(|c| c.is_ascii_digit()) && s_trim.len() >= 5 {
                if !current.is_empty() {
                    lines.push(current.trim().to_string());
                    current.clear();
                }
                current.push_str(s_trim);
            } else {
                if !current.is_empty() {
                    current.push(' ');
                }
                current.push_str(s_trim);
            }
        }
        if !current.is_empty() {
            lines.push(current.trim().to_string());
        }

        Ok(lines.join("\n"))
        Ok(lines.join("\n"))
    }

    /// Open file dialog to select a PDF file
    #[tauri::command]
    pub async fn open_file_dialog(
        _app: tauri::AppHandle,
        _filters: Option<Vec<FileFilter>>,
        _multiple: Option<bool>,
    ) -> Result<Option<FileDialogResult>, String> {
        // For testing: return the sample PDF path directly
        let pdf_path = "C:\\Users\\Administrator\\workspace\\gaokaoapply\\上海市2025年普通高校招生本科普通批次平行志愿院校专业组投档分数线.pdf";
        println!("[DEBUG] open_file_dialog called, returning path: {}", pdf_path);
        if std::path::Path::new(pdf_path).exists() {
            println!("[DEBUG] PDF file exists, returning Some");
            Ok(Some(FileDialogResult {
                filePath: pdf_path.to_string(),
            }))
        } else {
            println!("[DEBUG] PDF file NOT found!");
            Err("Sample PDF not found at expected path".to_string())
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::chat_stream,
            commands::mcp_get_tools,
            commands::mcp_call_tool,
            commands::get_settings,
            commands::save_settings,
            commands::get_conversations,
            commands::save_conversations,
            commands::list_custom_skills,
            commands::read_skill_file,
            commands::save_skill_file,
            commands::delete_skill_file,
            commands::get_gaokao_data,
            commands::save_gaokao_data,
            commands::get_local_rank,
            commands::import_pdf_cutoffs,
            commands::open_file_dialog,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
