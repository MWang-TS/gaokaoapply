export type View = "chat" | "settings" | "skills" | "mcp" | "data" | "about";
export type Role = "user" | "assistant" | "system";
export type Theme = "light" | "dark";

export interface ChatMessage {
  id: string;
  role: Role;
  content: string;
  timestamp: number;
  isStreaming?: boolean;
  toolName?: string;
}

export interface Conversation {
  id: string;
  title: string;
  messages: ChatMessage[];
  createdAt: number;
  updatedAt: number;
  skillId: string;
}

export interface AppSettings {
  apiBaseUrl: string;
  apiKey: string;
  model: string;
  temperature: number;
  maxTokens: number;
  activeSkillId: string;
  theme: Theme;
}

export interface Skill {
  id: string;
  name: string;
  description: string;
  content: string;
  isBuiltIn: boolean;
  icon?: string;
}

export interface McpServer {
  id: string;
  name: string;
  description: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  enabled: boolean;
}

export interface GaokaoSchool {
  id: string;
  name: string;
  shortName?: string;
  province: string;
  city: string;
  type: string;
  is985: boolean;
  is211: boolean;
  isDoubleFirstClass: boolean;
  website?: string;
  majors: GaokaoMajor[];
}

export interface GaokaoMajor {
  code: string;
  name: string;
  enrollmentCount: number;
  tuition: number;
  duration: number;
  requirements: string[];
  scores: {
    year: number;
    minScore: number;
    minRank: number;
  }[];
}

// PDF Import types
export interface PdfCutoffEntry {
  code: string;
  name: string;
  cutoff: number | string;
}

export interface PdfCutoffData {
  title: string;
  year: number;
  province: string;
  note?: string;
  data: PdfCutoffEntry[];
}
