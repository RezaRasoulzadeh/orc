<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { api, type AgentDefinition, type DesktopSnapshot, type LeadContext, type LeadProposal, type ManualRunContext, type ManualWorkspaceInfo, type QueueEntry, type QueueReport, type TaskDetails, type ReviewSummary, type PlanningRequest, type ProjectReport, type ApprovalRequest, type PlanResponse, type RegisteredProject } from './lib/api'
import { listen } from '@tauri-apps/api/event'
import UiBadge from './components/UiBadge.vue'
import UiButton from './components/UiButton.vue'
import UiModal from './components/UiModal.vue'
import UiDisclosure from './components/UiDisclosure.vue'
import ProjectPicker from './components/ProjectPicker.vue'

const sections = ['Dashboard', 'Tasks', 'Runs', 'Agents', 'Lead', 'Planner', 'Approvals', 'Reports', 'Project', 'Settings']
const active = ref('Dashboard')
const snapshot = ref<DesktopSnapshot | null>(null)
const error = ref('')
const projects = ref<RegisteredProject[]>([])
const activeProject = ref<RegisteredProject | null>(null)
const closeConfirmation = ref(false)
const tasks = computed(() => snapshot.value?.dashboard?.tasks ?? [])
const lead = ref<LeadContext | null>(null)
const proposals = ref<LeadProposal[]>([])
const leadPanel = ref(false)
const leadError = ref('')
const leadMessage = ref('')
const panelMessage = ref('')
const leadLoading = ref(false)
const planning = ref<PlanningRequest | null>(null)
const planningObjective = ref('')
const leadConfig = ref<import('./lib/api').LeadProviderConfig | null>(null)
const templateClass = ref('coder')
const templateModel = ref('')
const templateEffort = ref('')
const planJson = ref('')
const plan = ref<PlanResponse | null>(null)
const controlError = ref('')
const approvalList = ref<ApprovalRequest[]>([])
const report = ref<ProjectReport | null>(null)
const runsWorkspace = ref<Awaited<ReturnType<typeof api.runsWorkspace>> | null>(null)
const selectedRun = ref<number | null>(null)
const runError = ref('')
const selectedDetail = computed(() => runsWorkspace.value?.details.find(detail => detail.run.id === selectedRun.value) ?? null)
const taskFilter = ref('all')
const selectedTask = ref<string | null>(null)
const taskDetails = ref<TaskDetails | null>(null)
const taskReview = ref<ReviewSummary | null>(null)
const taskError = ref('')
const agentList = ref<AgentDefinition[]>([])
const selectedAgent = ref<string | null>(null)
const agentError = ref('')
const manualRuns = ref<ManualRunContext[]>([])
const selectedManualRunId = ref<number | null>(null)
const workspaceInfo = ref<ManualWorkspaceInfo | null>(null)
const manualValue = ref('')
const selectedAgentDefinition = computed(() => agentList.value.find(agent => agent.id === selectedAgent.value) ?? null)
const selectedManualRun = computed(() => manualRuns.value.find(item => item.run.id === selectedManualRunId.value) ?? null)
const queue = computed(() => snapshot.value?.dashboard.queue)
const queueCategories = ['ready', 'blocked', 'active', 'review', 'backlog', 'done', 'cancelled'] as const satisfies ReadonlyArray<keyof QueueReport>
const filteredTasks = computed<QueueEntry[]>(() => {
  if (!queue.value) return []
  if (taskFilter.value === 'all') return queueCategories.flatMap(category => queue.value?.[category] ?? [])
  return queue.value[taskFilter.value as keyof QueueReport] ?? []
})
const revisionAgents = computed(() => snapshot.value?.dashboard.agents.filter(agent => agent.enabled && agent.status === 'available') ?? [])
const leadAgents = computed(() => agentList.value.filter(agent => agent.enabled && agent.status === 'available' && agent.capabilities.some(capability => capability.toLowerCase() === 'lead' || capability.toLowerCase() === 'plan')))
const runningAgents = computed(() => snapshot.value?.dashboard.running_agents ?? [])
const healthLabel = computed(() => snapshot.value?.dashboard.repository_available ? 'HEALTHY' : 'DEGRADED')
const projectName = computed(() => snapshot.value?.dashboard.project_name || report.value?.project.name || 'CURRENT PROJECT')
const repositoryPath = computed(() => snapshot.value?.dashboard.repository_path || report.value?.project.repository || 'Location unavailable')
const projectBranch = computed(() => report.value?.project.branch || 'branch unavailable')
function blockingReason(reason: QueueEntry['blocking_reasons'][number]) {
  if (reason.explanation) return reason.explanation
  if (reason.incomplete_dependencies?.length) return `incomplete dependencies: ${reason.incomplete_dependencies.map(dependency => `${dependency.task_id} [${dependency.status ?? 'unknown'}]`).join(', ')}`
  return reason.kind.replaceAll('_', ' ')
}
async function selectTask(id: string) { selectedTask.value = id; taskError.value = ''; taskDetails.value = await api.taskDetails(id); taskReview.value = null }
async function loadReview() { if (selectedTask.value) taskReview.value = await api.review(selectedTask.value) }
function requiredInput(message: string) {
  const value = window.prompt(message)?.trim()
  return value || null
}
async function taskAction(action: string, suppliedReason?: string) {
  if (!selectedTask.value) return
  let reason = suppliedReason
  let agentId: string | undefined
  if (action === 'reject' || action === 'cancel') {
    reason = requiredInput(`${action === 'reject' ? 'Rejection' : 'Cancellation'} reason for ${selectedTask.value}:`) ?? undefined
    if (!reason) return
  }
  if (action === 'revise') {
    reason = requiredInput(`Revision feedback for ${selectedTask.value}:`) ?? undefined
    if (!reason) return
    const choices = revisionAgents.value.map(agent => `${agent.id} (${agent.display_name})`).join('\n')
    agentId = requiredInput(`Agent id for the revision:${choices ? `\n\nAvailable agents:\n${choices}` : ''}`) ?? undefined
    if (!agentId) return
  }
  if (action === 'add_dependency') {
    reason = requiredInput(`Dependency task id for ${selectedTask.value}:`) ?? undefined
    if (!reason) return
  }
  if (!window.confirm(`${action} task ${selectedTask.value}${reason ? `?\n\nReason: ${reason}` : '?'}${agentId ? `\nAgent: ${agentId}` : ''}`)) return
  try {
    await api.taskAction(action, selectedTask.value, reason, agentId)
    await refreshSnapshot()
    await selectTask(selectedTask.value)
  } catch (err: unknown) {
    taskError.value = String(err)
  }
}
async function refreshSnapshot() { snapshot.value = await api.snapshot() }
async function refreshProjects() { projects.value = await api.registeredProjects(); activeProject.value = await api.currentProject() }
function resetProjectState() { snapshot.value = null; report.value = null; runsWorkspace.value = null; selectedRun.value = null; lead.value = null; proposals.value = []; planning.value = null; plan.value = null; approvalList.value = []; taskDetails.value = null; taskReview.value = null; agentList.value = []; manualRuns.value = []; workspaceInfo.value = null; selectedTask.value = null; selectedAgent.value = null }
async function openPickerProject() { resetProjectState(); await refreshProjects(); active.value = 'Dashboard'; error.value = ''; await refreshSnapshot(); await refreshRuns(); await refreshControl('Project') }
async function closeActiveProject() { if (!activeProject.value) return; closeConfirmation.value = true }
async function confirmCloseProject() { try { await api.closeProject(); resetProjectState(); activeProject.value = null; closeConfirmation.value = false; await refreshProjects() } catch (err: unknown) { error.value = String(err) } }
async function dispatchReady() {
  const ready = queue.value?.ready ?? []
  if (!ready.length) return
  try { await api.dispatch(ready[0].task.id); await refreshSnapshot() } catch (err: unknown) { error.value = String(err) }
}
async function refreshRuns() { try { runsWorkspace.value = await api.runsWorkspace(); runError.value = '' } catch (err: unknown) { runError.value = String(err) } }
async function refreshAgents() { try { agentList.value = await api.agents(); if (!selectedAgent.value && agentList.value.length) selectedAgent.value = agentList.value[0].id; if (selectedAgent.value) await selectAgent(selectedAgent.value); agentError.value = '' } catch (err: unknown) { agentError.value = String(err) } }
async function selectAgent(id: string) { selectedAgent.value = id; manualRuns.value = []; selectedManualRunId.value = null; workspaceInfo.value = null; const agent = agentList.value.find(item => item.id === id); if (agent?.execution_mode === 'manual') { try { [manualRuns.value, workspaceInfo.value] = await Promise.all([api.manualRuns(id), api.manualWorkspaceInfo(id)]); selectedManualRunId.value = manualRuns.value[0]?.run.id ?? null } catch (err: unknown) { agentError.value = String(err) } } }
async function updateAgent(field: string, value: string) { if (!selectedAgent.value) return; try { await api.configureAgent(selectedAgent.value, field, value); await refreshAgents() } catch (err: unknown) { agentError.value = String(err) } }
async function syncAgent() { if (!selectedAgent.value) return; try { await api.syncAgent(selectedAgent.value); await refreshAgents() } catch (err: unknown) { agentError.value = String(err) } }
async function promptAgent(field: string, current: string | number | null) { const value = window.prompt(`New ${field.replace('_', ' ')}:`, String(current ?? ''))?.trim(); if (value != null && value !== '') await updateAgent(field, value) }
async function workspaceAction(action: 'open' | 'close') { if (!selectedAgent.value) return; try { action === 'open' ? await api.openManualWorkspace(selectedAgent.value) : await api.closeManualWorkspace(selectedAgent.value); agentError.value = '' } catch (err: unknown) { agentError.value = String(err) } }
async function manualAction(action: 'submit' | 'patch' | 'fail') { if (!selectedManualRun.value || !manualValue.value.trim()) return; if (!window.confirm(`${action} manual run #${selectedManualRun.value.run.id}?`)) return; try { await api.manualRunAction(action, selectedManualRun.value.run.id, manualValue.value); manualValue.value = ''; await refreshAgents(); await refreshSnapshot() } catch (err: unknown) { agentError.value = String(err) } }
async function copyPacket() { if (selectedManualRun.value) await navigator.clipboard.writeText(selectedManualRun.value.task_packet) }
function runLabel(run: { status: string; last_activity: string; finished_at: string | null }) {
  if (run.status === 'running' && Date.now() - Date.parse(run.last_activity + 'Z') > 120000) return 'probable stall'
  return run.status
}
const quickActions: Record<string, string> = {
  'Explain project': 'Explain the current project status, priorities, and important risks concisely.',
  'Investigate failure': 'Investigate the most relevant recent failure. Explain the likely cause and propose safe next steps.',
  'Plan work': 'Plan the next useful work for this project. Create pending proposals for any actions that should be considered.',
  'Review queue': 'Review the current task queue. Highlight blockers, ordering issues, and recommended next actions.',
  'Inspect run': 'Inspect the most relevant recent run and explain its outcome, problems, and recommended follow-up.',
}
async function refreshLead() { try { [lead.value, proposals.value] = await Promise.all([api.leadContext(), api.leadProposals()]); leadError.value = '' } catch (err: unknown) { leadError.value = String(err) } }
async function sendLead(message: string, source: 'workspace' | 'panel') {
  const prompt = message.trim()
  if (!prompt || leadLoading.value) return
  leadLoading.value = true
  leadError.value = ''
  try {
    await api.invokeLead(prompt)
    if (source === 'workspace') leadMessage.value = ''
    else panelMessage.value = ''
    await refreshLead()
  } catch (err: unknown) {
    const invocationError = String(err)
    await refreshLead().catch(() => undefined)
    leadError.value = invocationError
  } finally {
    leadLoading.value = false
  }
}
function useQuickAction(action: string) { void sendLead(quickActions[action], 'workspace') }
async function resolveProposal(proposal: LeadProposal, action: 'apply' | 'reject') { if (!window.confirm(`${action === 'apply' ? 'Apply' : 'Reject'} this Lead proposal?`)) return; try { action === 'apply' ? await api.applyLeadProposal(proposal.id) : await api.rejectLeadProposal(proposal.id); await refreshLead() } catch (err: unknown) { leadError.value = String(err) } }
async function refreshControl(section: string) { try { controlError.value = ''; if (section === 'Planner') { planning.value = await api.planningRequest(); planningObjective.value = planning.value.objective } if (section === 'Approvals') approvalList.value = await api.approvals(); if (section === 'Reports' || section === 'Project') report.value = await api.projectReport(); if (section === 'Settings') { await refreshAgents(); leadConfig.value = await api.leadProviderConfig(); const template = await api.executionTemplate(templateClass.value); templateModel.value = template.model ?? ''; templateEffort.value = template.reasoning_effort ?? '' } } catch (err: unknown) { controlError.value = String(err) } }
async function runPlanner() { if (!planningObjective.value.trim()) return; try { await api.automatedPlan(planningObjective.value.trim()); const workspace = await api.runsWorkspace(); const output = [...workspace.runs].find(run => run.output?.trim().startsWith('{') && run.output.includes('"tasks"'))?.output; if (!output) throw new Error('Planner completed without a readable PlanResponse. Inspect the run output in Runs.'); planJson.value = output; plan.value = await api.plannerValidate(output); controlError.value = 'Planner completed. Review the generated plan before validating and applying it.' } catch (err: unknown) { controlError.value = String(err) } }
async function saveLeadConfig(agentId: string) { try { await api.setLeadProvider({ agent_id: agentId }); leadConfig.value = await api.leadProviderConfig() } catch (err: unknown) { controlError.value = String(err) } }
async function clearLeadConfig() { try { await api.clearLeadProvider(); leadConfig.value = null; controlError.value = 'Lead provider cleared.' } catch (err: unknown) { controlError.value = String(err) } }
async function saveTemplate() { try { await api.setExecutionTemplate(templateClass.value, templateModel.value || null, templateEffort.value || null); controlError.value = 'Execution template saved.' } catch (err: unknown) { controlError.value = String(err) } }
async function validatePlan() { try { plan.value = await api.plannerValidate(planJson.value); controlError.value = '' } catch (err: unknown) { plan.value = null; controlError.value = String(err) } }
async function applyPlan() { if (!plan.value || !window.confirm('Apply this validated plan? This creates persisted backlog tasks.')) return; try { await api.plannerApply(planJson.value); await refreshSnapshot(); controlError.value = '' } catch (err: unknown) { controlError.value = String(err) } }
async function resolveApprovalItem(item: ApprovalRequest) { if (!window.confirm(`Resolve approval #${item.id}?`)) return; try { await api.resolveApproval(item.id); await refreshControl('Approvals'); await refreshSnapshot() } catch (err: unknown) { controlError.value = String(err) } }

onMounted(async () => {
  try { await refreshProjects(); if (activeProject.value) { await refreshSnapshot(); await refreshRuns(); await refreshControl('Project') } } catch (err: unknown) { error.value = String(err) }
  await listen('orc://run-event', () => { if (active.value === 'Runs') void refreshRuns() })
})
</script>

<template>
  <div class="shell">
    <aside class="sidebar">
      <div class="brand"><span class="mark">+</span><span>ORC</span><small>LOCAL OPS</small></div>
      <nav aria-label="Primary navigation">
        <button v-for="section in sections" :key="section" :class="{ active: active === section }" @click="active = section; section === 'Lead' && refreshLead(); section === 'Agents' && refreshAgents(); ['Planner','Approvals','Reports','Project','Settings'].includes(section) && refreshControl(section)">
          <span class="nav-index">{{ String(sections.indexOf(section) + 1).padStart(2, '0') }}</span>{{ section }}
        </button>
      </nav>
      <div class="connection"><i></i><span>LOCAL / SQLITE</span><small>CONNECTED</small></div>
    </aside>
    <main class="content">
      <header class="topbar"><div class="topbar-title"><span class="eyebrow">WORKSPACE / ORC</span><h1>{{ activeProject ? active : 'PROJECT PICKER' }}</h1></div><div v-if="activeProject" class="project-context"><div class="project-switcher"><span class="project-glyph">◈</span><div><strong>{{ projectName }}</strong><small :title="repositoryPath">{{ repositoryPath }}</small></div><UiButton size="compact" @click="active = 'Project'; refreshControl('Project')">PROJECT</UiButton></div><span class="branch">⌘ {{ projectBranch }}</span><UiBadge :tone="snapshot?.dashboard.repository_available ? 'success' : 'danger'" dot>{{ healthLabel }}</UiBadge><UiBadge tone="neutral" dot>{{ snapshot?.health.unresolved_approvals ?? 0 }} APPROVALS</UiBadge><UiButton size="compact" @click="closeActiveProject">CLOSE</UiButton></div><div class="top-actions"><kbd>⌘ K</kbd><span class="avatar">RZ</span></div></header>
      <div class="workspace-scroll">
      <ProjectPicker v-if="!activeProject" :projects="projects" @changed="refreshProjects" @opened="openPickerProject" />
      <UiModal :open="closeConfirmation" title="Close active project" @close="closeConfirmation = false"><p>Close {{ activeProject?.display_name }}? The project stays registered and repository data is unchanged.</p><div class="lead-actions"><UiButton @click="closeConfirmation = false">CANCEL</UiButton><UiButton variant="primary" @click="confirmCloseProject">CLOSE PROJECT</UiButton></div></UiModal>
      <div v-else-if="error" class="notice">Unable to load project state: {{ error }}</div>
      <template v-else>
      <section v-if="active === 'Dashboard'" class="dashboard">
        <div class="intro"><div><span class="eyebrow">PROJECT / {{ snapshot?.dashboard.project_name || 'CURRENT PROJECT' }}</span><h2>Operational control surface</h2><p>{{ snapshot?.dashboard.repository_path }}</p></div><span class="timestamp">{{ healthLabel }} · {{ new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) }}</span></div>
        <div class="lead-actions"><button class="action-chip" @click="active = 'Lead'; refreshLead()">PLAN WORK</button><button class="action-chip" @click="active = 'Tasks'">NEW TASK</button><button class="action-chip" @click="dispatchReady">DISPATCH READY</button><button class="action-chip" @click="active = 'Tasks'; taskFilter = 'review'">REVIEW QUEUE</button><button class="action-chip" @click="active = 'Agents'; refreshAgents()">SYNC AGENTS</button><button class="action-chip" @click="active = 'Lead'; refreshLead()">OPEN LEAD</button></div>
        <div class="metrics"><article v-for="category in queueCategories.filter(category => ['ready', 'active', 'review', 'blocked'].includes(category))" :key="category"><span>{{ category.toUpperCase() }}</span><strong>{{ queue?.[category]?.length ?? 0 }}</strong><small>tasks in persisted state</small></article><article><span>APPROVALS</span><strong>{{ snapshot?.health?.unresolved_approvals ?? 0 }}</strong><small>awaiting resolution</small></article></div>
        <div class="grid"><article class="panel wide"><div class="panel-head"><h3>RUNNING AGENTS</h3><button class="text-button" @click="active = 'Runs'">VIEW RUNS →</button></div><div v-if="runningAgents.length" class="task-list"><div v-for="run in runningAgents" :key="run.id" class="task"><span class="status-dot active"></span><span class="task-id">{{ run.agent }}</span><span class="task-title">{{ run.task_id || 'unassigned' }} · {{ run.phase || 'starting' }} · last {{ run.last_activity }}</span><span class="pill">{{ run.status }}</span></div></div><div v-else class="empty">NO AGENTS RUNNING</div></article><article class="panel"><div class="panel-head"><h3>CAPACITY / HEALTH</h3></div><p>Busy {{ snapshot?.dashboard.capacity.busy.length ?? 0 }} / {{ snapshot?.dashboard.capacity.agents.length ?? 0 }}</p><p>Quota reserve {{ snapshot?.dashboard.capacity.quota_reserve_percent ?? 0 }}%</p><p>Repository {{ snapshot?.dashboard.repository_available ? 'available' : 'unavailable' }}</p></article></div>
        <div class="grid"><article class="panel wide"><div class="panel-head"><h3>RECENT LIFECYCLE ACTIVITY</h3></div><div v-for="event in snapshot?.dashboard.recent_activity.slice(0, 8)" :key="event.id" class="task"><span class="task-id">{{ event.timestamp }}</span><span class="task-title">{{ event.kind }} {{ event.task_id || '' }}</span></div></article><article class="panel"><div class="panel-head"><h3>OUTCOMES</h3></div><p v-for="(count, outcome) in snapshot?.dashboard.outcome_trends" :key="outcome">{{ outcome }}: {{ count }}</p></article></div>
      </section>
      <section v-else-if="active === 'Lead'" class="lead-workspace"><div class="intro"><div><span class="eyebrow">PROJECT LEAD / {{ lead?.project_name || 'CURRENT PROJECT' }}</span><h2>What should we do next?</h2><p>Guidance from the persisted Orc project state.</p></div><button class="text-button" @click="refreshLead">REFRESH ↻</button></div><div v-if="leadError" class="notice">{{ leadError }}</div><div class="lead-grid"><article class="panel lead-conversation"><div class="panel-head"><h3>CONVERSATION</h3></div><div v-if="lead?.turns.length" class="turns"><div v-for="turn in lead.turns.slice().reverse()" :key="turn.id" class="turn"><span class="turn-role">{{ turn.role.toUpperCase() }}</span><p>{{ turn.content }}</p></div></div><div v-else class="empty">Ask the Lead to explain the project, investigate a failure, plan work, review the queue, or inspect a run.</div><form class="lead-composer" @submit.prevent="sendLead(leadMessage, 'workspace')"><textarea v-model="leadMessage" aria-label="Message the Lead" placeholder="Ask the Lead about this project…" rows="3" :disabled="leadLoading"></textarea><button class="apply-button" type="submit" :disabled="leadLoading || !leadMessage.trim()">{{ leadLoading ? 'SENDING…' : 'SEND' }}</button></form><div class="lead-actions"><button v-for="action in Object.keys(quickActions)" :key="action" class="action-chip" :disabled="leadLoading" @click="useQuickAction(action)">{{ action }}</button></div></article><article class="panel"><div class="panel-head"><h3>CURRENT STATUS</h3></div><div class="status-summary"><strong>{{ lead?.tasks.length ?? 0 }}</strong><span>TASKS</span><strong>{{ lead?.runs.length ?? 0 }}</strong><span>RECENT RUNS</span><strong>{{ lead?.approvals.length ?? 0 }}</strong><span>APPROVALS</span></div></article></div><article class="panel proposals"><div class="panel-head"><h3>PENDING ACTIONS</h3><span class="pill">INSPECT BEFORE APPLY</span></div><div v-if="proposals.length" v-for="proposal in proposals" :key="proposal.id" class="proposal"><div><span class="turn-role">PROPOSAL #{{ proposal.id }} · {{ proposal.proposal.kind.toUpperCase() }}</span><pre>{{ JSON.stringify(proposal.proposal.details, null, 2) }}</pre></div><div class="proposal-controls"><button class="apply-button" @click="resolveProposal(proposal, 'apply')">APPLY</button><button class="reject-button" @click="resolveProposal(proposal, 'reject')">REJECT</button></div></div><div v-else class="empty">NO PENDING ACTIONS</div></article></section>
      <section v-else-if="active === 'Runs'" class="runs-workspace"><div class="intro"><div><span class="eyebrow">EXECUTION / LIVE RUNS</span><h2>Runs</h2><p>Current and historical agent execution, backed by persisted lifecycle events.</p></div><button class="text-button" @click="refreshRuns">REFRESH ↻</button></div><div v-if="runError" class="notice">{{ runError }}</div><div class="runs-grid"><article class="panel run-list"><div class="panel-head"><h3>RUN HISTORY</h3><span class="pill">{{ runsWorkspace?.runs.length ?? 0 }} RUNS</span></div><button v-for="run in runsWorkspace?.runs ?? []" :key="run.id" class="run-row" :class="{ selected: selectedRun === run.id }" @click="selectedRun = run.id"><span class="status-dot" :class="run.status"></span><span><strong>#{{ run.id }} · {{ run.agent }}</strong><small>{{ run.task_id || 'no task' }} · {{ run.execution_mode }}</small></span><span class="pill">{{ runLabel(run) }}</span></button><div v-if="!runsWorkspace?.runs.length" class="empty">NO RUNS FOUND</div></article><article v-if="selectedDetail" class="panel run-detail"><div class="panel-head"><h3>RUN #{{ selectedDetail.run.id }} · {{ selectedDetail.run.agent }}</h3><span class="pill">{{ runLabel(selectedDetail.run) }}</span></div><div class="run-meta"><span>PHASE <b>{{ selectedDetail.run.phase || '—' }}</b></span><span>STARTED <b>{{ selectedDetail.run.started_at }}</b></span><span>LAST ACTIVITY <b>{{ selectedDetail.run.last_activity }}</b></span><span>RESULT <b>{{ selectedDetail.result?.outcome || 'in progress' }}</b></span><span>TOKENS <b>{{ selectedDetail.result?.total_tokens?.toLocaleString() || 'unavailable' }}</b><small v-if="selectedDetail.result?.total_tokens != null">{{ selectedDetail.result.input_tokens?.toLocaleString() ?? '—' }} input · {{ selectedDetail.result.output_tokens?.toLocaleString() ?? '—' }} output</small></span></div><h3>VALIDATION / LIFECYCLE</h3><div class="events"><div v-for="event in selectedDetail.activity" :key="event.id"><time>{{ event.timestamp }}</time><b>{{ event.kind }}</b><span>{{ event.payload || '' }}</span></div></div><h3>FINAL OUTPUT</h3><pre class="run-output">{{ selectedDetail.run.output || 'No final output yet.' }}</pre></article><article v-else class="panel empty run-detail">Select a run to inspect its phase, events, validation, and complete output.</article></div></section>
      <section v-else-if="active === 'Tasks'" class="tasks-workspace"><div class="intro"><div><span class="eyebrow">OPERATIONS / TASK QUEUE</span><h2>Tasks & queue</h2><p>Filter work, inspect readiness, and review consequential lifecycle actions.</p></div><button class="text-button" @click="refreshSnapshot">REFRESH ↻</button></div><div v-if="taskError" class="notice">{{ taskError }}</div><div class="task-toolbar"><button v-for="filter in ['all', ...queueCategories]" :key="filter" class="action-chip" :class="{ active: taskFilter === filter }" @click="taskFilter = filter">{{ filter.toUpperCase() }}</button></div><div class="tasks-grid"><article class="panel task-list"><button v-for="entry in filteredTasks" :key="entry.task.id" class="run-row" :class="{ selected: selectedTask === entry.task.id }" @click="selectTask(entry.task.id)"><span class="status-dot" :class="entry.category"></span><span><strong>{{ entry.task.id }} · {{ entry.task.title }}</strong><small>{{ entry.task.role }} · {{ entry.task.priority }} · persisted {{ entry.task.status }}</small></span><span class="pill">{{ entry.category }}</span></button><div v-if="!filteredTasks.length" class="empty">NO TASKS MATCH THIS FILTER</div></article><article class="panel task-detail" v-if="taskDetails"><div class="panel-head"><h3>{{ taskDetails.task.id }} · {{ taskDetails.task.title }}</h3><span class="pill">{{ taskDetails.queue?.category ?? taskDetails.task.status }}</span></div><p>{{ taskDetails.task.objective }}</p><p><b>Scope:</b> {{ taskDetails.task.scope_mode || 'unspecified' }}</p><p><b>Context:</b> {{ taskDetails.task.context_files.join(', ') || 'none' }}</p><p><b>Expected changes:</b> {{ taskDetails.task.expected_changes.join(', ') || 'none' }}</p><p v-if="taskDetails.queue?.blocking_reasons.length"><b>Why blocked:</b> {{ taskDetails.queue.blocking_reasons.map(blockingReason).join('; ') }}</p><p v-else-if="taskDetails.queue?.category === 'ready'"><b>Why ready:</b> dependencies and agent eligibility are satisfied</p><p><b>Dependencies:</b> {{ taskDetails.queue?.dependencies.map(dep => dep.task_id + ' [' + (dep.status || 'unknown') + ']').join(', ') || 'none' }}</p><div class="lead-actions"><button class="text-button" @click="taskAction('add_dependency')" v-if="taskDetails.queue && !['active','review','done','cancelled'].includes(taskDetails.queue.category)">ADD DEPENDENCY</button><button v-for="dependency in taskDetails.queue?.dependencies ?? []" :key="dependency.task_id" class="text-button" @click="taskAction('remove_dependency', dependency.task_id)">REMOVE {{ dependency.task_id }}</button></div><div class="lead-actions"><button class="apply-button" @click="taskAction('dispatch')" v-if="taskDetails.queue?.category === 'ready'">DISPATCH</button><button class="apply-button" @click="taskAction('accept')" v-if="taskDetails.queue?.category === 'review'">ACCEPT</button><button class="apply-button" @click="taskAction('revise')" v-if="taskDetails.queue?.category === 'review'">REVISE</button><button class="reject-button" @click="taskAction('reject')" v-if="taskDetails.queue?.category === 'review'">REJECT</button><button class="apply-button" @click="taskAction('requeue')" v-if="taskDetails.queue?.category === 'active'">REQUEUE</button><button class="reject-button" @click="taskAction('cancel')" v-if="taskDetails.queue && !['done','cancelled'].includes(taskDetails.queue.category)">CANCEL</button><button class="text-button" @click="loadReview">REVIEW DIFF</button></div><pre v-if="taskReview" class="run-output">{{ taskReview.changes.stat }}\n{{ taskReview.changes.diff }}</pre><h3>RUNS ({{ taskDetails.runs.length }})</h3><div v-for="run in taskDetails.runs" :key="run.id" class="event"><b>#{{ run.id }} {{ run.status }}</b> · {{ run.agent }}</div></article><article v-else class="panel empty task-detail">Select a task to inspect its objective, dependencies, runs, and review surface.</article></div></section>
      <section v-else-if="active === 'Agents'" class="agents-workspace"><div class="intro"><div><span class="eyebrow">EXECUTION / AGENT REGISTRY</span><h2>Agents</h2><p>Availability, capacity, execution settings, and manual provider workspaces.</p></div><button class="text-button" @click="refreshAgents">REFRESH ↻</button></div><div v-if="agentError" class="notice">{{ agentError }}</div><div class="agents-grid"><article class="panel agent-list"><button v-for="agent in agentList" :key="agent.id" class="run-row" :class="{ selected: selectedAgent === agent.id }" @click="selectAgent(agent.id)"><span class="status-dot" :class="agent.status"></span><span><strong>{{ agent.display_name }}</strong><small>{{ agent.id }} · {{ agent.backend }} · {{ agent.execution_mode }}</small></span><span class="pill">{{ agent.enabled ? agent.status : 'disabled' }}</span></button></article><article v-if="selectedAgentDefinition" class="panel agent-detail"><div class="panel-head"><h3>{{ selectedAgentDefinition.display_name }} · {{ selectedAgentDefinition.id }}</h3><button class="apply-button" @click="updateAgent('enabled', String(!selectedAgentDefinition.enabled))">{{ selectedAgentDefinition.enabled ? 'DISABLE' : 'ENABLE' }}</button></div><div class="agent-fields"><span>BACKEND <b>{{ selectedAgentDefinition.backend }}</b></span><span>MODE <b>{{ selectedAgentDefinition.execution_mode }}</b></span><span>AVAILABILITY <b>{{ selectedAgentDefinition.status }}</b><small>{{ selectedAgentDefinition.unavailable_reason || 'no reported issue' }}</small></span><span>PRIORITY <b>{{ selectedAgentDefinition.priority }}</b><button class="text-button" @click="promptAgent('priority', selectedAgentDefinition.priority)">EDIT</button></span><span>CAPABILITIES <b>{{ selectedAgentDefinition.capabilities.join(', ') || 'none' }}</b></span><span>MODEL <b>{{ selectedAgentDefinition.model || 'default' }}</b><button v-if="selectedAgentDefinition.backend === 'codex' && selectedAgentDefinition.execution_mode === 'automated'" class="text-button" @click="promptAgent('model', selectedAgentDefinition.model)">EDIT</button></span><span>REASONING <b>{{ selectedAgentDefinition.reasoning_effort || 'default' }}</b><button v-if="selectedAgentDefinition.backend === 'codex' && selectedAgentDefinition.execution_mode === 'automated'" class="text-button" @click="promptAgent('reasoning_effort', selectedAgentDefinition.reasoning_effort)">EDIT</button></span><span>PROFILE <b>{{ selectedAgentDefinition.profile_path || 'not set' }}</b><button class="text-button" @click="promptAgent('profile_path', selectedAgentDefinition.profile_path)">EDIT</button></span><span>QUOTA <b>{{ selectedAgentDefinition.quota_remaining_percent == null ? 'unknown' : selectedAgentDefinition.quota_remaining_percent + '%' }}</b><small>reset {{ selectedAgentDefinition.quota_reset_at || 'unknown' }} · {{ selectedAgentDefinition.quota_source || 'no source' }}</small></span></div><div v-if="selectedAgentDefinition.execution_mode === 'manual'" class="manual-workspace"><div class="panel-head"><h3>MANUAL PROVIDER WORKSPACE</h3><span class="pill">{{ workspaceInfo?.url || workspaceInfo?.error }}</span></div><div class="lead-actions"><button class="apply-button" :disabled="!workspaceInfo?.supported" @click="workspaceAction('open')">OPEN PROVIDER</button><button class="text-button" @click="workspaceAction('close')">CLOSE PROVIDER</button></div><div v-if="manualRuns.length" class="run-list"><button v-for="item in manualRuns" :key="item.run.id" class="run-row" :class="{ selected: selectedManualRunId === item.run.id }" @click="selectedManualRunId = item.run.id; manualValue = ''"><span class="status-dot" :class="item.run.status"></span><span><strong>RUN #{{ item.run.id }} · {{ item.task.id }}</strong><small>{{ item.task.title }}</small></span><span class="pill">{{ item.run.status }}</span></button></div><div v-if="selectedManualRun" class="manual-run"><h3>WAITING RUN #{{ selectedManualRun.run.id }} · {{ selectedManualRun.task.id }}</h3><p>{{ selectedManualRun.task.title }} — {{ selectedManualRun.task.objective }}</p><button class="text-button" @click="copyPacket">COPY TASK PACKET</button><pre class="run-output">{{ selectedManualRun.task_packet }}</pre><textarea v-model="manualValue" rows="7" placeholder="Paste provider response or unified patch"></textarea><div class="lead-actions"><button class="apply-button" @click="manualAction('submit')">SUBMIT OUTPUT</button><button class="apply-button" @click="manualAction('patch')">SUBMIT PATCH</button><button class="reject-button" @click="manualAction('fail')">FAIL RUN</button></div></div><div v-else class="empty">NO WAITING_EXTERNAL RUNS FOR THIS AGENT</div></div></article></div></section>
      <section v-else-if="active === 'Planner'" class="panel"><div class="intro"><div><span class="eyebrow">CONTROL PLANE / PLANNER</span><h2>Plan the next work</h2><p>Describe the objective; the generated plan remains subject to human validation and approval.</p></div><button class="text-button" @click="refreshControl('Planner')">REFRESH ↻</button></div><div v-if="controlError" class="notice">{{ controlError }}</div><form class="lead-composer" @submit.prevent="runPlanner"><textarea v-model="planningObjective" rows="4" placeholder="What should the planner accomplish?" aria-label="Planning objective"></textarea><button class="apply-button" type="submit" :disabled="!planningObjective.trim()">RUN PLANNER</button></form><div v-if="plan" class="grid"><article class="panel"><h3>PLAN · {{ plan.objective }}</h3><p v-if="plan.assumptions.length"><b>Assumptions:</b> {{ plan.assumptions.join('; ') }}</p><div v-for="task in plan.tasks" :key="task.local_id" class="proposal"><div><b>{{ task.local_id }} · {{ task.title }}</b><p>{{ task.objective }}</p><small>Role: {{ task.role }} · Priority: {{ task.priority }} · Depends on: {{ task.depends_on.join(', ') || 'none' }}</small></div></div></article><article class="panel"><h3>RISKS / QUESTIONS</h3><p v-for="item in [...plan.risks, ...plan.questions]" :key="item">{{ item }}</p><p v-if="!plan.risks.length && !plan.questions.length">None reported.</p></article></div><div v-if="plan" class="lead-actions"><button class="text-button" @click="validatePlan">VALIDATE PLAN</button><button class="apply-button" :disabled="!plan" @click="applyPlan">APPROVE & APPLY</button></div><UiDisclosure title="Advanced / Raw PlanResponse"><textarea v-model="planJson" rows="12" placeholder="PlanResponse JSON"></textarea></UiDisclosure><UiDisclosure v-if="planning?.full_report" title="Planning context"><pre class="run-output">{{ JSON.stringify(planning.full_report, null, 2) }}</pre></UiDisclosure></section>
      <section v-else-if="active === 'Approvals'" class="panel"><div class="intro"><div><span class="eyebrow">CONTROL PLANE / APPROVALS</span><h2>Pending decisions</h2><p>Review the reason and context, then explicitly resolve each request.</p></div><button class="text-button" @click="refreshControl('Approvals')">REFRESH ↻</button></div><div v-if="controlError" class="notice">{{ controlError }}</div><div v-for="item in approvalList" :key="item.id" class="proposal"><div><b>#{{ item.id }}</b><span class="pill">{{ item.resolved ? 'RESOLVED' : 'PENDING' }}</span><p><b>Reason:</b> {{ item.reason }}</p><small>Context: This request is persisted by the project control plane and requires operator resolution.</small></div><button v-if="!item.resolved" class="apply-button" @click="resolveApprovalItem(item)">RESOLVE</button></div><div v-if="!approvalList.length" class="empty">NO APPROVAL REQUESTS</div></section>
      <section v-else-if="active === 'Reports'" class="panel"><div class="intro"><div><span class="eyebrow">CONTROL PLANE / REPORTS</span><h2>Project report</h2><p>Structured lifecycle, result, risk, and planning summary.</p></div><button class="text-button" @click="refreshControl('Reports')">REFRESH ↻</button></div><div v-if="controlError" class="notice">{{ controlError }}</div><template v-if="report"><div class="metrics"><article><span>TASKS</span><strong>{{ report.lifecycle.tasks.length }}</strong><small>reported lifecycle records</small></article><article><span>RISKS</span><strong>{{ report.risks.length }}</strong><small>open report risks</small></article><article><span>QUESTIONS</span><strong>{{ report.open_questions.length }}</strong><small>open questions</small></article></div><h3>LIFECYCLE COUNTS</h3><p>{{ JSON.stringify(report.lifecycle.counts) }}</p><h3>RECENT WORK</h3><pre class="run-output">{{ JSON.stringify(report.recent_work, null, 2) }}</pre><h3>RISKS & OPEN QUESTIONS</h3><p>{{ [...report.risks, ...report.open_questions].join('; ') || 'None reported.' }}</p></template></section>
      <section v-else-if="active === 'Project'" class="panel"><div class="intro"><div><span class="eyebrow">CONTROL PLANE / PROJECT</span><h2>Project identity, state, and health</h2><p>Application-owned project facts from the shared read models.</p></div><button class="text-button" @click="refreshControl('Project')">REFRESH ↻</button></div><div v-if="controlError" class="notice">{{ controlError }}</div><template v-if="report"><p><b>Name:</b> {{ report.project.name }}</p><p><b>Repository:</b> {{ report.project.repository }}</p><p><b>Branch:</b> {{ report.project.branch || 'unknown' }}</p><p><b>Commit:</b> {{ report.project.commit || 'unknown' }}</p><h3>HEALTH</h3><p><b>Task counts:</b> {{ JSON.stringify(snapshot?.health.task_counts) }}</p><p><b>Active runs:</b> {{ snapshot?.health.active_runs }}</p><p><b>Unresolved approvals:</b> {{ snapshot?.health.unresolved_approvals }}</p><h3>ARCHITECTURE & DISCOVERY</h3><p>{{ report.architecture.modules.join(', ') || 'No modules recorded.' }}</p><p v-for="(value, key) in report.architecture.discovery" :key="key"><b>{{ key }}:</b> {{ value }}</p></template></section>
      <section v-else-if="active === 'Settings'" class="panel"><div class="intro"><div><span class="eyebrow">CONTROL PLANE / SETTINGS</span><h2>Persistent project settings</h2><p>These values are persisted through the existing project APIs.</p></div><button class="text-button" @click="refreshControl('Settings')">REFRESH ↻</button></div><div v-if="controlError" class="notice">{{ controlError }}</div><h3>LEAD PROVIDER</h3><p v-if="leadConfig">Currently configured: <b>{{ leadConfig.agent_id }}</b></p><p v-else>No Lead provider configured. Select an eligible agent to enable Lead interaction.</p><div class="lead-actions"><select :value="leadConfig?.agent_id || ''" @change="saveLeadConfig(($event.target as HTMLSelectElement).value)"><option value="" disabled>Select Lead-capable agent</option><option v-for="agent in leadAgents" :key="agent.id" :value="agent.id">{{ agent.display_name }} · {{ agent.id }}</option></select><button class="text-button" :disabled="!leadConfig" @click="clearLeadConfig">CLEAR LEAD</button></div><h3>EXECUTION TEMPLATE</h3><div class="agent-fields"><label>CLASS <select v-model="templateClass" @change="refreshControl('Settings')"><option value="coder">Coder</option><option value="reviewer">Reviewer</option><option value="planner">Planner</option><option value="lead">Lead</option></select></label><label>MODEL <input v-model="templateModel" placeholder="default" /></label><label>REASONING <select v-model="templateEffort"><option value="">default</option><option value="None">None</option><option value="Low">Low</option><option value="Medium">Medium</option><option value="High">High</option></select></label></div><button class="apply-button" @click="saveTemplate">SAVE TEMPLATE</button><h3>AGENT SETTINGS</h3><p>Model, profile, enablement, and provider workspace controls remain in Agents.</p><button class="apply-button" @click="active = 'Agents'; refreshAgents()">OPEN AGENT SETTINGS</button></section>
      <section v-else class="placeholder"><span class="eyebrow">SECTION / {{ active.toUpperCase() }}</span><h2>{{ active }}</h2><p>This workspace is connected to the Orc application API. Its operational view will appear here as the section is implemented.</p></section>
      </template>
      </div>
    </main>
    <button v-if="active !== 'Lead'" class="lead-fab" @click="leadPanel = !leadPanel; refreshLead()">LEAD</button><aside v-if="leadPanel" class="lead-panel"><div class="panel-head"><h3>LEAD</h3><button class="text-button" @click="leadPanel = false">CLOSE</button></div><p>Ask about the current project from any screen.</p><div v-if="leadError" class="notice compact">{{ leadError }}</div><form class="lead-composer compact" @submit.prevent="sendLead(panelMessage, 'panel')"><textarea v-model="panelMessage" aria-label="Message the Lead" placeholder="Ask the Lead…" rows="3" :disabled="leadLoading"></textarea><button class="apply-button" type="submit" :disabled="leadLoading || !panelMessage.trim()">{{ leadLoading ? 'SENDING…' : 'SEND' }}</button></form><button class="text-button open-lead" @click="active = 'Lead'; leadPanel = false; refreshLead()">OPEN WORKSPACE →</button></aside>
  </div>
</template>
