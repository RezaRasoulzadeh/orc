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

export const api = {
  snapshot: () => invoke<DesktopSnapshot>('snapshot'),
  tasks: () => invoke<Task[]>('tasks'),
  runs: (limit: number) => invoke<AgentRun[]>('runs', { limit }),
}
