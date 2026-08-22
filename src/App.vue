<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { api, type DesktopSnapshot, type LeadContext, type LeadProposal } from './lib/api'

const sections = ['Dashboard', 'Tasks', 'Runs', 'Agents', 'Lead', 'Planner', 'Approvals', 'Reports', 'Project', 'Settings']
const active = ref('Dashboard')
const snapshot = ref<DesktopSnapshot | null>(null)
const error = ref('')
const tasks = computed(() => snapshot.value?.dashboard?.tasks ?? [])
const lead = ref<LeadContext | null>(null)
const proposals = ref<LeadProposal[]>([])
const leadPanel = ref(false)
const leadError = ref('')
const leadMessage = ref('')
const panelMessage = ref('')
const leadLoading = ref(false)
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

onMounted(async () => {
  try { snapshot.value = await api.snapshot() } catch (err: unknown) { error.value = String(err) }
})
</script>

<template>
  <div class="shell">
    <aside class="sidebar">
      <div class="brand"><span class="mark">+</span><span>ORC</span><small>LOCAL OPS</small></div>
      <nav aria-label="Primary navigation">
        <button v-for="section in sections" :key="section" :class="{ active: active === section }" @click="active = section; section === 'Lead' && refreshLead()">
          <span class="nav-index">{{ String(sections.indexOf(section) + 1).padStart(2, '0') }}</span>{{ section }}
        </button>
      </nav>
      <div class="connection"><i></i><span>LOCAL / SQLITE</span><small>CONNECTED</small></div>
    </aside>
    <main class="content">
      <header class="topbar"><div><span class="eyebrow">WORKSPACE / ORC</span><h1>{{ active }}</h1></div><div class="top-actions"><kbd>⌘ K</kbd><span class="avatar">RZ</span></div></header>
      <div v-if="error" class="notice">Unable to load project state: {{ error }}</div>
      <section v-else-if="active === 'Dashboard'" class="dashboard">
        <div class="intro"><div><span class="eyebrow">SYSTEM OVERVIEW</span><h2>Good evening, operator.</h2><p>Project state at a glance. All systems are local and under control.</p></div><span class="timestamp">LIVE · {{ new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) }}</span></div>
        <div class="metrics"><article><span>TASKS</span><strong>{{ tasks.length }}</strong><small>tracked in project</small></article><article><span>ACTIVE RUNS</span><strong>{{ snapshot?.health?.active_runs ?? 0 }}</strong><small>currently executing</small></article><article><span>APPROVALS</span><strong>{{ snapshot?.health?.unresolved_approvals ?? 0 }}</strong><small>awaiting review</small></article></div>
        <div class="grid"><article class="panel wide"><div class="panel-head"><h3>RECENT TASKS</h3><button class="text-button" @click="active = 'Tasks'">VIEW ALL →</button></div><div v-if="tasks.length" class="task-list"><div v-for="task in tasks.slice(0, 6)" :key="task.id" class="task"><span class="status-dot" :class="task.status"></span><span class="task-id">{{ task.id }}</span><span class="task-title">{{ task.title }}</span><span class="pill">{{ task.status }}</span></div></div><div v-else class="empty">NO TASKS FOUND IN CURRENT PROJECT</div></article><article class="panel"><div class="panel-head"><h3>QUEUE SIGNAL</h3></div><div class="signal"><span class="signal-line"></span><strong>{{ snapshot?.dashboard?.queue?.ready?.length ?? 0 }}</strong><small>items ready for dispatch</small></div></article></div>
      </section>
      <section v-else-if="active === 'Lead'" class="lead-workspace"><div class="intro"><div><span class="eyebrow">PROJECT LEAD / {{ lead?.project_name || 'CURRENT PROJECT' }}</span><h2>What should we do next?</h2><p>Guidance from the persisted Orc project state.</p></div><button class="text-button" @click="refreshLead">REFRESH ↻</button></div><div v-if="leadError" class="notice">{{ leadError }}</div><div class="lead-grid"><article class="panel lead-conversation"><div class="panel-head"><h3>CONVERSATION</h3></div><div v-if="lead?.turns.length" class="turns"><div v-for="turn in lead.turns.slice().reverse()" :key="turn.id" class="turn"><span class="turn-role">{{ turn.role.toUpperCase() }}</span><p>{{ turn.content }}</p></div></div><div v-else class="empty">Ask the Lead to explain the project, investigate a failure, plan work, review the queue, or inspect a run.</div><form class="lead-composer" @submit.prevent="sendLead(leadMessage, 'workspace')"><textarea v-model="leadMessage" aria-label="Message the Lead" placeholder="Ask the Lead about this project…" rows="3" :disabled="leadLoading"></textarea><button class="apply-button" type="submit" :disabled="leadLoading || !leadMessage.trim()">{{ leadLoading ? 'SENDING…' : 'SEND' }}</button></form><div class="lead-actions"><button v-for="action in Object.keys(quickActions)" :key="action" class="action-chip" :disabled="leadLoading" @click="useQuickAction(action)">{{ action }}</button></div></article><article class="panel"><div class="panel-head"><h3>CURRENT STATUS</h3></div><div class="status-summary"><strong>{{ lead?.tasks.length ?? 0 }}</strong><span>TASKS</span><strong>{{ lead?.runs.length ?? 0 }}</strong><span>RECENT RUNS</span><strong>{{ lead?.approvals.length ?? 0 }}</strong><span>APPROVALS</span></div></article></div><article class="panel proposals"><div class="panel-head"><h3>PENDING ACTIONS</h3><span class="pill">INSPECT BEFORE APPLY</span></div><div v-if="proposals.length" v-for="proposal in proposals" :key="proposal.id" class="proposal"><div><span class="turn-role">PROPOSAL #{{ proposal.id }} · {{ proposal.proposal.kind.toUpperCase() }}</span><pre>{{ JSON.stringify(proposal.proposal.details, null, 2) }}</pre></div><div class="proposal-controls"><button class="apply-button" @click="resolveProposal(proposal, 'apply')">APPLY</button><button class="reject-button" @click="resolveProposal(proposal, 'reject')">REJECT</button></div></div><div v-else class="empty">NO PENDING ACTIONS</div></article></section>
      <section v-else class="placeholder"><span class="eyebrow">SECTION / {{ active.toUpperCase() }}</span><h2>{{ active }}</h2><p>This workspace is connected to the Orc application API. Its operational view will appear here as the section is implemented.</p></section>
    </main>
    <button v-if="active !== 'Lead'" class="lead-fab" @click="leadPanel = !leadPanel; refreshLead()">LEAD</button><aside v-if="leadPanel" class="lead-panel"><div class="panel-head"><h3>LEAD</h3><button class="text-button" @click="leadPanel = false">CLOSE</button></div><p>Ask about the current project from any screen.</p><div v-if="leadError" class="notice compact">{{ leadError }}</div><form class="lead-composer compact" @submit.prevent="sendLead(panelMessage, 'panel')"><textarea v-model="panelMessage" aria-label="Message the Lead" placeholder="Ask the Lead…" rows="3" :disabled="leadLoading"></textarea><button class="apply-button" type="submit" :disabled="leadLoading || !panelMessage.trim()">{{ leadLoading ? 'SENDING…' : 'SEND' }}</button></form><button class="text-button open-lead" @click="active = 'Lead'; leadPanel = false; refreshLead()">OPEN WORKSPACE →</button></aside>
  </div>
</template>
