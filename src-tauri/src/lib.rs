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
    /// First ~3000 chars of raw extracted text, for debugging
    pub raw_text_sample: Option<String>,
    /// Set when text extraction succeeded but parsing found 0 entries
    pub parse_warning: Option<String>,
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

            // Prevent a console window from appearing on Windows
            #[cfg(target_os = "windows")]
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

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

    #[tauri::command]
    pub fn list_gaokao_data(app: tauri::AppHandle) -> Vec<serde_json::Value> {
        let dir = app_data_dir(&app).join("gaokao");
        let mut result = Vec::new();
        if !dir.exists() {
            return result;
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "json") {
                    let key = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                            let title = v.get("title").and_then(|t| t.as_str()).unwrap_or(&key).to_string();
                            let year = v.get("year").and_then(|y| y.as_u64()).unwrap_or(0);
                            let province = v.get("province").and_then(|p| p.as_str()).unwrap_or("").to_string();
                            let count = v.get("data").and_then(|d| d.as_array()).map(|a| a.len()).unwrap_or(0);
                            result.push(serde_json::json!({
                                "key": key,
                                "title": title,
                                "year": year,
                                "province": province,
                                "count": count
                            }));
                        }
                    }
                }
            }
        }
        result
    }

    #[tauri::command]
    pub fn delete_gaokao_data(
        app: tauri::AppHandle,
        data_type: String,
    ) -> Result<(), String> {
        let path = app_data_dir(&app)
            .join("gaokao")
            .join(format!("{}.json", data_type));
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
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

    /// Check if a char is a CJK character
    fn is_cjk(c: char) -> bool {
        matches!(c, '\u{4e00}'..='\u{9fff}')
    }

    /// Remove isolated single CJK chars (PDF watermarks) from a string.
    /// Chars that are not adjacent to another CJK char are considered watermarks.
    fn remove_isolated_cjk(s: &str) -> String {
        let chars: Vec<char> = s.chars().collect();
        let n = chars.len();
        let mut result = String::new();
        let mut i = 0;
        while i < n {
            let c = chars[i];
            if is_cjk(c) {
                let prev_cjk = i > 0 && is_cjk(chars[i - 1]);
                let next_cjk = i + 1 < n && is_cjk(chars[i + 1]);
                if !prev_cjk && !next_cjk {
                    // Isolated single CJK — watermark, skip
                    i += 1;
                    continue;
                }
            }
            result.push(c);
            i += 1;
        }
        // Collapse multiple spaces
        let mut out = String::new();
        let mut prev_space = false;
        for c in result.chars() {
            if c == ' ' || c == '\t' {
                if !prev_space { out.push(' '); }
                prev_space = true;
            } else {
                out.push(c);
                prev_space = false;
            }
        }
        out.trim().to_string()
    }

    /// Parse PDF text to extract cutoff data.
    /// Handles watermark single-chars mixed into data lines.
    fn parse_cutoff_lines(text: &str, _year: u32, _province: &str) -> Vec<CutoffEntry> {
        let mut entries = Vec::new();
        // Match optional leading watermark char + 5-digit school code at line start
        let re_code = Regex::new(r"^[\u{4e00}-\u{9fff}]?(\d{5,})").unwrap();

        for line in text.lines() {
            let line = line.trim();
            if line.len() < 8 { continue; }

            // Must start with (optional CJK watermark +) 5+ digit code
            let code_caps = match re_code.captures(line) {
                Some(c) => c,
                None => continue,
            };
            let code = code_caps.get(1).unwrap().as_str().to_string();
            let rest = line[code_caps.get(0).unwrap().end()..].trim();

            // Split rest into whitespace tokens
            // Name = tokens before first 3-4 digit number in score range (300-700)
            // Score = that first number
            let tokens: Vec<&str> = rest.split_whitespace().collect();
            if tokens.is_empty() { continue; }

            let mut name_tokens: Vec<&str> = Vec::new();
            let mut score: Option<u32> = None;

            for token in &tokens {
                // Pure numeric token in plausible score range?
                if let Ok(n) = token.parse::<u32>() {
                    if n >= 300 && n <= 700 && score.is_none() {
                        score = Some(n);
                        break;
                    }
                }
                name_tokens.push(token);
            }

            let Some(score_val) = score else { continue };

            // Build name: join tokens, strip isolated watermark CJK chars
            let raw_name = name_tokens.join(" ");
            let name = remove_isolated_cjk(&raw_name);
            if name.is_empty() { continue; }

            // If the line contains "以上", treat as "NNN分及以上"
            let cutoff: serde_json::Value = if line.contains("以上") {
                serde_json::json!(format!("{}分及以上", score_val))
            } else {
                serde_json::json!(score_val)
            };

            entries.push(CutoffEntry { code, name, cutoff });
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

        // pdf_extract::extract_text is blocking I/O — must run off the async executor
        let path_clone = pdf_path.clone();
        let text = tokio::task::spawn_blocking(move || extract_pdf_text(&path_clone))
            .await
            .map_err(|e| format!("任务调度失败: {}", e))?
            .map_err(|e| format!("PDF解析失败: {}", e))?;

        // Parse cutoff entries from text
        let data = parse_cutoff_lines(&text, year_val, &province_val);

        // Always include a raw text sample (first 3000 chars) so the frontend can show it
        let raw_sample = if text.len() > 3000 {
            format!("{}…（共{}字符）", &text[..3000], text.len())
        } else {
            text.clone()
        };

        let parse_warning = if data.is_empty() {
            Some(format!(
                "提取到 {} 行文本，但未匹配到任何分数线记录。请检查原始文本是否包含院校代码列。",
                text.lines().count()
            ))
        } else {
            None
        };

        Ok(CutoffData {
            title: format!(
                "{}年{}普通高校招生本科普通批次平行志愿院校专业组投档分数线",
                year_val, province_val
            ),
            year: year_val,
            province: province_val,
            note: Some("580分及以上考生投档信息另行告知；部分院校Q组、中外合作办学院校专业组投档结果另行公布".to_string()),
            data,
            raw_text_sample: Some(raw_sample),
            parse_warning,
        })
    }


    /// Extract plain text from a PDF.
    /// First tries pdf-extract; if it panics (e.g. unsupported CID encoding like GBK-EUC-H),
    /// falls back to a lopdf-based extractor that decodes bytes as GBK.
    /// Returns true if the text looks like a legitimate Chinese document
    /// (CJK chars make up >8% of non-whitespace content).
    fn text_quality_ok(text: &str) -> bool {
        let non_ws: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
        if non_ws.is_empty() { return false; }
        if non_ws.len() < 50 { return true; } // short text – accept as-is
        let cjk = non_ws.iter()
            .filter(|&&c| c >= '\u{4E00}' && c <= '\u{9FFF}')
            .count();
        cjk as f64 / non_ws.len() as f64 > 0.08
    }

    fn extract_pdf_text(path: &str) -> Result<String, String> {
        // Try pdf-extract (catches panics from unsupported CID encodings)
        let path_owned = path.to_string();
        let result = std::panic::catch_unwind(move || {
            pdf_extract::extract_text(&path_owned)
        });
        if let Ok(Ok(text)) = result {
            if text_quality_ok(&text) {
                return Ok(text);
            }
        }

        // Fallback 1: lopdf with ToUnicode CMap decoding
        if let Ok(text) = extract_pdf_text_lopdf(path) {
            if text_quality_ok(&text) {
                return Ok(text);
            }
        }

        // Fallback 2: Python pdfplumber (most reliable for CID/CJK PDFs)
        extract_pdf_text_python(path)
    }

    /// Use Python + pdfplumber via `uv run` (auto-installs) or system Python.
    fn extract_pdf_text_python(path: &str) -> Result<String, String> {
        use std::process::Command;
        #[cfg(target_os = "windows")]
        use std::os::windows::process::CommandExt;

        // Inline Python: extract all page text and print to stdout
        let script = format!(
            "import pdfplumber\nwith pdfplumber.open({:?}) as _pdf:\n    print('\\n'.join(pg.extract_text() or '' for pg in _pdf.pages))\n",
            path
        );

        let run_cmd = |cmd: &mut Command| -> Option<String> {
            #[cfg(target_os = "windows")]
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
            cmd.output().ok().and_then(|o| {
                if o.status.success() {
                    let t = String::from_utf8_lossy(&o.stdout).into_owned();
                    if text_quality_ok(&t) { Some(t) } else { None }
                } else {
                    None
                }
            })
        };

        // Try uv run (downloads pdfplumber into isolated env automatically)
        if let Some(text) = run_cmd(
            Command::new("uv").args(["run", "--with", "pdfplumber", "python", "-c", &script])
        ) {
            return Ok(text);
        }

        // Try system python / conda python
        for py in &["python", "python3"] {
            if let Some(text) = run_cmd(Command::new(py).args(["-c", &script])) {
                return Ok(text);
            }
        }

        Err("PDF文本提取失败：所有方法均无法解码该PDF（CID字体）。\n建议：pip install pdfplumber 后重试。".to_string())
    }

    /// lopdf-based PDF text extractor with ToUnicode CMap decoding.
    /// Handles CIDFont / Identity-H encoded PDFs (common for official Chinese government PDFs).
    fn extract_pdf_text_lopdf(path: &str) -> Result<String, String> {
        use lopdf::{Document, Object};
        use lopdf::content::Content;
        use std::collections::HashMap;

        type CMap = HashMap<Vec<u8>, String>;

        // --- CMap helpers ---

        fn parse_hex_bytes(s: &str) -> Option<Vec<u8>> {
            let s = s.strip_prefix('<')?.strip_suffix('>')?;
            if s.len() % 2 != 0 { return None; }
            (0..s.len()).step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
                .collect()
        }

        fn bytes_to_u32(b: &[u8]) -> u32 {
            b.iter().fold(0u32, |acc, &x| (acc << 8) | x as u32)
        }

        fn parse_cmap(data: &[u8]) -> CMap {
            let text = String::from_utf8_lossy(data);
            let mut map = CMap::new();
            let mut in_bfchar = false;
            let mut in_bfrange = false;
            for line in text.lines() {
                let line = line.trim();
                match line {
                    "beginbfchar"  => { in_bfchar = true;  continue; }
                    "endbfchar"    => { in_bfchar = false; continue; }
                    "beginbfrange" => { in_bfrange = true;  continue; }
                    "endbfrange"   => { in_bfrange = false; continue; }
                    _ => {}
                }
                let parts: Vec<&str> = line.split_whitespace().collect();
                if in_bfchar && parts.len() >= 2 {
                    if let (Some(src), Some(dst)) = (parse_hex_bytes(parts[0]), parse_hex_bytes(parts[1])) {
                        if let Some(ch) = char::from_u32(bytes_to_u32(&dst)) {
                            map.insert(src, ch.to_string());
                        }
                    }
                } else if in_bfrange && parts.len() >= 3 && parts[2].starts_with('<') {
                    if let (Some(s_b), Some(e_b), Some(d_b)) = (
                        parse_hex_bytes(parts[0]), parse_hex_bytes(parts[1]), parse_hex_bytes(parts[2])
                    ) {
                        let s_val = bytes_to_u32(&s_b);
                        let e_val = bytes_to_u32(&e_b);
                        let d_val = bytes_to_u32(&d_b);
                        let klen  = s_b.len();
                        for delta in 0..=e_val.saturating_sub(s_val) {
                            if let Some(ch) = char::from_u32(d_val + delta) {
                                let cid = s_val + delta;
                                let key = if klen == 1 { vec![cid as u8] } else { vec![(cid >> 8) as u8, cid as u8] };
                                map.insert(key, ch.to_string());
                            }
                        }
                    }
                }
            }
            map
        }

        fn decode_with_cmap(bytes: &[u8], cmap: &CMap) -> String {
            let mut out = String::new();
            let mut i = 0;
            while i < bytes.len() {
                if i + 1 < bytes.len() {
                    let key2: &[u8] = &[bytes[i], bytes[i + 1]];
                    if let Some(s) = cmap.get(key2) {
                        out.push_str(s);
                        i += 2;
                        continue;
                    }
                }
                let key1: &[u8] = &[bytes[i]];
                if let Some(s) = cmap.get(key1) {
                    out.push_str(s);
                }
                i += 1;
            }
            out
        }

        // --- Main extraction ---

        let doc = Document::load(path).map_err(|e| format!("无法加载PDF: {}", e))?;
        let mut all_text = String::new();

        // Helper: dereference Object::Reference, returning cloned owned Object
        let deref_obj = |obj: &Object| -> Option<Object> {
            match obj {
                Object::Reference(id) => doc.get_object(*id).ok().cloned(),
                other => Some(other.clone()),
            }
        };

        for page_id in doc.page_iter() {
            // Build font-name -> CMap table for this page
            let mut font_cmaps: HashMap<String, CMap> = HashMap::new();

            'font_load: {
                let page_obj  = match doc.get_object(page_id).ok().cloned() { Some(o) => o, None => break 'font_load };
                let page_dict = match page_obj.as_dict().ok().cloned()       { Some(d) => d, None => break 'font_load };
                let res_obj   = match page_dict.get(b"Resources").ok().and_then(|r| deref_obj(r)) { Some(o) => o, None => break 'font_load };
                let res_dict  = match res_obj.as_dict().ok().cloned()        { Some(d) => d, None => break 'font_load };
                let fonts_obj = match res_dict.get(b"Font").ok().and_then(|f| deref_obj(f)) { Some(o) => o, None => break 'font_load };
                let fonts_dict = match fonts_obj.as_dict().ok().cloned()     { Some(d) => d, None => break 'font_load };

                for (fname, font_ref) in fonts_dict.iter() {
                    let font_name = String::from_utf8_lossy(fname).to_string();
                    let font_obj  = match deref_obj(font_ref)                { Some(o) => o, None => continue };
                    let font_dict = match font_obj.as_dict().ok().cloned()   { Some(d) => d, None => continue };
                    let tu_ref    = match font_dict.get(b"ToUnicode").ok().cloned() { Some(r) => r, None => continue };
                    let tu_obj    = match deref_obj(&tu_ref)                 { Some(o) => o, None => continue };
                    if let Object::Stream(stream) = tu_obj {
                        if let Ok(data) = stream.decompressed_content() {
                            let cmap = parse_cmap(&data);
                            if !cmap.is_empty() {
                                font_cmaps.insert(font_name, cmap);
                            }
                        }
                    }
                }
            }

            let stream_bytes = match doc.get_page_content(page_id) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let content = match Content::decode(&stream_bytes) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let mut in_text = false;
            let mut current_font: Option<String> = None;

            for op in &content.operations {
                match op.operator.as_str() {
                    "BT" => { in_text = true; }
                    "ET" => { in_text = false; all_text.push('\n'); }
                    "Td" | "TD" | "T*" => { all_text.push('\n'); }
                    "Tf" if in_text => {
                        if let Some(Object::Name(name)) = op.operands.first() {
                            current_font = Some(String::from_utf8_lossy(name).to_string());
                        }
                    }
                    "Tj" | "'" if in_text => {
                        if let Some(Object::String(bytes, _)) = op.operands.first() {
                            if let Some(cm) = current_font.as_ref().and_then(|f| font_cmaps.get(f)) {
                                all_text.push_str(&decode_with_cmap(bytes, cm));
                            } else {
                                push_pdf_string(&mut all_text, bytes);
                            }
                        }
                    }
                    "TJ" if in_text => {
                        if let Some(Object::Array(arr)) = op.operands.first() {
                            let cmap = current_font.as_ref().and_then(|f| font_cmaps.get(f));
                            for item in arr {
                                if let Object::String(bytes, _) = item {
                                    if let Some(cm) = cmap {
                                        all_text.push_str(&decode_with_cmap(bytes, cm));
                                    } else {
                                        push_pdf_string(&mut all_text, bytes);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        if all_text.trim().is_empty() {
            Err("PDF中未能提取到文本（可能为扫描件或加密PDF）".to_string())
        } else {
            Ok(all_text)
        }
    }

    /// Decode PDF string bytes: try GBK for high-byte sequences, else UTF-8 lossy.
    fn push_pdf_string(out: &mut String, bytes: &[u8]) {
        if bytes.iter().any(|&b| b > 0x7F) {
            let (text, _, had_errors) = encoding_rs::GBK.decode(bytes);
            if !had_errors {
                out.push_str(&text);
                return;
            }
        }
        out.push_str(&String::from_utf8_lossy(bytes));
    }

    /// Open file dialog to select a PDF file
    #[tauri::command]
    pub async fn open_file_dialog(
        app: tauri::AppHandle,
        _filters: Option<Vec<FileFilter>>,
        _multiple: Option<bool>,
    ) -> Result<Option<FileDialogResult>, String> {
        use tauri_plugin_dialog::DialogExt;

        let path = app
            .dialog()
            .file()
            .add_filter("PDF 文件", &["pdf"])
            .blocking_pick_file();

        match path {
            Some(file_path) => Ok(Some(FileDialogResult {
                filePath: file_path.to_string(),
            })),
            None => Ok(None),
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
            commands::list_gaokao_data,
            commands::delete_gaokao_data,
            commands::get_local_rank,
            commands::import_pdf_cutoffs,
            commands::open_file_dialog,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
