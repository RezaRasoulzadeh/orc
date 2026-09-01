import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'

const packageMetadata = JSON.parse(readFileSync('package.json', 'utf8'))
const packageLock = JSON.parse(readFileSync('package-lock.json', 'utf8'))
const tauriConfiguration = JSON.parse(readFileSync(join('src-tauri', 'tauri.conf.json'), 'utf8'))
const cargoVersion = packageVersion('Cargo.toml')
const tauriCargoVersion = packageVersion(join('src-tauri', 'Cargo.toml'))
const expectedVersion = packageMetadata.version
const versions = {
  'Cargo.toml': cargoVersion,
  'src-tauri/Cargo.toml': tauriCargoVersion,
  'package-lock.json': packageLock.version,
  'package-lock.json root package': packageLock.packages?.['']?.version,
  'src-tauri/tauri.conf.json': tauriConfiguration.version,
}
for (const [source, version] of Object.entries(versions)) {
  if (version !== expectedVersion) throw new Error(`release version mismatch: package.json=${expectedVersion}, ${source}=${version ?? 'missing'}`)
}

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
const versionedPackages = packages.filter(path => String(path).includes(expectedVersion))
if (versionedPackages.length === 0) throw new Error(`no Orc ${expectedVersion} package found under ${bundle}`)
console.log(`validated Orc ${expectedVersion} metadata, desktop executable, and ${versionedPackages.length} versioned Tauri package artifact(s)`)

function packageVersion(path) {
  const manifest = readFileSync(path, 'utf8')
  const packageSection = manifest.split(/\n(?=\[)/).find(section => section.startsWith('[package]'))
  return packageSection?.match(/^version\s*=\s*"([^"]+)"\s*$/m)?.[1]
}
