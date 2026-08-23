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

export interface DependencyInfo { task_id: string; status: TaskStatus | null; is_done: boolean }
export interface BlockingReason { kind: string; incomplete_dependencies?: DependencyInfo[]; explanation?: string }
export interface QueueEntry { task: Task; category: string; dependencies: DependencyInfo[]; waiting_on: string[]; blocking_reasons: BlockingReason[]; active_agent: string | null; recommended_agent: string | null }
export interface QueueReport {
  ready: QueueEntry[]
  blocked: QueueEntry[]
  active: QueueEntry[]
  review: QueueEntry[]
  done: QueueEntry[]
  cancelled: QueueEntry[]
  backlog: QueueEntry[]
}
export interface AgentDefinition { id: string; backend: string; execution_mode: string; display_name: string; enabled: boolean; priority: number; capabilities: string[]; status: string; unavailable_reason: string | null; profile_path: string | null; model: string | null; reasoning_effort: string | null; config_metadata: string | null; quota_remaining_percent: number | null; quota_reset_at: string | null; quota_checked_at: string | null; quota_source: string | null; quota_limits: unknown | null; actions: AgentActionProfile['action'][] }
export interface CreateTaskInput { title: string; objective: string; role: string; priority: Task['priority']; required_capabilities: string[]; scope_mode: Task['scope_mode']; context_files: string[]; expected_changes: string[]; dependencies: string[] }
export interface AgentActionProfile { action: 'code' | 'review' | 'plan' | 'lead'; model: string | null; reasoning_effort: string | null }
export interface Dashboard { project_name: string; repository_path: string; queue: QueueReport; tasks: Task[]; agents: AgentDefinition[]; approvals: ApprovalRequest[]; recent_activity: LifecycleEvent[]; running_agents: AgentRun[]; capacity: AgentCapacity; outcome_trends: Record<string, number>; repository_available: boolean }
export interface AgentCapacity { agents: AgentDefinition[]; busy: string[]; quota_reserve_percent: number }
export interface ApprovalRequest { id: number; reason: string; resolved: boolean }
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
export interface ExecutionTemplate { model: string | null; reasoning_effort: 'None' | 'Low' | 'Medium' | 'High' | null }
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
export interface ManualRunContext { run: AgentRun; task: Task; task_packet: string }
export interface ManualWorkspaceInfo { supported: boolean; url: string | null; error: string | null }
export interface WorkerResult { run_id: number; outcome: string; failure_category: string | null; duration_ms: number | null; metadata: string | null; total_tokens: number | null; input_tokens: number | null; output_tokens: number | null }
export interface LifecycleEvent { id: number; timestamp: string; kind: string; task_id: string | null; run_id: number | null; agent_id: string | null; payload: string | null }
export interface RunDetails { run: AgentRun; result: WorkerResult | null; activity: LifecycleEvent[] }
export interface RunsWorkspace { runs: AgentRun[]; details: RunDetails[] }
export interface TaskDetails { task: Task; queue: QueueEntry | null; runs: AgentRun[]; activity: LifecycleEvent[] }
export interface ReviewSummary { task: Task; run: AgentRun | null; result: WorkerResult | null; worktree_path: string | null; changes: { files: { status: string; path: string }[]; stat: string; diff: string } }
export interface PlanningRequest { protocol_version: number; kind: string; project: unknown; engineering_contract: string; objective: string; constraints: string[]; target_platforms: string[]; stack: string[]; non_goals: string[]; deliverables: string[]; definition_of_done: string[]; response_schema: unknown; role_boundaries: string[]; planning_constraints: string[]; approval_requirements: string[]; current_state: unknown; full_report: ProjectReport | null }
export interface ProjectReport { protocol_version: number; project: { name: string; repository: string; branch: string | null; commit: string | null }; engineering_contract: string; architecture: { modules: string[]; boundaries: string[]; discovery: Record<string, string> }; lifecycle: { counts: Record<string, number>; tasks: { id: string; title: string; status: string }[] }; agents: unknown[]; queue: QueueReport; recent_work: unknown[]; risks: string[]; open_questions: string[]; role_boundaries: string[]; planning_constraints: string[]; approval_requirements: string[] }
export type ProjectStatus = 'Available' | 'Missing' | 'Invalid' | 'TemporarilyUnavailable'
export interface RegisteredProject { id: string; display_name: string; repository_root: string; project_id: number; project_name: string; last_opened_at: number | null; available: boolean; status: ProjectStatus }

export const api = {
  snapshot: () => invoke<DesktopSnapshot>('snapshot'),
  tasks: () => invoke<Task[]>('tasks'),
  agents: () => invoke<AgentDefinition[]>('agents'),
  createTask: (input: CreateTaskInput) => invoke<string>('create_task', { input }),
  configureAgentRecord: (agent: unknown) => invoke<void>('configure_agent_record', { agent }),
  archiveAgent: (id: string) => invoke<void>('archive_agent', { id }),
  agentActions: (id: string) => invoke<AgentActionProfile[]>('agent_actions', { id }),
  configureAgentAction: (id: string, action: AgentActionProfile['action'], enabled: boolean) => invoke<void>('configure_agent_action', { id, action, enabled }),
  configureAgent: (id: string, field: string, value: string) => invoke<void>('configure_agent', { id, field, value }),
  syncAgent: (id: string) => invoke<void>('sync_agent', { id }),
  manualRuns: (agentId: string) => invoke<ManualRunContext[]>('manual_runs', { agentId }),
  manualRunAction: (action: 'submit' | 'patch' | 'fail', runId: number, value: string) => invoke<void>('manual_run_action', { action, runId, value }),
  manualWorkspaceInfo: (agentId: string) => invoke<ManualWorkspaceInfo>('manual_workspace_info', { agentId }),
  openManualWorkspace: (agentId: string) => invoke<void>('open_manual_workspace', { agentId }),
  closeManualWorkspace: (agentId: string) => invoke<void>('close_manual_workspace', { agentId }),
  queue: () => invoke<QueueReport>('queue'),
  taskDetails: (taskId: string, activityLimit = 100) => invoke<TaskDetails | null>('task_details', { taskId, activityLimit }),
  review: (taskId: string) => invoke<ReviewSummary>('review', { taskId }),
  dispatch: (taskId: string, agentId?: string) => invoke('dispatch', { taskId, agentId }),
  taskAction: (action: string, taskId: string, reason?: string, agentId?: string) => invoke<void>('task_action', { action, taskId, reason, agentId }),
  runs: (limit: number) => invoke<AgentRun[]>('runs', { limit }),
  runsWorkspace: (limit = 50, activityLimit = 100) => invoke<RunsWorkspace>('runs_workspace', { limit, activityLimit }),
  runDetails: (runId: number, activityLimit = 100) => invoke<RunDetails | null>('run_details', { runId, activityLimit }),
  leadContext: (limit = 20) => invoke<LeadContext>('lead_context', { limit }),
  leadProposals: () => invoke<LeadProposal[]>('lead_proposals'),
  invokeLead: (message: string, config?: LeadProviderConfig) => invoke<LeadResponse>('invoke_lead', { message, config }),
  applyLeadProposal: (proposalId: number) => invoke<void>('apply_lead_proposal', { proposalId }),
  rejectLeadProposal: (proposalId: number) => invoke<void>('reject_lead_proposal', { proposalId }),
  leadProviderConfig: () => invoke<LeadProviderConfig | null>('lead_provider_config'),
  setLeadProvider: (config: LeadProviderConfig) => invoke<void>('set_lead_provider', { config }),
  clearLeadProvider: () => invoke<void>('clear_lead_provider'),
  executionTemplate: (className: string) => invoke<ExecutionTemplate>('execution_template', { class: className }),
  setExecutionTemplate: (className: string, model: string | null, effort: string | null) => invoke<void>('set_execution_template', { class: className, model, effort }),
  automatedPlan: (objective: string) => invoke<void>('automated_plan', { objective }),
  planningRequest: () => invoke<PlanningRequest>('planning_request'),
  plannerValidate: (json: string) => invoke<PlanResponse>('planner_validate', { json }),
  plannerApply: (json: string) => invoke<Record<string, string>>('planner_apply', { json }),
  approvals: () => invoke<ApprovalRequest[]>('approvals'),
  resolveApproval: (id: number) => invoke<void>('resolve_approval', { id }),
  projectReport: () => invoke<ProjectReport>('project_report'),
  registeredProjects: () => invoke<RegisteredProject[]>('registered_projects'),
  currentProject: () => invoke<RegisteredProject | null>('current_project'),
  importProject: (root: string, displayName?: string) => invoke<RegisteredProject>('import_project', { root, displayName }),
  adoptProject: (root: string, displayName?: string) => invoke<RegisteredProject>('adopt_project', { root, displayName }),
  openProject: (id: string) => invoke<void>('open_project', { id }),
  closeProject: () => invoke<void>('close_project'),
  relocateProject: (id: string, root: string) => invoke<RegisteredProject>('relocate_project', { id, root }),
  removeProject: (id: string) => invoke<boolean>('remove_project', { id }),
}
