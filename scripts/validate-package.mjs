import { existsSync, readdirSync } from 'node:fs'
import { join } from 'node:path'

const release = join('src-tauri', 'target', 'release')
const desktop = join(release, process.platform === 'win32' ? 'orc-desktop.exe' : 'orc-desktop')
if (!existsSync(desktop)) throw new Error(`missing packaged desktop executable: ${desktop}`)

const bundle = join(release, 'bundle')
if (!existsSync(bundle)) throw new Error(`missing Tauri bundle directory: ${bundle}`)
const packages = readdirSync(bundle, { recursive: true }).filter(path => {
  const value = String(path)
  return /\.(appimage|deb|rpm|dmg|msi|exe)$/i.test(value) || value.endsWith('.app')
})
if (packages.length === 0) throw new Error(`no installable Tauri package found under ${bundle}`)
console.log(`validated desktop executable and ${packages.length} Tauri package artifact(s)`)
