import { invoke } from '@tauri-apps/api/core'

export type TaskStatus = 'backlog' | 'ready' | 'active' | 'review' | 'blocked' | 'done' | 'cancelled'

export interface Task {
  id: string
  title: string
  objective: string
  role: string
  priority: 'low' | 'normal' | 'high' | 'critical'
  status: TaskStatus
  cancellation_reason: string | null
  required_capabilities: string[]
  scope_mode: 'focused' | 'module' | 'project' | null
  context_files: string[]
  expected_changes: string[]
}

export interface QueueEntry { task: Task }
export interface QueueReport {
  ready: QueueEntry[]
  blocked: QueueEntry[]
  active: QueueEntry[]
  review: QueueEntry[]
  done: QueueEntry[]
  cancelled: QueueEntry[]
  backlog: QueueEntry[]
}
export interface Dashboard { queue: QueueReport; tasks: Task[] }
export interface ProjectHealth {
  task_counts: Record<string, number>
  active_runs: number
  unresolved_approvals: number
}
export interface DesktopSnapshot { dashboard: Dashboard; health: ProjectHealth }
export interface LeadTurn { id: number; role: 'user' | 'assistant' | 'system'; content: string; created_at: string }
export interface PlannedTask { local_id: string; title: string; objective: string; role: string; priority: 'low' | 'normal' | 'high' | 'critical'; depends_on: string[]; capabilities: string[]; scope_mode: 'focused' | 'module' | 'project' | null; context_files: string[]; expected_changes: string[] }
export interface PlanResponse { protocol_version: number; objective: string; assumptions: string[]; risks: string[]; questions: string[]; tasks: PlannedTask[] }
export type LeadProposalKind =
  | { kind: 'plan'; details: PlanResponse }
  | { kind: 'task'; details: PlannedTask }
  | { kind: 'revision'; details: { task_id: string; feedback: string } }
  | { kind: 'approval_request'; details: { reason: string; details: string } }
export interface LeadProposal { id: number; proposal: LeadProposalKind; status: 'pending' | 'applying' | 'applied' | 'rejected'; created_at: string; applying_at: string | null; resolved_at: string | null }
export interface LeadContext { project_id: number; project_name: string; repository_path: string; state: unknown; tasks: Task[]; runs: AgentRun[]; approvals: unknown[]; turns: LeadTurn[]; proposals: LeadProposal[]; queue: QueueReport }
export interface LeadProviderConfig { agent_id: string; model?: string | null; reasoning_effort?: 'None' | 'Low' | 'Medium' | 'High' | null }
export interface LeadResponse { turn: LeadTurn; proposals: LeadProposal[] }
export interface AgentRun {
  id: number
  project_id: number
  task_id: string | null
  agent: string
  execution_mode: string
  status: string
  output: string | null
  started_at: string
  finished_at: string | null
  phase: string | null
  last_activity: string
}
export interface WorkerResult { run_id: number; outcome: string; failure_category: string | null; duration_ms: number | null; metadata: string | null; total_tokens: number | null; input_tokens: number | null; output_tokens: number | null }
export interface LifecycleEvent { id: number; timestamp: string; kind: string; task_id: string | null; run_id: number | null; agent_id: string | null; payload: string | null }
export interface RunDetails { run: AgentRun; result: WorkerResult | null; activity: LifecycleEvent[] }
export interface RunsWorkspace { runs: AgentRun[]; details: RunDetails[] }

export const api = {
  snapshot: () => invoke<DesktopSnapshot>('snapshot'),
  tasks: () => invoke<Task[]>('tasks'),
  runs: (limit: number) => invoke<AgentRun[]>('runs', { limit }),
  runsWorkspace: (limit = 50, activityLimit = 100) => invoke<RunsWorkspace>('runs_workspace', { limit, activityLimit }),
  runDetails: (runId: number, activityLimit = 100) => invoke<RunDetails | null>('run_details', { runId, activityLimit }),
  leadContext: (limit = 20) => invoke<LeadContext>('lead_context', { limit }),
  leadProposals: () => invoke<LeadProposal[]>('lead_proposals'),
  invokeLead: (message: string, config?: LeadProviderConfig) => invoke<LeadResponse>('invoke_lead', { message, config }),
  applyLeadProposal: (proposalId: number) => invoke<void>('apply_lead_proposal', { proposalId }),
  rejectLeadProposal: (proposalId: number) => invoke<void>('reject_lead_proposal', { proposalId }),
}
