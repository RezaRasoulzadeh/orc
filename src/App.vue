<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { api, type DesktopSnapshot } from './lib/api'

const sections = ['Dashboard', 'Tasks', 'Runs', 'Agents', 'Lead', 'Planner', 'Approvals', 'Reports', 'Project', 'Settings']
const active = ref('Dashboard')
const snapshot = ref<DesktopSnapshot | null>(null)
const error = ref('')
const tasks = computed(() => snapshot.value?.dashboard?.tasks ?? [])

onMounted(async () => {
  try { snapshot.value = await api.snapshot() } catch (err: unknown) { error.value = String(err) }
})
</script>

<template>
  <div class="shell">
    <aside class="sidebar">
      <div class="brand"><span class="mark">+</span><span>ORC</span><small>LOCAL OPS</small></div>
      <nav aria-label="Primary navigation">
        <button v-for="section in sections" :key="section" :class="{ active: active === section }" @click="active = section">
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
      <section v-else class="placeholder"><span class="eyebrow">SECTION / {{ active.toUpperCase() }}</span><h2>{{ active }}</h2><p>This workspace is connected to the Orc application API. Its operational view will appear here as the section is implemented.</p></section>
    </main>
  </div>
</template>
