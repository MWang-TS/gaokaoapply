import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { v4 as uuidv4 } from "uuid";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type {
  View,
  ChatMessage,
  Conversation,
  AppSettings,
  Skill,
  McpServer,
  GaokaoSchool,
  PdfCutoffEntry,
} from "./types";
import zhangxuefengRaw from "./data/skills/zhangxuefeng.md?raw";
import shanghaiSchools from "./data/gaokao/shanghai_schools.json";
import cutoffsData from "./data/gaokao/shanghai_2025_cutoffs.json";
import "./index.css";
import "./App.css";

// ─── Built-in skills ───────────────────────────────────────────────────────
const BUILTIN_SKILLS: Skill[] = [
  {
    id: "zhangxuefeng",
    name: "张雪峰",
    description: "张雪峰高考志愿填报思维框架 · 社会筛子论 · 就业倒推法",
    content: zhangxuefengRaw,
    isBuiltIn: true,
    icon: "🎯",
  },
  {
    id: "general",
    name: "通用助手",
    description: "标准 AI 助手模式，不使用特定 Skill",
    content: `你是一个专业的高考志愿填报助手，拥有丰富的高校信息、专业就业数据和志愿填报经验。
请用专业、客观、友好的方式帮助用户分析志愿填报相关问题。
注意结合用户的分数、省份、兴趣和家庭情况给出个性化建议。`,
    isBuiltIn: true,
    icon: "🤖",
  },
];

// MCP tool descriptor as returned by mcp_get_tools
interface McpTool {
  name: string;
  description?: string;
  inputSchema?: Record<string, unknown>;
  _serverId: string;
  _serverName: string;
  [key: string]: unknown;
}

// ─── Local cutoffs query ───────────────────────────────────────────────────
type CutoffEntry = {
  code: string;
  name: string;
  cutoff: number | string;
  chinese_math?: number;
  higher_score?: number;
  english?: number;
  subject1?: number;
  subject2?: number;
  subject3?: number;
  bonus?: number;
};

const CUTOFFS: CutoffEntry[] = (cutoffsData as { data: CutoffEntry[] }).data;

function queryCutoffs(args: Record<string, unknown>): string {
  const score = args.score as number | undefined;
  const keyword = (args.name as string | undefined)?.trim();

  // Search by college name keyword
  if (keyword && !score) {
    const matches = CUTOFFS.filter((e) =>
      e.name.includes(keyword) || e.code === keyword
    );
    if (matches.length === 0) {
      return JSON.stringify({ error: `未找到包含"${keyword}"的院校专业组` });
    }
    return JSON.stringify({
      query: { name: keyword },
      count: matches.length,
      results: matches.slice(0, 20),
    });
  }

  // Search by score: return colleges where cutoff <= score
  if (score !== undefined) {
    const eligible = CUTOFFS.filter(
      (e) => typeof e.cutoff === "number" && e.cutoff <= score
    ) as (CutoffEntry & { cutoff: number })[];
    // Also list 580+ schools as "需要580分以上"
    const above580 = CUTOFFS.filter((e) => e.cutoff === "580分及以上");

    // Sort by cutoff descending (highest competitive ones first)
    eligible.sort((a, b) => b.cutoff - a.cutoff);

    // If keyword provided, also filter by name
    const filtered = keyword
      ? eligible.filter((e) => e.name.includes(keyword))
      : eligible;

    return JSON.stringify({
      query: { score, name: keyword },
      eligible_count: eligible.length,
      above_580_schools: above580.length,
      note: `分数${score}可投档的院校专业组共${eligible.length}个（另有${above580.length}个院校专业组需要580分以上）`,
      results: filtered.slice(0, 30),
    });
  }

  return JSON.stringify({ error: "请提供score（分数）或name（院校名称关键词）参数" });
}

// Local tools built into the app (no MCP server required)
const LOCAL_TOOLS: McpTool[] = [
  {
    name: "get_shanghai_rank",
    description: "查询上海高考2025年综合成绩对应的位次排名（本地数据）。适用于：用户询问上海考生某分数能排第几名、位次多少等问题。",
    inputSchema: {
      type: "object",
      properties: {
        score: { type: "integer", description: "上海高考综合成绩（整数，100-660）" },
        year:  { type: "string",  description: "年份，目前仅支持\"2025\"", default: "2025" },
      },
      required: ["score"],
    },
    _serverId: "local",
    _serverName: "本地数据",
  },
  {
    name: "query_college_cutoffs",
    description: "查询上海2025年高校投档分数线数据库（本地数据，无需网络）。可按分数查询能投档的院校列表，或按院校名称关键词查询其投档线。每条结果包含：院校代码、院校专业组名称、投档线、语数合计、外语成绩、三门选考科目分数。",
    inputSchema: {
      type: "object",
      properties: {
        score: { type: "integer", description: "考生综合成绩，查询该分数可投档的所有院校专业组" },
        name:  { type: "string",  description: "院校名称关键词，如\"复旦\"、\"交大\"、\"同济\"，用于精确查找某校投档线" },
      },
    },
    _serverId: "local",
    _serverName: "本地数据",
  },
];

const DEFAULT_SETTINGS: AppSettings = {
  apiBaseUrl: "https://api.openai.com/v1",
  apiKey: "",
  model: "gpt-4o",
  temperature: 0.7,
  maxTokens: 2000,
  activeSkillId: "zhangxuefeng",
  theme: "light",
};

// ─── App ───────────────────────────────────────────────────────────────────
export default function App() {
  const [view, setView] = useState<View>("chat");
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS);
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeConvId, setActiveConvId] = useState<string>("");
  const [customSkills, setCustomSkills] = useState<Skill[]>([]);
  const [isStreaming, setIsStreaming] = useState(false);
  const [streamError, setStreamError] = useState<string | null>(null);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [mcpTools, setMcpTools] = useState<McpTool[]>([]);
  const [toolCallStatus, setToolCallStatus] = useState<string | null>(null);
  const initialized = useRef(false);

  const allSkills = [...BUILTIN_SKILLS, ...customSkills];

  // Load persisted data on mount
  useEffect(() => {
    if (initialized.current) return;
    initialized.current = true;

    (async () => {
      try {
        const s = await invoke<AppSettings>("get_settings");
        setSettings((prev) => ({ ...prev, ...s }));
        document.documentElement.setAttribute(
          "data-theme",
          s.theme ?? "light"
        );
      } catch {
        /* use defaults */
      }

      try {
        const convs = await invoke<Conversation[]>("get_conversations");
        if (convs.length > 0) {
          setConversations(convs);
          setActiveConvId(convs[0].id);
        }
      } catch {
        /* start fresh */
      }
    })();
  }, []);

  // Load MCP tools when mcpServers config actually changes (not on every settings ref change)
  const mcpServersKey = JSON.stringify(
    ((settings as AppSettings & { mcpServers?: McpServer[] }).mcpServers ?? [])
      .filter((s) => s.enabled)
      .map((s) => `${s.id}:${s.command}:${s.args.join(",")}`)
  );
  useEffect(() => {
    const servers = (settings as AppSettings & { mcpServers?: McpServer[] }).mcpServers ?? [];
    const enabled = servers.filter((s) => s.enabled);
    if (enabled.length === 0) {
      setMcpTools([]);
      return;
    }
    invoke<McpTool[]>("mcp_get_tools", { servers })
      .then((tools) => setMcpTools(tools))
      .catch(() => setMcpTools([]));
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mcpServersKey]);

  const activeConv = conversations.find((c) => c.id === activeConvId) ?? null;
  const activeSkill =
    allSkills.find((s) => s.id === settings.activeSkillId) ?? allSkills[0];

  // ── Conversation helpers ──────────────────────────────────────────────
  const newConversation = useCallback(() => {
    const conv: Conversation = {
      id: uuidv4(),
      title: "新对话",
      messages: [],
      createdAt: Date.now(),
      updatedAt: Date.now(),
      skillId: settings.activeSkillId,
    };
    setConversations((prev) => {
      const updated = [conv, ...prev];
      invoke("save_conversations", { conversations: updated }).catch(() => {});
      return updated;
    });
    setActiveConvId(conv.id);
    setView("chat");
  }, [settings.activeSkillId]);

  const deleteConversation = useCallback(
    (id: string) => {
      setConversations((prev) => {
        const updated = prev.filter((c) => c.id !== id);
        invoke("save_conversations", { conversations: updated }).catch(
          () => {}
        );
        if (activeConvId === id) {
          setActiveConvId(updated[0]?.id ?? "");
        }
        return updated;
      });
    },
    [activeConvId]
  );

  const updateConversation = useCallback((updated: Conversation) => {
    setConversations((prev) => {
      const next = prev.map((c) => (c.id === updated.id ? updated : c));
      invoke("save_conversations", { conversations: next }).catch(() => {});
      return next;
    });
  }, []);

  // ── Settings helpers ───────────────────────────────────────────────────
  const saveSettings = useCallback(async (updated: AppSettings) => {
    setSettings(updated);
    document.documentElement.setAttribute("data-theme", updated.theme);
    await invoke("save_settings", { settings: updated }).catch(() => {});
  }, []);

  // ── Streaming chat with MCP tool-call loop ─────────────────────────────
  const sendMessage = useCallback(
    async (content: string, baseConv?: Conversation) => {
      if (!content.trim() || isStreaming) return;

      let conv = baseConv ?? activeConv;
      if (!conv) {
        const newConv: Conversation = {
          id: uuidv4(),
          title: content.slice(0, 30) + (content.length > 30 ? "…" : ""),
          messages: [],
          createdAt: Date.now(),
          updatedAt: Date.now(),
          skillId: settings.activeSkillId,
        };
        conv = newConv;
        setConversations((prev) => {
          const updated = [newConv, ...prev];
          invoke("save_conversations", { conversations: updated }).catch(() => {});
          return updated;
        });
        setActiveConvId(newConv.id);
      }

      const userMsg: ChatMessage = {
        id: uuidv4(),
        role: "user",
        content,
        timestamp: Date.now(),
      };

      const assistantMsgId = uuidv4();
      const assistantMsg: ChatMessage = {
        id: assistantMsgId,
        role: "assistant",
        content: "",
        timestamp: Date.now(),
        isStreaming: true,
      };

      const convBase: Conversation = {
        ...conv,
        title:
          conv.messages.length === 0
            ? content.slice(0, 30) + (content.length > 30 ? "…" : "")
            : conv.title,
        messages: [...conv.messages, userMsg, assistantMsg],
        updatedAt: Date.now(),
      };
      updateConversation(convBase);

      setIsStreaming(true);
      setStreamError(null);
      setToolCallStatus(null);

      const convId = convBase.id;
      // messages tracked outside React state for the tool loop
      // (React state is async; we need synchronous access inside listeners)
      const msgHistory: { role: string; content: string; tool_call_id?: string; tool_calls?: unknown }[] = [
        ...convBase.messages
          .filter((m) => m.role !== "system" && m.id !== assistantMsgId)
          .map((m) => ({ role: m.role, content: m.content })),
      ];

      let accumulated = "";

      // rAF handle: batch chunk renders to at most one re-render per animation frame (~60fps)
      let rafId: number | null = null;

      const unlistenChunk = await listen<{ conversationId: string; content: string }>(
        "chat-chunk",
        (event) => {
          if (event.payload.conversationId !== convId) return;
          accumulated += event.payload.content;
          if (rafId !== null) return; // already scheduled
          rafId = requestAnimationFrame(() => {
            rafId = null;
            const snapshot = accumulated;
            setConversations((prev) =>
              prev.map((c) => {
                if (c.id !== convId) return c;
                return {
                  ...c,
                  messages: c.messages.map((m) =>
                    m.id === assistantMsgId ? { ...m, content: snapshot } : m
                  ),
                };
              })
            );
          });
        }
      );

      const cleanup = () => {
        unlistenChunk();
        if (rafId !== null) { cancelAnimationFrame(rafId); rafId = null; }
      };

      // Agentic loop: invoke chat_stream → returns tool calls (or []) → execute → repeat
      const MAX_ROUNDS = 10;
      for (let round = 0; round < MAX_ROUNDS; round++) {
        accumulated = "";

        try {
          // chat_stream streams chunks via "chat-chunk" events,
          // and returns the list of tool calls (empty = done).
          const toolCalls = await invoke<{
            id: string;
            name: string;
            arguments: string;
            serverId: string;
          }[]>("chat_stream", {
            messages: msgHistory,
            settings: {
              apiBaseUrl: settings.apiBaseUrl,
              apiKey: settings.apiKey,
              model: settings.model,
              temperature: settings.temperature,
              maxTokens: settings.maxTokens,
            },
            conversationId: convId,
            systemPrompt: activeSkill?.content ?? null,
            tools: [...mcpTools, ...LOCAL_TOOLS],
          });

          if (toolCalls.length === 0) {
            // Done — finalize the assistant message
            cleanup();
            setIsStreaming(false);
            setToolCallStatus(null);
            setConversations((prev) => {
              const next = prev.map((c) => {
                if (c.id !== convId) return c;
                return {
                  ...c,
                  messages: c.messages.map((m) =>
                    m.id === assistantMsgId ? { ...m, isStreaming: false } : m
                  ),
                };
              });
              invoke("save_conversations", { conversations: next }).catch(() => {});
              return next;
            });
            break;
          }

          // Record the assistant's tool_calls turn
          msgHistory.push({
            role: "assistant",
            content: accumulated,
            tool_calls: toolCalls.map((c) => ({
              id: c.id,
              type: "function",
              function: { name: c.name, arguments: c.arguments },
            })),
          });

          // Execute each tool call
          for (const call of toolCalls) {
            setToolCallStatus(`⚙️ 调用工具：${call.name}`);

            let parsedArgs: Record<string, unknown> = {};
            try { parsedArgs = JSON.parse(call.arguments || "{}"); } catch { parsedArgs = {}; }

            try {
              let result: string;

              if (call.serverId === "local") {
                // Dispatch to built-in Tauri commands
                if (call.name === "get_shanghai_rank") {
                  result = await invoke<string>("get_local_rank", {
                    score: parsedArgs.score as number,
                    year: (parsedArgs.year as string | undefined) ?? "2025",
                  });
                } else if (call.name === "query_college_cutoffs") {
                  result = queryCutoffs(parsedArgs);
                } else {
                  result = JSON.stringify({ error: `未知本地工具: ${call.name}` });
                }
              } else {
                const servers = (settings as AppSettings & { mcpServers?: McpServer[] }).mcpServers ?? [];
                const serverConfig = servers.find((s) => s.id === call.serverId);
                if (!serverConfig) {
                  msgHistory.push({
                    role: "tool",
                    content: `错误：找不到服务器 ${call.serverId}`,
                    tool_call_id: call.id,
                  });
                  continue;
                }
                result = await invoke<string>("mcp_call_tool", {
                  server: serverConfig,
                  toolName: call.name,
                  arguments: parsedArgs,
                });
              }
              msgHistory.push({
                role: "tool",
                content: result,
                tool_call_id: call.id,
              });

              // Show tool result in the conversation
              setConversations((prev) =>
                prev.map((c) => {
                  if (c.id !== convId) return c;
                  const toolMsg: ChatMessage = {
                    id: uuidv4(),
                    role: "assistant" as const,
                    content: result,
                    timestamp: Date.now(),
                    toolName: call.name,
                  };
                  return { ...c, messages: [...c.messages, toolMsg] };
                })
              );
            } catch (toolErr) {
              msgHistory.push({
                role: "tool",
                content: `错误：${String(toolErr)}`,
                tool_call_id: call.id,
              });
            }
          }

          setToolCallStatus("⚙️ AI 正在处理工具结果…");
          // Reset assistant message for next round
          accumulated = "";
          setConversations((prev) =>
            prev.map((c) => {
              if (c.id !== convId) return c;
              return {
                ...c,
                messages: c.messages.map((m) =>
                  m.id === assistantMsgId ? { ...m, content: "", isStreaming: true } : m
                ),
              };
            })
          );
        } catch (err) {
          cleanup();
          setIsStreaming(false);
          setToolCallStatus(null);
          setStreamError(String(err));
          setConversations((prev) =>
            prev.map((c) => {
              if (c.id !== convId) return c;
              return {
                ...c,
                messages: c.messages.map((m) =>
                  m.id === assistantMsgId
                    ? { ...m, content: `❌ 错误：${String(err)}`, isStreaming: false }
                    : m
                ),
              };
            })
          );
          break;
        }
      }
    },
    [activeConv, isStreaming, settings, activeSkill, updateConversation, mcpTools]
  );

  const regenerate = useCallback(() => {
    if (!activeConv || isStreaming) return;
    const msgs = activeConv.messages;
    let lastUserIdx = -1;
    for (let i = msgs.length - 1; i >= 0; i--) {
      if (msgs[i].role === "user") { lastUserIdx = i; break; }
    }
    if (lastUserIdx === -1) return;
    const lastUserContent = msgs[lastUserIdx].content;
    const trimmedConv: Conversation = { ...activeConv, messages: msgs.slice(0, lastUserIdx) };
    sendMessage(lastUserContent, trimmedConv);
  }, [activeConv, isStreaming, sendMessage]);

  return (
    <div className="app-layout">
      {/* Icon sidebar */}
      <nav className="icon-nav">
        <div className="icon-nav-top">
          <div className="app-logo" title="高考报志愿助手">
            🎓
          </div>
          <NavIcon
            icon="💬"
            label="对话"
            active={view === "chat"}
            onClick={() => setView("chat")}
          />
          <NavIcon
            icon="📚"
            label="技能"
            active={view === "skills"}
            onClick={() => setView("skills")}
          />
          <NavIcon
            icon="🔧"
            label="MCP"
            active={view === "mcp"}
            onClick={() => setView("mcp")}
          />
          <NavIcon
            icon="📊"
            label="高考数据"
            active={view === "data"}
            onClick={() => setView("data")}
          />
        </div>
        <div className="icon-nav-bottom">
          <NavIcon
            icon="⚙️"
            label="设置"
            active={view === "settings"}
            onClick={() => setView("settings")}
          />
          <NavIcon
            icon="ℹ️"
            label="关于"
            active={view === "about"}
            onClick={() => setView("about")}
          />
        </div>
      </nav>

      {/* Chat history sidebar (only in chat view) */}
      {view === "chat" && sidebarOpen && (
        <aside className="chat-sidebar">
          <div className="chat-sidebar-header">
            <span className="chat-sidebar-title">对话历史</span>
            <button
              className="icon-btn"
              onClick={newConversation}
              title="新建对话"
            >
              ✏️
            </button>
          </div>
          <div className="conversation-list">
            {conversations.length === 0 && (
              <div className="empty-hint">还没有对话，点击 ✏️ 开始</div>
            )}
            {conversations.map((c) => (
              <div
                key={c.id}
                className={`conversation-item ${c.id === activeConvId ? "active" : ""}`}
                onClick={() => setActiveConvId(c.id)}
              >
                <span className="conv-title">{c.title}</span>
                <button
                  className="conv-delete"
                  onClick={(e) => {
                    e.stopPropagation();
                    deleteConversation(c.id);
                  }}
                  title="删除"
                >
                  ×
                </button>
              </div>
            ))}
          </div>
          <div className="chat-sidebar-footer">
            <div className="active-skill-badge">
              <span>{activeSkill?.icon ?? "🤖"}</span>
              <span>{activeSkill?.name}</span>
            </div>
          </div>
        </aside>
      )}

      {/* Main content */}
      <main className="main-content">
        {view === "chat" && (
          <ChatView
            conversation={activeConv}
            isStreaming={isStreaming}
            streamError={streamError}
            toolCallStatus={toolCallStatus}
            mcpToolCount={mcpTools.length}
            onSend={sendMessage}
            onRegenerate={regenerate}
            onNewConv={newConversation}
            onToggleSidebar={() => setSidebarOpen((v) => !v)}
            sidebarOpen={sidebarOpen}
            activeSkill={activeSkill}
            settings={settings}
            allSkills={allSkills}
            onSkillChange={(id) =>
              saveSettings({ ...settings, activeSkillId: id })
            }
          />
        )}
        {view === "settings" && (
          <SettingsView settings={settings} onSave={saveSettings} />
        )}
        {view === "skills" && (
          <SkillsView
            skills={allSkills}
            activeSkillId={settings.activeSkillId}
            onActivate={(id) =>
              saveSettings({ ...settings, activeSkillId: id })
            }
            onRefresh={() => {
              invoke<{ id: string; name: string; description: string; isBuiltIn: boolean }[]>(
                "list_custom_skills"
              )
                .then(async (list) => {
                  const loaded: Skill[] = [];
                  for (const item of list) {
                    const content = await invoke<string>("read_skill_file", {
                      skillId: item.id,
                    }).catch(() => "");
                    loaded.push({ ...item, content, icon: "📝" });
                  }
                  setCustomSkills(loaded);
                })
                .catch(() => {});
            }}
          />
        )}
        {view === "mcp" && (
          <McpView
            mcpServers={
              (settings as AppSettings & { mcpServers?: McpServer[] })
                .mcpServers ?? []
            }
            onSave={(servers) =>
              saveSettings({
                ...settings,
                ...({ mcpServers: servers } as unknown as AppSettings),
              })
            }
          />
        )}
        {view === "data" && (
          <DataView schools={shanghaiSchools as GaokaoSchool[]} />
        )}
        {view === "about" && <AboutView />}
      </main>
    </div>
  );
}

// ─── Nav icon ─────────────────────────────────────────────────────────────
function NavIcon({
  icon,
  label,
  active,
  onClick,
}: {
  icon: string;
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      className={`nav-icon-btn ${active ? "active" : ""}`}
      onClick={onClick}
      title={label}
    >
      <span className="nav-icon">{icon}</span>
      <span className="nav-label">{label}</span>
    </button>
  );
}

// ─── Chat view ────────────────────────────────────────────────────────────
function ChatView({
  conversation,
  isStreaming,
  streamError,
  toolCallStatus,
  mcpToolCount,
  onSend,
  onRegenerate,
  onNewConv,
  onToggleSidebar,
  sidebarOpen,
  activeSkill,
  allSkills,
  onSkillChange,
}: {
  conversation: Conversation | null;
  isStreaming: boolean;
  streamError: string | null;
  toolCallStatus: string | null;
  mcpToolCount: number;
  onSend: (content: string) => void;
  onRegenerate: () => void;
  onNewConv: () => void;
  onToggleSidebar: () => void;
  sidebarOpen: boolean;
  activeSkill: Skill | undefined;
  settings: AppSettings;
  allSkills: Skill[];
  onSkillChange: (id: string) => void;
}) {
  const [input, setInput] = useState("");
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [conversation?.messages]);

  const handleSend = () => {
    const text = input.trim();
    if (!text || isStreaming) return;
    setInput("");
    onSend(text);
    if (textareaRef.current) {
      textareaRef.current.style.height = "auto";
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  const handleTextareaChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setInput(e.target.value);
    e.target.style.height = "auto";
    e.target.style.height = Math.min(e.target.scrollHeight, 200) + "px";
  };

  return (
    <div className="chat-view">
      {/* Header */}
      <div className="chat-header">
        <div className="chat-header-left">
          <button
            className="icon-btn"
            onClick={onToggleSidebar}
            title={sidebarOpen ? "收起侧栏" : "展开侧栏"}
          >
            {sidebarOpen ? "◀" : "▶"}
          </button>
          <button className="icon-btn" onClick={onNewConv} title="新建对话">
            ✏️ 新对话
          </button>
        </div>
        <div className="chat-header-center">
          <span className="chat-header-title">
            {conversation?.title ?? "高考报志愿助手"}
          </span>
        </div>
        <div className="chat-header-right">
          <select
            className="skill-select"
            value={activeSkill?.id ?? ""}
            onChange={(e) => onSkillChange(e.target.value)}
            title="选择技能"
          >
            {allSkills.map((s) => (
              <option key={s.id} value={s.id}>
                {s.icon ?? "📝"} {s.name}
              </option>
            ))}
          </select>
        </div>
      </div>

      {/* Messages */}
      <div className="messages-area">
        {(!conversation || conversation.messages.length === 0) && (
          <WelcomeScreen activeSkill={activeSkill} onSend={onSend} />
        )}
        {conversation?.messages.map((msg) => (
          <MessageBubble key={msg.id} message={msg} />
        ))}
        {streamError && (
          <div className="error-banner">⚠️ {streamError}</div>
        )}
        {toolCallStatus && (
          <div className="tool-call-banner">{toolCallStatus}</div>
        )}
        {!isStreaming && conversation && conversation.messages.some((m) => m.role === "assistant") && (
          <div className="regenerate-row">
            <button className="regenerate-btn" onClick={onRegenerate} title="重新生成最后一条回复">
              ↺ 重新生成
            </button>
          </div>
        )}
        <div ref={messagesEndRef} />
      </div>

      {/* Input */}
      <div className="input-area">
        <div className="input-wrapper">
          <textarea
            ref={textareaRef}
            className="chat-input"
            value={input}
            onChange={handleTextareaChange}
            onKeyDown={handleKeyDown}
            placeholder={
              isStreaming ? "AI 正在思考中…" : "输入问题，Enter 发送，Shift+Enter 换行"
            }
            disabled={isStreaming}
            rows={1}
          />
          <button
            className={`send-btn ${isStreaming ? "loading" : ""}`}
            onClick={handleSend}
            disabled={isStreaming || !input.trim()}
            title="发送"
          >
            {isStreaming ? (
              <span className="spinner" />
            ) : (
              "↑"
            )}
          </button>
        </div>
        <div className="input-hint">
          当前技能：{activeSkill?.icon} {activeSkill?.name}
          {mcpToolCount > 0 && <span className="mcp-badge"> · 🔧 {mcpToolCount} 工具</span>}
          {" · "}
          {isStreaming && <span className="streaming-indicator">● {toolCallStatus ?? "生成中"}</span>}
          {!isStreaming && <span>Enter 发送</span>}
        </div>
      </div>
    </div>
  );
}

// ─── Welcome screen ───────────────────────────────────────────────────────
function WelcomeScreen({
  activeSkill,
  onSend,
}: {
  activeSkill: Skill | undefined;
  onSend: (msg: string) => void;
}) {
  const suggestions = [
    "我孩子今年上海高考 580 分，想学计算机，有哪些学校可以报？",
    "上海 560 分，理科，应该怎么平衡学校和专业的选择？",
    "金融专业就业前景怎么样？普通家庭值得报吗？",
    "生化环材真的是天坑专业吗？我孩子喜欢化学怎么办？",
    "上海本地孩子报志愿有什么特别需要注意的地方？",
    "冲稳保策略应该怎么制定？比例大概是多少？",
  ];

  return (
    <div className="welcome-screen">
      <div className="welcome-icon">{activeSkill?.icon ?? "🎓"}</div>
      <h2 className="welcome-title">
        {activeSkill?.name ?? "高考报志愿助手"}
      </h2>
      <p className="welcome-desc">{activeSkill?.description}</p>
      <div className="suggestion-grid">
        {suggestions.map((s) => (
          <button key={s} className="suggestion-btn" onClick={() => onSend(s)}>
            {s}
          </button>
        ))}
      </div>
    </div>
  );
}

// ─── Message bubble ────────────────────────────────────────────────────────
function parseThinking(content: string): { thinking: string; text: string } {
  const match = content.match(/^<think>([\s\S]*?)<\/think>\s*/);
  if (match) {
    return { thinking: match[1].trim(), text: content.slice(match[0].length) };
  }
  // Streaming: unclosed <think> block — everything after is thinking so far
  const openMatch = content.match(/^<think>([\s\S]*)$/);
  if (openMatch) {
    return { thinking: openMatch[1], text: "" };
  }
  return { thinking: "", text: content };
}

function MessageBubble({ message }: { message: ChatMessage }) {
  const isUser = message.role === "user";

  // Tool result bubble — hidden from conversation
  if (message.toolName) {
    return null;
  }

  const { thinking, text } = isUser
    ? { thinking: "", text: message.content }
    : parseThinking(message.content || "");

  return (
    <div className={`message-row ${isUser ? "user" : "assistant"}`}>
      <div className="message-avatar">{isUser ? "👤" : "🤖"}</div>
      <div className={`message-bubble ${isUser ? "user" : "assistant"}`}>
        {isUser ? (
          <span style={{ whiteSpace: "pre-wrap" }}>{message.content}</span>
        ) : (
          <>
            {thinking && (
              <details className="thinking-block">
                <summary className="thinking-summary">思考过程</summary>
                <div className="thinking-content markdown-content">
                  <ReactMarkdown remarkPlugins={[remarkGfm]}>{thinking}</ReactMarkdown>
                </div>
              </details>
            )}
            <div className="markdown-content">
              <ReactMarkdown remarkPlugins={[remarkGfm]}>
                {text || (message.isStreaming && !thinking ? "▌" : "")}
              </ReactMarkdown>
            </div>
          </>
        )}
        {message.isStreaming && (
          <span className="cursor-blink">▌</span>
        )}
      </div>
    </div>
  );
}

// ─── Settings view ─────────────────────────────────────────────────────────
function SettingsView({
  settings,
  onSave,
}: {
  settings: AppSettings;
  onSave: (s: AppSettings) => void;
}) {
  const [form, setForm] = useState<AppSettings>(settings);
  const [saved, setSaved] = useState(false);

  const handleSave = async () => {
    await onSave(form);
    setSaved(true);
    setTimeout(() => setSaved(false), 2000);
  };

  return (
    <div className="panel-view">
      <div className="panel-header">
        <h2>⚙️ 模型设置</h2>
        <p>配置 AI 模型连接参数，支持 OpenAI 兼容接口</p>
      </div>
      <div className="settings-form">
        <FormGroup label="API Base URL" hint="OpenAI 兼容接口地址">
          <input
            className="form-input"
            value={form.apiBaseUrl}
            onChange={(e) => setForm({ ...form, apiBaseUrl: e.target.value })}
            placeholder="https://api.openai.com/v1"
          />
        </FormGroup>

        <FormGroup label="API Key" hint="您的 API 密钥（本地保存，不上传）">
          <input
            className="form-input"
            type="password"
            value={form.apiKey}
            onChange={(e) => setForm({ ...form, apiKey: e.target.value })}
            placeholder="sk-..."
          />
        </FormGroup>

        <FormGroup label="模型" hint="模型名称，如 gpt-4o, claude-3-5-sonnet, deepseek-chat">
          <input
            className="form-input"
            value={form.model}
            onChange={(e) => setForm({ ...form, model: e.target.value })}
            placeholder="gpt-4o"
          />
        </FormGroup>

        <div className="form-row">
          <FormGroup
            label={`温度 (${form.temperature})`}
            hint="越高越有创意，志愿填报建议保持 0.3-0.7"
          >
            <input
              type="range"
              min={0}
              max={1}
              step={0.1}
              value={form.temperature}
              onChange={(e) =>
                setForm({ ...form, temperature: parseFloat(e.target.value) })
              }
            />
          </FormGroup>

          <FormGroup label="最大 Token" hint="单次回复最大长度">
            <input
              className="form-input"
              type="number"
              min={500}
              max={8000}
              step={100}
              value={form.maxTokens}
              onChange={(e) =>
                setForm({ ...form, maxTokens: parseInt(e.target.value) })
              }
            />
          </FormGroup>
        </div>

        <FormGroup label="主题">
          <div className="theme-selector">
            {(["light", "dark"] as const).map((t) => (
              <label key={t} className="theme-option">
                <input
                  type="radio"
                  name="theme"
                  value={t}
                  checked={form.theme === t}
                  onChange={() => setForm({ ...form, theme: t })}
                />
                {t === "light" ? "☀️ 浅色" : "🌙 深色"}
              </label>
            ))}
          </div>
        </FormGroup>

        <div className="form-actions">
          <button className="btn-primary" onClick={handleSave}>
            {saved ? "✅ 已保存" : "保存设置"}
          </button>
        </div>
      </div>
    </div>
  );
}

function FormGroup({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="form-group">
      <label className="form-label">{label}</label>
      {hint && <span className="form-hint">{hint}</span>}
      {children}
    </div>
  );
}

// ─── Skills view ────────────────────────────────────────────────────────────
function SkillsView({
  skills,
  activeSkillId,
  onActivate,
  onRefresh,
}: {
  skills: Skill[];
  activeSkillId: string;
  onActivate: (id: string) => void;
  onRefresh: () => void;
}) {
  const [selectedSkill, setSelectedSkill] = useState<Skill | null>(null);
  const [editMode, setEditMode] = useState(false);
  const [editContent, setEditContent] = useState("");
  const [newSkillName, setNewSkillName] = useState("");
  const [showNew, setShowNew] = useState(false);

  const handleSaveEdit = async () => {
    if (!selectedSkill || selectedSkill.isBuiltIn) return;
    await invoke("save_skill_file", {
      skillId: selectedSkill.id,
      content: editContent,
    });
    onRefresh();
    setEditMode(false);
  };

  const handleNewSkill = async () => {
    const id = newSkillName.trim().replace(/\s+/g, "-").toLowerCase();
    if (!id) return;
    const template = `# ${newSkillName}\n\n## 角色描述\n\n在这里描述AI应该扮演的角色...\n\n## 行为规则\n\n- 规则1\n- 规则2\n`;
    await invoke("save_skill_file", { skillId: id, content: template });
    setShowNew(false);
    setNewSkillName("");
    onRefresh();
  };

  if (editMode && selectedSkill) {
    return (
      <div className="panel-view">
        <div className="panel-header">
          <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
            <button className="btn-ghost" onClick={() => setEditMode(false)}>
              ← 返回
            </button>
            <h2>编辑 Skill：{selectedSkill.name}</h2>
          </div>
        </div>
        <div style={{ flex: 1, display: "flex", flexDirection: "column", gap: 12 }}>
          <textarea
            style={{
              flex: 1,
              fontFamily: "var(--font-mono)",
              fontSize: 13,
              padding: 16,
              border: "1px solid var(--color-border)",
              borderRadius: "var(--radius-sm)",
              background: "var(--color-input-bg)",
              color: "var(--color-text)",
              resize: "none",
            }}
            value={editContent}
            onChange={(e) => setEditContent(e.target.value)}
          />
          <div className="form-actions">
            <button className="btn-ghost" onClick={() => setEditMode(false)}>
              取消
            </button>
            <button className="btn-primary" onClick={handleSaveEdit}>
              保存
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="panel-view">
      <div className="panel-header">
        <div>
          <h2>📚 技能管理</h2>
          <p>选择 AI 的思维框架和角色扮演模式</p>
        </div>
        <button className="btn-primary" onClick={() => setShowNew(true)}>
          + 新建 Skill
        </button>
      </div>

      {showNew && (
        <div className="new-skill-form">
          <input
            className="form-input"
            placeholder="技能名称（如：李华老师）"
            value={newSkillName}
            onChange={(e) => setNewSkillName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && handleNewSkill()}
          />
          <button className="btn-primary" onClick={handleNewSkill}>
            创建
          </button>
          <button className="btn-ghost" onClick={() => setShowNew(false)}>
            取消
          </button>
        </div>
      )}

      <div className="skill-grid">
        {skills.map((skill) => (
          <div
            key={skill.id}
            className={`skill-card ${skill.id === activeSkillId ? "active" : ""}`}
          >
            <div className="skill-card-icon">{skill.icon ?? "📝"}</div>
            <div className="skill-card-body">
              <div className="skill-card-name">
                {skill.name}
                {skill.isBuiltIn && (
                  <span className="badge badge-built-in">内置</span>
                )}
                {skill.id === activeSkillId && (
                  <span className="badge badge-active">当前</span>
                )}
              </div>
              <div className="skill-card-desc">{skill.description}</div>
            </div>
            <div className="skill-card-actions">
              <button
                className="btn-ghost btn-sm"
                onClick={() => {
                  setSelectedSkill(skill);
                  setEditContent(skill.content);
                  setEditMode(!skill.isBuiltIn);
                }}
              >
                {skill.isBuiltIn ? "查看" : "编辑"}
              </button>
              {skill.id !== activeSkillId && (
                <button
                  className="btn-primary btn-sm"
                  onClick={() => onActivate(skill.id)}
                >
                  启用
                </button>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

// ─── MCP view ──────────────────────────────────────────────────────────────
// ─── Built-in MCP presets ───────────────────────────────────────────────────
const MCP_PRESETS: Omit<McpServer, "id">[] = [
  {
    name: "高考排名查询",
    description: "查询高考分数在各省的位次排名，支持多年份、多类别（需安装 uv）",
    command: "uvx",
    args: ["mcp-gaokao-rank"],
    env: {},
    enabled: true,
  },
];

function McpView({
  mcpServers,
  onSave,
}: {
  mcpServers: McpServer[];
  onSave: (servers: McpServer[]) => void;
}) {
  const [servers, setServers] = useState<McpServer[]>(mcpServers);
  const [showForm, setShowForm] = useState(false);
  const [editServer, setEditServer] = useState<McpServer | null>(null);
  const [form, setForm] = useState({
    name: "",
    description: "",
    command: "",
    args: "",
    env: "",
  });

  const handleAdd = () => {
    const newServer: McpServer = {
      id: uuidv4(),
      name: form.name,
      description: form.description,
      command: form.command,
      args: form.args
        .split(" ")
        .map((s) => s.trim())
        .filter(Boolean),
      env: Object.fromEntries(
        form.env
          .split("\n")
          .map((line) => line.split("="))
          .filter((parts) => parts.length === 2)
          .map(([k, v]) => [k.trim(), v.trim()])
      ),
      enabled: true,
    };
    const updated = editServer
      ? servers.map((s) => (s.id === editServer.id ? newServer : s))
      : [...servers, newServer];
    setServers(updated);
    onSave(updated);
    setShowForm(false);
    setEditServer(null);
    setForm({ name: "", description: "", command: "", args: "", env: "" });
  };

  const handleDelete = (id: string) => {
    const updated = servers.filter((s) => s.id !== id);
    setServers(updated);
    onSave(updated);
  };

  const handleToggle = (id: string) => {
    const updated = servers.map((s) =>
      s.id === id ? { ...s, enabled: !s.enabled } : s
    );
    setServers(updated);
    onSave(updated);
  };

  const startEdit = (server: McpServer) => {
    setEditServer(server);
    setForm({
      name: server.name,
      description: server.description,
      command: server.command,
      args: server.args.join(" "),
      env: Object.entries(server.env)
        .map(([k, v]) => `${k}=${v}`)
        .join("\n"),
    });
    setShowForm(true);
  };

  return (
    <div className="panel-view">
      <div className="panel-header">
        <div>
          <h2>🔧 MCP 服务器</h2>
          <p>配置 Model Context Protocol 工具服务器，扩展 AI 能力</p>
        </div>
        <button className="btn-primary" onClick={() => setShowForm(true)}>
          + 添加服务器
        </button>
      </div>

      {showForm && (
        <div className="mcp-form card">
          <h3>{editServer ? "编辑服务器" : "添加 MCP 服务器"}</h3>
          <FormGroup label="名称">
            <input
              className="form-input"
              value={form.name}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
              placeholder="如：文件系统、搜索工具"
            />
          </FormGroup>
          <FormGroup label="描述">
            <input
              className="form-input"
              value={form.description}
              onChange={(e) =>
                setForm({ ...form, description: e.target.value })
              }
              placeholder="简短描述该服务器的功能"
            />
          </FormGroup>
          <FormGroup label="命令" hint="启动 MCP 服务器的命令">
            <input
              className="form-input"
              value={form.command}
              onChange={(e) => setForm({ ...form, command: e.target.value })}
              placeholder="如：npx, python, node"
            />
          </FormGroup>
          <FormGroup label="参数" hint="命令参数，空格分隔">
            <input
              className="form-input"
              value={form.args}
              onChange={(e) => setForm({ ...form, args: e.target.value })}
              placeholder="如：-m mcp_server.app"
            />
          </FormGroup>
          <FormGroup label="环境变量" hint="每行一个，格式：KEY=VALUE">
            <textarea
              className="form-input"
              style={{ resize: "vertical", minHeight: 80 }}
              value={form.env}
              onChange={(e) => setForm({ ...form, env: e.target.value })}
              placeholder="API_KEY=your_key"
            />
          </FormGroup>
          <div className="form-actions">
            <button
              className="btn-ghost"
              onClick={() => {
                setShowForm(false);
                setEditServer(null);
              }}
            >
              取消
            </button>
            <button
              className="btn-primary"
              onClick={handleAdd}
              disabled={!form.name || !form.command}
            >
              {editServer ? "更新" : "添加"}
            </button>
          </div>
        </div>
      )}

      {servers.length === 0 && !showForm && (
        <div className="empty-state">
          <div style={{ fontSize: 48, marginBottom: 16 }}>🔧</div>
          <h3>暂无 MCP 服务器</h3>
          <p>MCP（Model Context Protocol）可以让 AI 调用外部工具，如文件系统、网络搜索等。</p>
          <p style={{ marginTop: 8, color: "var(--color-text-muted)" }}>
            推荐：filesystem MCP, search MCP, puppeteer MCP
          </p>
        </div>
      )}

      {/* ── Built-in presets ── */}
      <div className="mcp-presets-section">
        <div className="mcp-presets-title">推荐预设</div>
        <div className="mcp-server-list">
          {MCP_PRESETS.map((preset) => {
            const alreadyAdded = servers.some(
              (s) => s.command === preset.command && s.args.join(" ") === preset.args.join(" ")
            );
            return (
              <div key={preset.name} className="mcp-server-card card mcp-preset-card">
                <div className="mcp-server-header">
                  <div>
                    <div className="mcp-server-name">
                      {preset.name}
                      <span className="badge badge-built-in" style={{ marginLeft: 6 }}>内置</span>
                    </div>
                    <div className="mcp-server-desc">{preset.description}</div>
                  </div>
                </div>
                <div className="mcp-server-cmd">
                  <code>{preset.command} {preset.args.join(" ")}</code>
                </div>
                <div className="mcp-server-actions">
                  {alreadyAdded ? (
                    <span style={{ fontSize: 12, color: "var(--color-text-muted)" }}>✓ 已添加</span>
                  ) : (
                    <button
                      className="btn-primary btn-sm"
                      onClick={() => {
                        const newServer: McpServer = { ...preset, id: uuidv4() };
                        const updated = [...servers, newServer];
                        setServers(updated);
                        onSave(updated);
                      }}
                    >
                      + 添加
                    </button>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      </div>

      <div className="mcp-server-list">
        {servers.map((server) => (
          <div key={server.id} className="mcp-server-card card">
            <div className="mcp-server-header">
              <div>
                <div className="mcp-server-name">{server.name}</div>
                <div className="mcp-server-desc">{server.description}</div>
              </div>
              <label className="toggle">
                <input
                  type="checkbox"
                  checked={server.enabled}
                  onChange={() => handleToggle(server.id)}
                />
                <span className="toggle-slider" />
              </label>
            </div>
            <div className="mcp-server-cmd">
              <code>
                {server.command} {server.args.join(" ")}
              </code>
            </div>
            <div className="mcp-server-actions">
              <button className="btn-ghost btn-sm" onClick={() => startEdit(server)}>
                编辑
              </button>
              <button
                className="btn-danger btn-sm"
                onClick={() => handleDelete(server.id)}
              >
                删除
              </button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

// ─── Data view ─────────────────────────────────────────────────────────────
function DataView({ schools }: { schools: GaokaoSchool[] }) {
  const [search, setSearch] = useState("");
  const [filterType, setFilterType] = useState("全部");
  const [filter985, setFilter985] = useState(false);
  const [filter211, setFilter211] = useState(false);
  const [selectedSchool, setSelectedSchool] = useState<GaokaoSchool | null>(
    null
  );
  const [scoreFilter, setScoreFilter] = useState<number | null>(null);
  // PDF Import states
  const [importing, setImporting] = useState(false);
  const [importProgress, setImportProgress] = useState("");
  const [importError, setImportError] = useState<string | null>(null);
  const [importPreview, setImportPreview] = useState<PdfCutoffEntry[] | null>(null);
  const [importTitle, setImportTitle] = useState("");
  const [importYear, setImportYear] = useState(2025);
  const [importProvince, setImportProvince] = useState("上海");

  const types = ["全部", ...Array.from(new Set(schools.map((s) => s.type)))];

  const filtered = schools.filter((s) => {
    const matchSearch =
      !search ||
      s.name.includes(search) ||
      (s.shortName?.includes(search) ?? false) ||
      s.majors.some((m) => m.name.includes(search));
    const matchType = filterType === "全部" || s.type === filterType;
    const match985 = !filter985 || s.is985;
    const match211 = !filter211 || s.is211;
    const matchScore =
      !scoreFilter ||
      s.majors.some((m) =>
        m.scores.some((sc) => sc.minScore <= scoreFilter + 20 && sc.minScore >= scoreFilter - 30)
      );
    return matchSearch && matchType && match985 && match211 && matchScore;
  });

  // PDF Import handler
  const handleImportPdf = async () => {
    console.log("Import PDF button clicked");
    try {
      setImporting(true);
      setImportError(null);
      setImportProgress("正在选择文件...");

      // Open file picker via Tauri
      console.log("Calling open_file_dialog...");
      const selected = await invoke<{ filePath: string } | null>("open_file_dialog", {
        filters: [{ name: "PDF", extensions: ["pdf"] }],
        multiple: false,
      });
      console.log("open_file_dialog result:", selected);

      if (!selected) {
        console.log("selected is falsy, returning early");
        setImporting(false);
        return;
      }
      if (!selected.filePath) {
        console.log("selected.filePath is falsy, returning early. filePath:", selected.filePath);
        setImporting(false);
        return;
      }

      setImportProgress("正在解析 PDF，请稍候...");

      // Call Rust backend to parse PDF
      console.log("Calling import_pdf_cutoffs with path:", selected.filePath);
      const result = await invoke<PdfCutoffData>("import_pdf_cutoffs", {
        pdfPath: selected.filePath,
        year: importYear,
        province: importProvince,
      });
      console.log("import_pdf_cutoffs result:", result);

      setImportTitle(result.title);
      setImportPreview(result.data);
      setImportProgress("");
    } catch (err) {
      console.error("PDF import error:", err);
      setImportError(String(err));
      setImporting(false);
    }
  };

  const handleConfirmImport = async () => {
    if (!importPreview) return;

    try {
      setImporting(true);
      setImportProgress("正在保存数据...");

      // Save to gaokao data file
      await invoke("save_gaokao_data", {
        dataType: "shanghai_2025_cutoffs",
        data: {
          title: importTitle,
          year: importYear,
          province: importProvince,
          note: importPreview.length > 0 ? "由 PDF 导入生成" : "",
          data: importPreview,
        },
      });

      // Close preview and reload page to reflect changes
      setImportPreview(null);
      setImportTitle("");
      window.location.reload(); // Simple refresh to reload data
    } catch (err) {
      setImportError(String(err));
      setImporting(false);
    }
  };

  const handleCancelImport = () => {
    setImportPreview(null);
    setImportError(null);
  };

  // Import preview modal
  if (importPreview !== null) {
    return (
      <div className="import-modal-overlay" onClick={handleCancelImport}>
        <div className="import-modal-content" onClick={(e) => e.stopPropagation()}>
          <div className="panel-header">
            <div>
              <h2>📄 PDF 导入预览</h2>
              <p style={{ fontSize: 13, color: "var(--text-secondary)", marginTop: 4 }}>
                {importTitle}
              </p>
            </div>
          </div>

          <div style={{ padding: "16px 20px", borderBottom: "1px solid var(--border-color)" }}>
            <div style={{ display: "flex", gap: 12, alignItems: "center", flexWrap: "wrap" }}>
              <label style={{ fontSize: 13 }}>
                年份：
                <input
                  type="number"
                  className="form-input"
                  style={{ width: 80 }}
                  value={importYear}
                  onChange={(e) => setImportYear(parseInt(e.target.value) || 2025)}
                />
              </label>
              <label style={{ fontSize: 13 }}>
                省份：
                <input
                  className="form-input"
                  style={{ width: 120 }}
                  value={importProvince}
                  onChange={(e) => setImportProvince(e.target.value)}
                />
              </label>
            </div>
          </div>

          {importError && (
            <div style={{ padding: "12px 20px", background: "var(--bg-error)", color: "var(--color-error)", fontSize: 13 }}>
              {importError}
            </div>
          )}

          {importProgress && (
            <div style={{ padding: "12px 20px", background: "var(--bg-info)", color: "var(--color-info)", fontSize: 13 }}>
              {importProgress}
            </div>
          )}

          <div style={{ padding: "16px 20px", maxHeight: 400, overflowY: "auto" }}>
            <div style={{ marginBottom: 12, fontSize: 13, color: "var(--text-secondary)" }}>
              解析到 {importPreview.length} 条记录。请确认数据格式正确：
            </div>
            <table className="data-table" style={{ fontSize: 12 }}>
              <thead>
                <tr>
                  <th>#</th>
                  <th>院校代码</th>
                  <th>院校专业组</th>
                  <th>投档线</th>
                </tr>
              </thead>
              <tbody>
                {importPreview.slice(0, 50).map((entry, idx) => (
                  <tr key={idx}>
                    <td>{idx + 1}</td>
                    <td><code>{entry.code}</code></td>
                    <td>{entry.name}</td>
                    <td>
                      {typeof entry.cutoff === "number"
                        ? <strong>{entry.cutoff}分</strong>
                        : <span style={{ color: "var(--color-warning)" }}>{entry.cutoff}</span>
                      }
                    </td>
                  </tr>
                ))}
                {importPreview.length > 50 && (
                  <tr>
                    <td colSpan={4} style={{ textAlign: "center", color: "var(--text-secondary)" }}>
                      ... 还有 {importPreview.length - 50} 条记录
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>

          <div style={{
            padding: "12px 20px",
            background: "var(--bg-secondary)",
            display: "flex",
            justifyContent: "flex-end",
            gap: 8
          }}>
            <button className="btn btn-ghost" onClick={handleCancelImport}>
              取消
            </button>
            <button
              className="btn btn-primary"
              onClick={handleConfirmImport}
              disabled={importing || importPreview.length === 0}
            >
              {importing ? "⏳ 保存中..." : "✅ 确认导入"}
            </button>
          </div>
        </div>
      </div>
    );
  }

  if (selectedSchool) {
    return (
      <div className="panel-view">
        <div className="panel-header">
          <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
            <button
              className="btn-ghost"
              onClick={() => setSelectedSchool(null)}
            >
              ← 返回
            </button>
            <div>
              <h2>{selectedSchool.name}</h2>
              <div className="school-badges">
                {selectedSchool.is985 && (
                  <span className="badge badge-985">985</span>
                )}
                {selectedSchool.is211 && (
                  <span className="badge badge-211">211</span>
                )}
                {selectedSchool.isDoubleFirstClass && (
                  <span className="badge badge-dfc">双一流</span>
                )}
                <span className="badge">{selectedSchool.type}</span>
              </div>
            </div>
          </div>
        </div>
        <div className="school-detail">
          <table className="data-table">
            <thead>
              <tr>
                <th>专业名称</th>
                <th>招生人数</th>
                <th>学费/年</th>
                <th>学制</th>
                <th>选考要求</th>
                <th>2024最低分</th>
                <th>2024位次</th>
                <th>2023最低分</th>
              </tr>
            </thead>
            <tbody>
              {selectedSchool.majors.map((major) => {
                const score2024 = major.scores.find((s) => s.year === 2024);
                const score2023 = major.scores.find((s) => s.year === 2023);
                return (
                  <tr key={major.code}>
                    <td>
                      <strong>{major.name}</strong>
                      <div style={{ fontSize: 11, color: "var(--color-text-muted)" }}>
                        {major.code}
                      </div>
                    </td>
                    <td>{major.enrollmentCount}</td>
                    <td>¥{major.tuition.toLocaleString()}</td>
                    <td>{major.duration}年</td>
                    <td>
                      {major.requirements.length > 0
                        ? major.requirements.join("、")
                        : "不限"}
                    </td>
                    <td className="score-cell">
                      {score2024 ? (
                        <strong>{score2024.minScore}</strong>
                      ) : (
                        "-"
                      )}
                    </td>
                    <td>{score2024 ? score2024.minRank : "-"}</td>
                    <td>{score2023 ? score2023.minScore : "-"}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </div>
    );
  }

  return (
    <div className="panel-view">
      <div className="panel-header">
        <div>
          <h2>📊 上海高考招生数据</h2>
          <p>2024-2025 年上海招生院校录取数据（参考用，2026年数据待更新）</p>
        </div>
      </div>

      <div className="data-filters">
        <input
          className="form-input"
          style={{ flex: 1, maxWidth: 300 }}
          placeholder="搜索院校或专业名称..."
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <select
          className="form-input"
          style={{ width: 120 }}
          value={filterType}
          onChange={(e) => setFilterType(e.target.value)}
        >
          {types.map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </select>
        <label className="filter-checkbox">
          <input
            type="checkbox"
            checked={filter985}
            onChange={(e) => setFilter985(e.target.checked)}
          />
          985
        </label>
        <label className="filter-checkbox">
          <input
            type="checkbox"
            checked={filter211}
            onChange={(e) => setFilter211(e.target.checked)}
          />
          211
        </label>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <input
            className="form-input"
            type="number"
            style={{ width: 90 }}
            placeholder="参考分数"
            value={scoreFilter ?? ""}
            onChange={(e) =>
              setScoreFilter(e.target.value ? parseInt(e.target.value) : null)
            }
          />
          {scoreFilter && (
            <button
              className="btn-ghost btn-sm"
              onClick={() => setScoreFilter(null)}
            >
              清除
            </button>
          )}
          <button
            className="btn btn-primary btn-sm"
            onClick={handleImportPdf}
            disabled={importing}
            title="从 PDF 导入分数线数据"
          >
            {importing ? "⏳ 导入中..." : "📄 导入 PDF"}
          </button>
        </div>
      </div>

      <div className="data-summary">
        共 {filtered.length} 所院校 ·{" "}
        {filtered.reduce((sum, s) => sum + s.majors.length, 0)} 个专业
      </div>

      <div className="schools-grid">
        {filtered.map((school) => {
          const minScore = Math.min(
            ...school.majors.flatMap((m) =>
              m.scores
                .filter((s) => s.year === 2024)
                .map((s) => s.minScore)
            ).filter(Boolean)
          );
          const maxScore = Math.max(
            ...school.majors.flatMap((m) =>
              m.scores
                .filter((s) => s.year === 2024)
                .map((s) => s.minScore)
            ).filter(Boolean)
          );

          return (
            <div
              key={school.id}
              className="school-card card"
              onClick={() => setSelectedSchool(school)}
            >
              <div className="school-card-header">
                <div className="school-card-name">{school.name}</div>
                <div className="school-badges">
                  {school.is985 && (
                    <span className="badge badge-985">985</span>
                  )}
                  {school.is211 && (
                    <span className="badge badge-211">211</span>
                  )}
                  {school.isDoubleFirstClass && (
                    <span className="badge badge-dfc">双一流</span>
                  )}
                </div>
              </div>
              <div className="school-card-meta">
                <span>🏛 {school.type}</span>
                <span>📚 {school.majors.length} 个专业</span>
              </div>
              {isFinite(minScore) && (
                <div className="school-card-score">
                  2024录取分：
                  <strong>
                    {minScore === maxScore
                      ? minScore
                      : `${minScore} - ${maxScore}`}
                  </strong>
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

// ─── About View ───────────────────────────────────────────────────────────
function AboutView() {
  return (
    <div className="settings-view" style={{ maxWidth: 640 }}>
      <h2 className="settings-title">关于</h2>

      <div style={{ display: "flex", alignItems: "center", gap: 16, marginBottom: 24 }}>
        <span style={{ fontSize: 48 }}>🎓</span>
        <div>
          <div style={{ fontSize: 20, fontWeight: 700 }}>高考报志愿助手</div>
          <div style={{ color: "var(--text-secondary)", marginTop: 4 }}>版本 v0.0.1</div>
        </div>
      </div>

      <section style={{ marginBottom: 24 }}>
        <h3 style={{ marginBottom: 8, fontSize: 15 }}>免责声明</h3>
        <div
          style={{
            background: "var(--bg-secondary)",
            borderRadius: 8,
            padding: "16px 20px",
            fontSize: 13,
            lineHeight: 1.8,
            color: "var(--text-secondary)",
            border: "1px solid var(--border-color)",
          }}
        >
          <p style={{ margin: "0 0 10px" }}>
            本软件仅供参考，所提供的高考数据、院校信息及 AI
            生成的志愿填报建议均不构成任何正式的录取承诺或报考指导意见。
          </p>
          <p style={{ margin: "0 0 10px" }}>
            数据来源于公开渠道，可能存在滞后、偏差或不完整之处。最终录取结果受多重因素影响，
            实际情况请以各省市招生主管部门及高校官方公布的信息为准。
          </p>
          <p style={{ margin: 0 }}>
            使用本软件产生的任何后果，开发者不承担法律责任。建议结合专业招生老师的意见审慎填报志愿。
          </p>
        </div>
      </section>

      <section>
        <h3 style={{ marginBottom: 8, fontSize: 15 }}>技术栈</h3>
        <div style={{ color: "var(--text-secondary)", fontSize: 13, lineHeight: 1.8 }}>
          Tauri 2 · React 18 · TypeScript · MCP Protocol
        </div>
      </section>
    </div>
  );
}
