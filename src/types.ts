export type AgentId = 'codex' | 'claude';
export type LogicalRole = 'analyst' | 'implementer';
export interface RoleStamp { logicalRole?: LogicalRole | null; provider?: AgentId | null; roleRevision?: number | null }
export type RunState = 'ready' | 'analyzing' | 'implementing' | 'verifying' | 'updating_skill' | 'paused' | 'interrupted' | 'completed' | 'failed';
export type EventKind = 'prompt' | 'message' | 'tool_summary' | 'skill_update' | 'state_transition' | 'system' | 'stdout' | 'stderr';
export type EdgeType = 'sequence' | 'handoff' | 'verification' | 'skill_revision' | 'recovery';
export interface RoleMapping { analyst: AgentId; implementer: AgentId; revision: number }
export interface AgentEvent extends RoleStamp {
  id: string; runId: string; sequence: number; createdAt: string;
  agent: AgentId | null; kind: EventKind; content: string; parentId: string | null;
}
export interface CausalEdge extends RoleStamp { id: string; runId: string; source: string; target: string; edgeType: EdgeType }
export interface Checkpoint {
  logicalRole?: LogicalRole | null; provider?: AgentId | null;
  runId: string; state: RunState; activeAgent: AgentId | null; nextStep: string;
  lastEventId: string | null; updatedAt: string; roleRevision: number;
  interruptedState: RunState | null;
}
export type TurnStatus = 'submitted' | 'running' | 'completed' | 'failed' | 'cancelled' | 'interrupted';
export interface InteractionQuestion { id: string; text: string; options: string[]; multiple: boolean }
export type InteractionKind = 'permission' | 'question' | 'mcp_confirmation' | 'mcp_form' | 'mcp_url' | 'mcp_unsupported';
export type ElicitationPolicy = 'ask' | 'decline' | 'cancel';
export interface Interaction { id: string; protocolId?: string | null; title: string; detail: string; questions: InteractionQuestion[]; approval: boolean; canRemember?: boolean; kind?: InteractionKind; mcpMode?: string | null; mcpSchema?: Record<string, unknown> | null; mcpUrl?: string | null; policy?: ElicitationPolicy; decision?: string | null; delivery?: string | null; status: 'pending' | 'answered' | 'expired' | 'blocked' }
export type McpElicitationAction = 'accept' | 'decline' | 'cancel';
export type InteractionAnswer = { decision?: 'allow' | 'deny'; remember?: boolean; answers?: Record<string, string[]> } | { action: McpElicitationAction; content?: Record<string, unknown> };
export type RespondInteraction = (promptId: string, requestId: string, answer: InteractionAnswer) => Promise<boolean>;
export interface Turn extends RoleStamp { promptId: string; agent: AgentId; status: TurnStatus; detail: string; interactions?: Interaction[]; roleSkill?: { path: string; content: string } | null }
export interface Execution { elicitationPolicy?: ElicitationPolicy; historyRevision?: number; activeSkillId?: string | null; pendingQueries?: string[]; chainActive: boolean; codexSession: string | null; claudeSession: string | null; turns: Turn[] }
export interface Run { id: string; title: string; workspace: string; createdAt: string; updatedAt: string; state: RunState; roles: RoleMapping; execution?: Execution }
export interface Artifact { id: string; runId: string; revision: number; relativePath: string; content: string; createdAt: string }
export interface RunSnapshot { run: Run; checkpoint: Checkpoint; events: AgentEvent[]; edges: CausalEdge[]; artifacts: Artifact[] }
export type CheckStatus = 'ok' | 'warning' | 'error' | 'not_checked';
export interface EnvCheck { id: string; label: string; status: CheckStatus; detail: string; path: string | null }
export type ToolOutcome = 'verified' | 'needs_context' | 'approval_required' | 'unavailable' | 'failed' | 'timeout' | 'interrupted' | 'unsupported_rule';
export interface McpToolCheck { name: string; status: CheckStatus; detail: string; outcome?: ToolOutcome; method?: 'empty' | 'context' | 'none' }
export interface McpServerReport { id: string; name: string; owner: AgentId; transport: string; source: string; status: CheckStatus; connectionStatus?: CheckStatus; detail: string; tools: string[]; toolChecks?: McpToolCheck[] }
export interface EnvCheckReport { checkedAt: string; platform: string; checks: EnvCheck[]; mcpServers: McpServerReport[] }
export interface WorkspaceCatalog { snapshot: RunSnapshot | null; workspaces: Run[] }
export interface Bootstrap extends WorkspaceCatalog { dataDirectory: string }