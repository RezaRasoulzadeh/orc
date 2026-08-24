import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'esbuild'
import { compileScript, parse } from '@vue/compiler-sfc'

const registeredProject = { id: 'registered-1', display_name: 'Orc', repository_root: '/repo', project_id: 1, project_name: 'orc', last_opened_at: null, available: true, status: 'Available' }
const state = { api: null }
globalThis.window = { addEventListener() {}, removeEventListener() {} }
globalThis.__orcApi = new Proxy({}, { get: (_, key) => (...args) => state.api[key](...args) })
globalThis.__orcRunEvent = null

const buildDirectory = await mkdtemp(join(tmpdir(), 'orc-app-test-'))
const bundle = join(buildDirectory, 'app.mjs')
await build({
  stdin: { contents: "import App from './src/App.vue'; export { createRenderer, nextTick } from 'vue'; export default App", resolveDir: process.cwd(), sourcefile: 'app-test-entry.ts' },
  outfile: bundle,
  bundle: true,
  format: 'esm',
  platform: 'node',
  plugins: [
    { name: 'app-test-mocks', setup(build) { build.onResolve({ filter: /^@tauri-apps\/api\/event$/ }, () => ({ path: 'event-mock', namespace: 'mock' })); build.onResolve({ filter: /lib\/api$/ }, () => ({ path: 'api-mock', namespace: 'mock' })); build.onLoad({ filter: /.*/, namespace: 'mock' }, args => ({ contents: args.path === 'event-mock' ? 'export async function listen(_name, handler) { globalThis.__orcRunEvent = handler; return () => { globalThis.__orcRunEvent = null } }' : 'export const api = globalThis.__orcApi', loader: 'js' })) } },
    { name: 'vue-sfc', setup(build) { build.onLoad({ filter: /\.vue$/ }, async args => { const source = await readFile(args.path, 'utf8'); const { descriptor } = parse(source, { filename: args.path }); const compiled = compileScript(descriptor, { id: args.path, inlineTemplate: true }); return { contents: compiled.content, loader: 'ts', resolveDir: new URL('.', pathToFileURL(args.path)).pathname } }) } },
  ],
})
const compiled = await import(`${pathToFileURL(bundle)}?${Date.now()}`)
const { createRenderer, nextTick } = compiled
const App = compiled.default
after(() => rm(buildDirectory, { recursive: true, force: true }))

function apiHarness(options = {}) {
  let current = options.current ?? null
  const calls = []
  const handlers = {
    currentProject: async () => { calls.push('currentProject'); return current },
    registeredProjects: async () => { calls.push('registeredProjects'); return [registeredProject] },
    importProject: async () => { calls.push('importProject'); return registeredProject },
    adoptProject: async () => { calls.push('adoptProject'); return registeredProject },
    openProject: async id => { calls.push(`openProject:${id}`); if (!options.failActivation) current = registeredProject },
    closeProject: async () => { calls.push('closeProject'); current = null },
    snapshot: async () => { calls.push('snapshot'); return options.snapshot ? options.snapshot() : { dashboard: { tasks: [], queue: {}, agents: [], running_agents: [], project_name: 'orc', repository_path: '/repo', repository_available: true, capacity: { busy: [], agents: [], quota_reserve_percent: 0 }, recent_activity: [], outcome_trends: {} }, health: { task_counts: {}, unresolved_approvals: 0 } } },
    runsWorkspace: async () => { calls.push('runsWorkspace'); return options.runsWorkspace ? options.runsWorkspace() : { runs: [], details: [] } },
    runDetails: async id => { calls.push(`runDetails:${id}`); return options.runDetails ? options.runDetails(id) : null },
    reviewRun: async id => { calls.push(`reviewRun:${id}`); return options.reviewRun ? options.reviewRun(id) : null },
    workerLog: async id => { calls.push(`workerLog:${id}`); return options.workerLog ? options.workerLog(id) : [] },
    projectReport: async () => { calls.push('projectReport'); return { project: { name: 'orc', repository: '/repo', branch: 'main' } } },
  }
  state.api = new Proxy(handlers, { get: (target, key) => target[key] ?? (async () => { throw new Error(`unexpected API call: ${String(key)}`) }) })
  return { calls }
}

function mountApp() {
  const root = { type: 'root', children: [], parent: null, props: {}, text: '' }
  const renderer = createRenderer({
    patchProp(node, key, _previous, value) { node.props[key] = value },
    insert(node, parent, anchor) { node.parent = parent; const index = anchor ? parent.children.indexOf(anchor) : -1; if (index < 0) parent.children.push(node); else parent.children.splice(index, 0, node) },
    remove(node) { const index = node.parent?.children.indexOf(node) ?? -1; if (index >= 0) node.parent.children.splice(index, 1) },
    createElement(type) { return { type, children: [], parent: null, props: {}, text: '', style: {}, addEventListener(name, handler) { this.props[`on${name}`] = handler }, removeEventListener() {}, getRootNode() { return root } } },
    createText(text) { return { type: 'text', children: [], parent: null, props: {}, text } },
    createComment(text) { return { type: 'comment', children: [], parent: null, props: {}, text } },
    setText(node, text) { node.text = text },
    setElementText(node, text) { node.children = []; node.text = text },
    parentNode(node) { return node.parent },
    nextSibling(node) { const index = node.parent?.children.indexOf(node) ?? -1; return index < 0 ? null : node.parent.children[index + 1] ?? null },
    querySelector() { return root },
    setScopeId() {},
    insertStaticContent(content, parent, anchor) { const node = { type: 'static', children: [], parent: null, props: {}, text: content }; this.insert(node, parent, anchor); return [node, node] },
  })
  const app = renderer.createApp(App)
  app.mount(root)
  return { root, unmount: () => app.unmount() }
}

function nodes(root) {
  const result = []
  function visit(node) { result.push(node); for (const child of node.children ?? []) visit(child) }
  visit(root)
  return result
}

function textContent(node) { if (node.type === 'comment') return ''; return `${node.text ?? ''}${(node.children ?? []).map(textContent).join('')}` }
function findButton(root, label) { const all = nodes(root); const button = all.find(node => node.type === 'button' && textContent(node).trim() === label); assert.ok(button, `button ${label} is rendered; found ${all.filter(node => node.type === 'button').map(textContent).join(', ')}`); return button }
function findNavButton(root, label) { const button = nodes(root).find(node => node.type === 'button' && textContent(node).trim().endsWith(label)); assert.ok(button, `navigation button ${label} is rendered`); return button }
function rendered(root, selector) { return nodes(root).some(node => node.props?.class?.split?.(' ').includes(selector)) }
async function settle() { for (let index = 0; index < 8; index++) { await Promise.resolve(); await nextTick() } }
async function settleUntil(predicate) { for (let index = 0; index < 20 && !predicate(); index++) { await new Promise(resolve => setImmediate(resolve)); await settle() } assert.equal(predicate(), true) }
async function click(root, label) { await findButton(root, label).props.onClick(); await settle() }
function deferred() { let resolve; const promise = new Promise(done => { resolve = done }); return { promise, resolve } }
function emitRunEvent(type = 'started', projectId = registeredProject.id, runId = 1) { globalThis.__orcRunEvent({ payload: { project_id: projectId, event: { type, event: { id: Date.now(), run_id: runId, task_id: 'T-1', kind: type, timestamp: '2026-01-01 00:00:00', payload: null } } } }) }
function setInput(root, index, value) { const input = nodes(root).filter(node => node.type === 'input')[index]; assert.ok(input); input.value = value; input.props.oninput({ target: input }) }
let sequence = Promise.resolve()
function serial(name, run) { test(name, async () => { const previous = sequence; let release; sequence = new Promise(resolve => { release = resolve }); await previous; try { await run() } finally { release() } }) }

serial('startup without an active project renders ProjectPicker and makes no project-scoped calls', async () => {
  const api = apiHarness()
  const app = mountApp()
  await settle()
  assert.equal(rendered(app.root, 'project-picker'), true)
  assert.equal(nodes(app.root).filter(node => node.type === 'nav')[0].children.filter(node => node.type === 'button').length, 0)
  assert.deepEqual(api.calls, ['currentProject', 'registeredProjects'])
  app.unmount()
})

for (const action of [{ button: 'IMPORT PROJECT', confirm: 'IMPORT', call: 'importProject' }, { button: 'INITIALIZE / ADOPT GIT REPOSITORY', confirm: 'INITIALIZE / ADOPT', call: 'adoptProject' }]) {
  serial(`${action.call} activates the registered project before workspace loading`, async () => {
    const api = apiHarness()
    const app = mountApp()
    await settle()
    await click(app.root, action.button)
    setInput(app.root, 0, '/repo')
    await click(app.root, action.confirm)
    assert.equal(rendered(app.root, 'project-picker'), false)
    assert.ok(rendered(app.root, 'dashboard'))
    assert.ok(api.calls.indexOf(`openProject:${registeredProject.id}`) < api.calls.indexOf('snapshot'))
    assert.deepEqual(api.calls.filter(call => ['snapshot', 'runsWorkspace', 'projectReport'].includes(call)), ['snapshot', 'runsWorkspace', 'projectReport'])
    app.unmount()
  })
}

serial('opening a registered project enters the workspace and closing returns to ProjectPicker', async () => {
  const api = apiHarness()
  const app = mountApp()
  await settle()
  await click(app.root, 'OPEN')
  await click(app.root, 'OPEN PROJECT')
  assert.ok(rendered(app.root, 'dashboard'))
  await click(app.root, 'CLOSE')
  await click(app.root, 'CLOSE PROJECT')
  assert.equal(rendered(app.root, 'project-picker'), true)
  assert.equal(api.calls.filter(call => call === 'closeProject').length, 1)
  app.unmount()
})

serial('activation failure remains in ProjectPicker without loading project-scoped APIs', async () => {
  const api = apiHarness({ failActivation: true })
  const app = mountApp()
  await settle()
  await click(app.root, 'OPEN')
  await click(app.root, 'OPEN PROJECT')
  assert.equal(rendered(app.root, 'project-picker'), true)
  assert.match(textContent(app.root), /could not be activated/i)
  assert.deepEqual(api.calls.filter(call => ['snapshot', 'runsWorkspace', 'projectReport'].includes(call)), [])
  app.unmount()
})

serial('an initial snapshot failure recovers the workspace after a successful retry', async () => {
  let snapshots = 0
  const api = apiHarness({
    current: registeredProject,
    snapshot: async () => {
      snapshots++
      if (snapshots === 1) throw new Error('initial snapshot unavailable')
      return { dashboard: { tasks: [], queue: {}, agents: [], running_agents: [], project_name: 'recovered-project', repository_path: '/repo', repository_available: true, capacity: { busy: [], agents: [], quota_reserve_percent: 0 }, recent_activity: [], outcome_trends: {} }, health: { task_counts: {}, unresolved_approvals: 0 } }
    },
  })
  const app = mountApp()
  await settle()
  assert.match(textContent(app.root), /Unable to load project state: initial snapshot unavailable/)
  assert.equal(rendered(app.root, 'dashboard'), false)
  await click(app.root, 'TRY AGAIN')
  assert.doesNotMatch(textContent(app.root), /Unable to load project state/)
  assert.equal(rendered(app.root, 'dashboard'), true)
  assert.match(textContent(app.root), /recovered-project/)
  assert.equal(api.calls.filter(call => call === 'snapshot').length, 2)
  app.unmount()
})

serial('snapshot refresh failure preserves the workspace and reports scoped feedback', async () => {
  let snapshots = 0
  const api = apiHarness({
    current: registeredProject,
    snapshot: async () => {
      snapshots++
      if (snapshots > 1) throw new Error('snapshot unavailable')
      return { dashboard: { tasks: [], queue: {}, agents: [], running_agents: [], project_name: 'last-good-project', repository_path: '/repo', repository_available: true, capacity: { busy: [], agents: [], quota_reserve_percent: 0 }, recent_activity: [], outcome_trends: {} }, health: { task_counts: {}, unresolved_approvals: 0 } }
    },
  })
  const app = mountApp()
  await settle()
  await findNavButton(app.root, 'Tasks').props.onClick()
  await settle()
  await assert.rejects(findButton(app.root, 'REFRESH ↻').props.onClick(), /snapshot unavailable/)
  await settle()
  assert.equal(rendered(app.root, 'tasks-workspace'), true)
  assert.equal(rendered(app.root, 'snapshot-refresh-error'), true)
  assert.match(textContent(app.root), /last-good-project/)
  assert.doesNotMatch(textContent(app.root), /Unable to load project state/)
  assert.equal(api.calls.filter(call => call === 'snapshot').length, 2)
  app.unmount()
})

serial('closing a project clears its scoped snapshot refresh error before rendering the picker', async () => {
  let snapshots = 0
  const api = apiHarness({
    current: registeredProject,
    snapshot: async () => {
      snapshots++
      if (snapshots > 1) throw new Error('previous project snapshot unavailable')
      return { dashboard: { tasks: [], queue: {}, agents: [], running_agents: [], project_name: 'first-project', repository_path: '/repo', repository_available: true, capacity: { busy: [], agents: [], quota_reserve_percent: 0 }, recent_activity: [], outcome_trends: {} }, health: { task_counts: {}, unresolved_approvals: 0 } }
    },
  })
  const app = mountApp()
  await settle()
  await findNavButton(app.root, 'Tasks').props.onClick()
  await settle()
  await assert.rejects(findButton(app.root, 'REFRESH ↻').props.onClick(), /previous project snapshot unavailable/)
  await settle()
  assert.equal(rendered(app.root, 'snapshot-refresh-error'), true)
  await click(app.root, 'CLOSE')
  await click(app.root, 'CLOSE PROJECT')
  assert.equal(rendered(app.root, 'project-picker'), true)
  assert.equal(rendered(app.root, 'snapshot-refresh-error'), false)
  assert.doesNotMatch(textContent(app.root), /previous project snapshot unavailable/)
  app.unmount()
})

serial('lifecycle event bursts queue at most one Runs refresh after the active refresh settles', async () => {
  const activeRefresh = deferred()
  let refreshes = 0
  const api = apiHarness({ current: registeredProject, runsWorkspace: async () => { refreshes++; if (refreshes === 2) return activeRefresh.promise; return { runs: [], details: [] } } })
  const app = mountApp()
  await settleUntil(() => api.calls.includes('projectReport'))
  await findNavButton(app.root, 'Runs').props.onClick()
  await settle()
  emitRunEvent('started')
  await settle()
  assert.equal(refreshes, 2)
  for (let index = 0; index < 8; index++) emitRunEvent('started')
  await settle()
  assert.equal(refreshes, 2)
  activeRefresh.resolve({ runs: [], details: [] })
  await settle()
  assert.equal(refreshes, 3)
  emitRunEvent('started', 'another-project')
  await settle()
  assert.equal(refreshes, 3)
  app.unmount()
})

serial('a Runs response settling after the project closes is ignored', async () => {
  const staleRefresh = deferred()
  let refreshes = 0
  const staleRun = { id: 99, task_id: null, agent: 'stale-agent', status: 'running', phase: 'work', output: null, error: null, execution_class: null, execution_mode: 'automated', started_at: '2026-01-01 00:00:00', finished_at: null, last_activity: '2026-01-01 00:00:00' }
  apiHarness({ current: registeredProject, runsWorkspace: async () => { refreshes++; if (refreshes === 2) return staleRefresh.promise; return { runs: [], details: [] } } })
  const app = mountApp()
  await settleUntil(() => typeof globalThis.__orcRunEvent === 'function')
  await findNavButton(app.root, 'Runs').props.onClick()
  emitRunEvent('started')
  await settle()
  assert.equal(refreshes, 2)
  await click(app.root, 'CLOSE')
  await click(app.root, 'CLOSE PROJECT')
  staleRefresh.resolve({ runs: [staleRun], details: [] })
  await settle()
  assert.equal(rendered(app.root, 'project-picker'), true)
  assert.doesNotMatch(textContent(app.root), /stale-agent/)
  app.unmount()
})

serial('complete worker activity remains lazy and cached while worker events update it without refetching', async () => {
  const run = { id: 1, task_id: null, agent: 'coder', status: 'running', phase: 'work', output: null, error: null, execution_class: null, execution_mode: 'automated', started_at: '2026-01-01 00:00:00', finished_at: null, last_activity: '2026-01-01 00:00:00' }
  const detail = { run, result: null, validation: null, activity: [] }
  const api = apiHarness({ current: registeredProject, runsWorkspace: async () => ({ runs: [run], details: [detail] }), runDetails: async () => detail, workerLog: async () => [{ id: 1, run_id: 1, task_id: null, kind: 'worker_output', timestamp: '2026-01-01 00:00:00', payload: 'first' }] })
  const app = mountApp()
  await settleUntil(() => typeof globalThis.__orcRunEvent === 'function')
  assert.equal(api.calls.filter(call => call.startsWith('workerLog:')).length, 0)
  emitRunEvent('worker_output')
  await settle()
  assert.equal(api.calls.filter(call => call.startsWith('workerLog:')).length, 0)
  await findNavButton(app.root, 'Runs').props.onClick()
  await settle()
  const activity = nodes(app.root).find(node => node.type === 'details')
  assert.ok(activity)
  await activity.props.onToggle({ target: { open: true } })
  await settle()
  await activity.props.onToggle({ target: { open: false } })
  await activity.props.onToggle({ target: { open: true } })
  await settle()
  assert.equal(api.calls.filter(call => call === 'workerLog:1').length, 1)
  app.unmount()
})

test('shared UI styles define control, form, and action contracts', async () => {
  const foundation = await readFile(join(process.cwd(), 'src/ui-foundation.css'), 'utf8')
  assert.match(foundation, /--ui-control-height:40px/)
  assert.match(foundation, /input,select,textarea\{[^}]*width:100%[^}]*min-height:var\(--ui-control-height\)[^}]*box-sizing:border-box/)
  assert.match(foundation, /textarea\{min-height:104px;resize:vertical\}/)
  assert.match(foundation, /form:not\(\.lead-composer\),\.ui-form\{display:grid;gap:var\(--ui-form-gap\)/)
  assert.match(foundation, /\.ui-button,\.text-button,\.apply-button,\.reject-button,\.action-chip\{min-height:var\(--ui-hit-area\)/)
})
