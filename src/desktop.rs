use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopLaunch {
    pub executable: PathBuf,
    pub detached: bool,
    pub null_stdio: bool,
}

pub trait DesktopProcessLauncher {
    fn launch(&self, launch: &DesktopLaunch) -> Result<()>;
}

pub struct DetachedDesktopProcess;

impl DesktopProcessLauncher for DetachedDesktopProcess {
    fn launch(&self, launch: &DesktopLaunch) -> Result<()> {
        let mut command = Command::new(&launch.executable);
        if launch.null_stdio {
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
        }
        configure_detachment(&mut command, launch.detached);
        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to launch desktop executable {}",
                launch.executable.display()
            )
        })?;
        std::thread::Builder::new()
            .name("orc-desktop-reaper".to_owned())
            .spawn(move || {
                let _ = child.wait();
            })
            .context("failed to start desktop process reaper")?;
        Ok(())
    }
}

#[cfg(unix)]
fn configure_detachment(command: &mut Command, detached: bool) {
    use std::os::unix::process::CommandExt;
    if detached {
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

#[cfg(windows)]
fn configure_detachment(command: &mut Command, detached: bool) {
    use std::os::windows::process::CommandExt;
    if detached {
        command.creation_flags(0x0000_0008 | 0x0000_0200);
    }
}

pub fn desktop_executable_path(current_exe: &Path) -> Result<PathBuf> {
    let candidates = desktop_executable_candidates(current_exe);
    resolve_desktop_executable(&candidates)
}

fn resolve_desktop_executable(candidates: &[PathBuf]) -> Result<PathBuf> {
    candidates
        .iter()
        .find(|candidate| candidate.is_file())
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Orc desktop is not installed. Install the desktop package and try again. Searched: {}",
                candidates.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ")
            )
        })
}

pub fn launch_desktop<L: DesktopProcessLauncher>(launcher: &L) -> Result<()> {
    let current_exe =
        std::env::current_exe().context("failed to determine the installed Orc CLI path")?;
    let desktop = desktop_executable_path(&current_exe)?;
    launcher.launch(&DesktopLaunch {
        executable: desktop,
        detached: true,
        null_stdio: true,
    })
}

fn desktop_executable_candidates(current_exe: &Path) -> Vec<PathBuf> {
    let name = if cfg!(windows) {
        "orc-desktop.exe"
    } else {
        "orc-desktop"
    };
    let mut candidates = vec![
        current_exe
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(name),
    ];

    if cfg!(target_os = "linux") {
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(PathBuf::from(home).join(".local/lib/orc").join(name));
        }
        candidates.push(PathBuf::from("/usr/local/lib/orc").join(name));
        candidates.push(PathBuf::from("/usr/lib/orc").join(name));
    } else if cfg!(target_os = "macos") {
        candidates.push(PathBuf::from("/Applications/Orc.app/Contents/MacOS").join(name));
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(
                PathBuf::from(home)
                    .join("Applications/Orc.app/Contents/MacOS")
                    .join(name),
            );
        }
    } else if cfg!(windows)
        && let Some(local_app_data) = std::env::var_os("LOCALAPPDATA")
    {
        candidates.push(
            PathBuf::from(local_app_data)
                .join("Programs/Orc")
                .join(name),
        );
    }

    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};
    #[cfg(unix)]
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    struct FakeLauncher(Arc<Mutex<Vec<DesktopLaunch>>>);

    impl DesktopProcessLauncher for FakeLauncher {
        fn launch(&self, launch: &DesktopLaunch) -> Result<()> {
            self.0.lock().unwrap().push(launch.clone());
            Ok(())
        }
    }

    #[test]
    fn resolves_desktop_next_to_cli() {
        let directory = tempdir().unwrap();
        let cli = directory
            .path()
            .join(if cfg!(windows) { "orc.exe" } else { "orc" });
        let desktop = directory.path().join(if cfg!(windows) {
            "orc-desktop.exe"
        } else {
            "orc-desktop"
        });
        std::fs::write(&desktop, b"desktop").unwrap();
        assert_eq!(desktop_executable_path(&cli).unwrap(), desktop);
    }

    #[test]
    fn missing_desktop_error_is_actionable() {
        let error = resolve_desktop_executable(&[PathBuf::from("/missing/orc-desktop")])
            .unwrap_err()
            .to_string();
        assert!(error.contains("desktop is not installed"));
        assert!(error.contains("Install the desktop package"));
    }

    #[test]
    fn launch_uses_resolved_executable_without_opening_gui() {
        let directory = tempdir().unwrap();
        let cli = directory.path().join("orc");
        let desktop = directory.path().join("orc-desktop");
        std::fs::write(&desktop, b"desktop").unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let launcher = FakeLauncher(calls.clone());
        let resolved = desktop_executable_path(&cli).unwrap();
        launcher
            .launch(&DesktopLaunch {
                executable: resolved,
                detached: true,
                null_stdio: true,
            })
            .unwrap();
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[DesktopLaunch {
                executable: desktop,
                detached: true,
                null_stdio: true,
            }]
        );
    }

    #[cfg(unix)]
    #[test]
    fn detached_launcher_creates_a_new_session() {
        let directory = tempdir().unwrap();
        let result = directory.path().join("session");
        let executable = directory.path().join("desktop-test");
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\nps -o pid= -o sid= -p $$ > '{}'\nsleep 0.5\n",
                result.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();

        let started = Instant::now();
        DetachedDesktopProcess
            .launch(&DesktopLaunch {
                executable,
                detached: true,
                null_stdio: true,
            })
            .unwrap();
        assert!(started.elapsed() < Duration::from_millis(200));

        let deadline = Instant::now() + Duration::from_secs(2);
        let fields = loop {
            if let Ok(values) = std::fs::read_to_string(&result) {
                let fields = values
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                if fields.len() == 2 {
                    break fields;
                }
            }
            assert!(
                Instant::now() < deadline,
                "detached process did not write session data"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0], fields[1]);
    }
}
