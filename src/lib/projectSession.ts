import type { RegisteredProject } from './api'

export interface ProjectRuntimeApi {
  currentProject: () => Promise<RegisteredProject | null>
  openProject: (id: string) => Promise<void>
  closeProject: () => Promise<void>
  importProject: (root: string, displayName?: string) => Promise<RegisteredProject>
  adoptProject: (root: string, displayName?: string) => Promise<RegisteredProject>
}

export interface ProjectRuntimeHooks {
  resetWorkspace: () => void
  setActiveProject: (project: RegisteredProject | null) => void
  loadWorkspace: () => Promise<void>
}

export class ProjectRuntime {
  private active: RegisteredProject | null = null
  private readonly api: ProjectRuntimeApi
  private readonly hooks: ProjectRuntimeHooks

  constructor(api: ProjectRuntimeApi, hooks: ProjectRuntimeHooks) {
    this.api = api
    this.hooks = hooks
  }

  get activeProject() { return this.active }
  get view() { return this.active ? 'workspace' : 'picker' }

  async start() {
    const project = await this.api.currentProject()
    if (!project) return this.enterPicker()
    await this.enterWorkspace(project)
    return project
  }

  async importProject(root: string, displayName?: string) {
    return this.registerAndOpen(() => this.api.importProject(root, displayName))
  }

  async adoptProject(root: string, displayName?: string) {
    return this.registerAndOpen(() => this.api.adoptProject(root, displayName))
  }

  async openProject(id: string) {
    return this.activate(id, 'The project could not be activated.')
  }

  async closeProject() {
    await this.api.closeProject()
    return this.enterPicker()
  }

  private async registerAndOpen(register: () => Promise<RegisteredProject>) {
    const project = await register()
    return this.activate(project.id, 'The project was registered but could not be activated.')
  }

  private async activate(id: string, message: string) {
    this.enterPicker()
    try {
      await this.api.openProject(id)
      const project = await this.api.currentProject()
      if (!project || project.id !== id) throw new Error(message)
      await this.enterWorkspace(project)
      return project
    } catch (error) {
      this.enterPicker()
      throw error
    }
  }

  private async enterWorkspace(project: RegisteredProject) {
    this.active = project
    this.hooks.setActiveProject(project)
    await this.hooks.loadWorkspace()
  }

  private enterPicker() {
    this.active = null
    this.hooks.resetWorkspace()
    this.hooks.setActiveProject(null)
    return null
  }
}
