import { spawnSync } from 'node:child_process'

const bundles = process.platform === 'linux' ? 'deb,rpm' : process.platform === 'darwin' ? 'app,dmg' : 'msi,nsis'
const command = process.platform === 'win32' ? 'npx.cmd' : 'npx'
const result = spawnSync(command, ['tauri', 'build', '--bundles', bundles], { stdio: 'inherit' })
if (result.error) throw result.error
if (result.status !== 0) process.exit(result.status ?? 1)
