//! Code editors a project or worktree can be opened in ("Open in VS Code").
//!
//! On macOS editors are found by their app bundle and opened with `open -a`, so they work
//! without their shell command installed. Elsewhere they're found on the login PATH
//! (Visual Studio through vswhere).

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::env::{find_command, login_env, no_window};

struct EditorDef {
    id: &'static str,
    name: &'static str,
    /// macOS app bundle names, first found wins.
    mac_apps: &'static [&'static str],
    /// Commands that open a folder (`code <dir>`), first found wins.
    commands: &'static [&'static str],
}

const EDITORS: &[EditorDef] = &[
    EditorDef {
        id: "vscode",
        name: "VS Code",
        mac_apps: &["Visual Studio Code"],
        commands: &["code"],
    },
    EditorDef {
        id: "vscode-insiders",
        name: "VS Code Insiders",
        mac_apps: &["Visual Studio Code - Insiders"],
        commands: &["code-insiders"],
    },
    EditorDef {
        id: "cursor",
        name: "Cursor",
        mac_apps: &["Cursor"],
        commands: &["cursor"],
    },
    EditorDef {
        id: "windsurf",
        name: "Windsurf",
        mac_apps: &["Windsurf"],
        commands: &["windsurf"],
    },
    EditorDef {
        id: "zed",
        name: "Zed",
        mac_apps: &["Zed"],
        commands: &["zed", "zeditor"],
    },
    EditorDef {
        id: "zed-preview",
        name: "Zed Preview",
        mac_apps: &["Zed Preview"],
        commands: &[],
    },
    EditorDef {
        id: "vscodium",
        name: "VSCodium",
        mac_apps: &["VSCodium"],
        commands: &["codium"],
    },
    EditorDef {
        id: "kiro",
        name: "Kiro",
        mac_apps: &["Kiro"],
        commands: &["kiro"],
    },
    // Found through vswhere (see `visual_studio`), Windows only.
    EditorDef {
        id: "visualstudio",
        name: "Visual Studio",
        mac_apps: &[],
        commands: &[],
    },
    EditorDef {
        id: "sublime",
        name: "Sublime Text",
        mac_apps: &["Sublime Text"],
        commands: &["subl"],
    },
    EditorDef {
        id: "intellij",
        name: "IntelliJ IDEA",
        mac_apps: &[
            "IntelliJ IDEA",
            "IntelliJ IDEA Ultimate",
            "IntelliJ IDEA CE",
        ],
        commands: &["idea"],
    },
    EditorDef {
        id: "webstorm",
        name: "WebStorm",
        mac_apps: &["WebStorm"],
        commands: &["webstorm"],
    },
    EditorDef {
        id: "pycharm",
        name: "PyCharm",
        mac_apps: &[
            "PyCharm",
            "PyCharm Professional Edition",
            "PyCharm CE",
            "PyCharm Community Edition",
        ],
        commands: &["pycharm"],
    },
    EditorDef {
        id: "goland",
        name: "GoLand",
        mac_apps: &["GoLand"],
        commands: &["goland"],
    },
    EditorDef {
        id: "rustrover",
        name: "RustRover",
        mac_apps: &["RustRover"],
        commands: &["rustrover"],
    },
    EditorDef {
        id: "clion",
        name: "CLion",
        mac_apps: &["CLion"],
        commands: &["clion"],
    },
    EditorDef {
        id: "phpstorm",
        name: "PhpStorm",
        mac_apps: &["PhpStorm"],
        commands: &["phpstorm"],
    },
    EditorDef {
        id: "rider",
        name: "Rider",
        mac_apps: &["Rider"],
        commands: &["rider"],
    },
    EditorDef {
        id: "android-studio",
        name: "Android Studio",
        mac_apps: &["Android Studio"],
        commands: &["studio"],
    },
    EditorDef {
        id: "xcode",
        name: "Xcode",
        mac_apps: &["Xcode"],
        commands: &[],
    },
    EditorDef {
        id: "nova",
        name: "Nova",
        mac_apps: &["Nova"],
        commands: &[],
    },
    EditorDef {
        id: "bbedit",
        name: "BBEdit",
        mac_apps: &["BBEdit"],
        commands: &["bbedit"],
    },
];

#[derive(Debug, Clone, Serialize)]
pub struct Editor {
    pub id: &'static str,
    pub name: &'static str,
}

/// How to start an editor on this machine.
enum Launch {
    /// `open -a <app> <dir>` (macOS).
    App(PathBuf),
    /// `<program> <dir>`.
    Program(PathBuf),
}

fn find(def: &EditorDef) -> Option<Launch> {
    if cfg!(target_os = "macos") {
        let home = dirs::home_dir().unwrap_or_default();
        let roots = [
            PathBuf::from("/Applications"),
            home.join("Applications"),
            PathBuf::from("/System/Applications"),
        ];
        // JetBrains Toolbox keeps its apps one level deeper.
        let toolbox = home.join("Applications/JetBrains Toolbox");
        return def
            .mac_apps
            .iter()
            .flat_map(|app| {
                roots
                    .iter()
                    .chain([&toolbox])
                    .map(move |root| root.join(format!("{app}.app")))
            })
            .find(|path| path.exists())
            .map(Launch::App);
    }
    if def.id == "visualstudio" {
        return visual_studio().map(Launch::Program);
    }
    def.commands
        .iter()
        .find_map(|c| find_command(c))
        .map(Launch::Program)
}

/// devenv.exe of the newest Visual Studio, asked from the installer's vswhere.
fn visual_studio() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let base = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from)?;
    let vswhere = base.join("Microsoft Visual Studio/Installer/vswhere.exe");
    if !vswhere.exists() {
        return None;
    }
    let out = no_window(Command::new(vswhere).args(["-latest", "-property", "productPath"]))
        .output()
        .ok()?;
    let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    path.exists().then_some(path)
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => dirs::home_dir()
            .unwrap_or_default()
            .join(rest.trim_start_matches(['/', '\\'])),
        _ => PathBuf::from(path),
    }
}

fn open(id: &str, dir: &Path) -> Result<(), String> {
    let def = EDITORS
        .iter()
        .find(|e| e.id == id)
        .ok_or_else(|| format!("unknown editor {id}"))?;
    if !dir.is_dir() {
        return Err(format!("{} doesn't exist anymore", dir.display()));
    }
    match find(def).ok_or_else(|| format!("{} isn't installed", def.name))? {
        Launch::App(app) => {
            let out = Command::new("open")
                .arg("-a")
                .arg(&app)
                .arg(dir)
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(format!(
                    "couldn't open {}: {}",
                    def.name,
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
        }
        Launch::Program(program) => {
            let mut child = no_window(
                Command::new(&program)
                    .arg(dir)
                    .current_dir(dir)
                    .envs(login_env()),
            )
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("couldn't start {}: {e}", def.name))?;
            // Some launchers (JetBrains scripts) keep running; reap them when they exit.
            std::thread::spawn(move || child.wait());
        }
    }
    Ok(())
}

/// Editors installed on this machine, in the order they're offered.
#[tauri::command]
pub async fn editors_list() -> Vec<Editor> {
    let list = || {
        EDITORS
            .iter()
            .filter(|def| find(def).is_some())
            .map(|def| Editor {
                id: def.id,
                name: def.name,
            })
            .collect()
    };
    tauri::async_runtime::spawn_blocking(list)
        .await
        .unwrap_or_default()
}

/// Opens `path` (a project or worktree folder) in the editor `id`.
#[tauri::command]
pub async fn editor_open(id: String, path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || open(&id, &expand_home(&path)))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<_> = EDITORS.iter().map(|e| e.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), EDITORS.len());
    }

    #[test]
    fn expands_home() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("~/code/app"), home.join("code/app"));
        assert_eq!(expand_home("/tmp/~x"), PathBuf::from("/tmp/~x"));
    }

    #[test]
    fn rejects_unknown_editor() {
        assert!(open("nope", Path::new("/"))
            .unwrap_err()
            .contains("unknown editor"));
    }
}
