//! Open In: launching another IDE on the current project and file.

use ide_command::desktop_command;
use ide_core::{IdeError, IdeResult};
use ide_host::ops::blocking;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Default Windows install locations of the VS Code family. `Code.exe` and friends
/// are usually not on PATH (the installer only adds `code.cmd`), so a bare name
/// needs these fallbacks before we can launch the app.
#[cfg(windows)]
fn windows_install(executable: &str) -> Vec<PathBuf> {
    let (dir, file): (&str, &str) = match executable.to_ascii_lowercase().as_str() {
        "code.exe" => ("Microsoft VS Code", "Code.exe"),
        "cursor.exe" => ("Cursor", "Cursor.exe"),
        "vscodium.exe" => ("VSCodium", "VSCodium.exe"),
        _ => return Vec::new(),
    };
    ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)"]
        .iter()
        .filter_map(std::env::var_os)
        .flat_map(|base| {
            let base = PathBuf::from(base);
            [base.join("Programs").join(dir).join(file), base.join(dir).join(file)]
        })
        .filter(|path| path.is_file())
        .collect()
}

/// macOS launchers: JetBrains Toolbox writes bare scripts under
/// `~/Library/Application Support/JetBrains/Toolbox/scripts`, and the VS Code
/// family ships its CLI inside the `.app` bundle rather than on PATH.
#[cfg(target_os = "macos")]
fn macos_install(executable: &str) -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut paths = Vec::new();

    if let Some(home) = &home {
        paths.push(home.join("Library/Application Support/JetBrains/Toolbox/scripts").join(executable));
    }

    let app_cli = match executable {
        "code" => Some("Visual Studio Code.app/Contents/Resources/app/bin/code"),
        "cursor" => Some("Cursor.app/Contents/Resources/app/bin/cursor"),
        "codium" => Some("VSCodium.app/Contents/Resources/app/bin/codium"),
        _ => None,
    };
    if let Some(rel) = app_cli {
        paths.push(PathBuf::from("/Applications").join(rel));
        if let Some(home) = &home {
            paths.push(home.join("Applications").join(rel));
        }
    }

    for base in ["/usr/local/bin", "/opt/homebrew/bin", "/usr/bin"] {
        paths.push(PathBuf::from(base).join(executable));
    }
    if let Some(home) = &home {
        paths.push(home.join("bin").join(executable));
        paths.push(home.join("Applications/JetBrains Toolbox").join(executable));
    }
    paths.push(PathBuf::from("/Applications/JetBrains Toolbox").join(executable));

    paths.retain(|path| path.is_file());
    paths
}

/// Executables to try: a configured path is used as is; a bare name checks the
/// platform's known install locations before PATH (whose `Code.exe` may be a
/// Microsoft Store alias, and whose macOS Toolbox scripts may not be exported).
fn candidates(executable: &str) -> Vec<OsString> {
    let exe = executable.trim();
    if exe.is_empty() {
        return Vec::new();
    }
    if exe.contains('/') || exe.contains('\\') {
        return vec![OsString::from(exe)];
    }
    let mut out = Vec::new();
    #[cfg(windows)]
    out.extend(windows_install(exe).into_iter().map(PathBuf::into_os_string));
    #[cfg(target_os = "macos")]
    out.extend(macos_install(exe).into_iter().map(PathBuf::into_os_string));
    out.push(OsString::from(exe));
    #[cfg(not(windows))]
    if let Some(name) = exe.strip_suffix(".exe") {
        if !name.is_empty() {
            out.push(OsString::from(name));
        }
    }
    out
}

/// IPC paths arrive with `/` separators on every OS. JetBrains' Windows launcher
/// rejects a drive path written that way as an invalid directory, so restore
/// native separators for arguments that look like absolute Windows paths.
fn native_arg(arg: &str) -> String {
    #[cfg(windows)]
    {
        let b = arg.as_bytes();
        if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/' {
            return arg.replace('/', "\\");
        }
    }
    #[cfg(not(windows))]
    let _ = arg;
    arg.to_owned()
}

/// Launches `executable` with `args`, detached from the IDE. The executable is a
/// user-chosen path from the Jump To settings (or a known app name resolved from
/// PATH); arguments never go through a shell.
#[desktop_command(trust = trusted)]
#[tauri::command]
#[specta::specta]
pub async fn jump_to_open(executable: String, args: Vec<String>) -> IdeResult<()> {
    if executable.trim().is_empty() {
        return Err(IdeError::invalid("no executable configured"));
    }
    blocking(move || {
        let args: Vec<String> = args.iter().map(|arg| native_arg(arg)).collect();
        let mut last_error = None;
        for candidate in candidates(&executable) {
            let mut cmd = Command::new(&candidate);
            cmd.args(&args);
            ide_core::process::prepare(&mut cmd);
            cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            match cmd.spawn() {
                Ok(mut child) => {
                    std::thread::spawn(move || {
                        let _ = child.wait();
                    });
                    return Ok(());
                }
                Err(e) => last_error = Some(e),
            }
        }
        Err(IdeError::unavailable(format!(
            "cannot launch {}: {}",
            executable,
            last_error.map_or_else(|| "no executable found".into(), |e| e.to_string())
        )))
    })
    .await
}

ide_command::desktop_domain!(jump_to_open);

#[cfg(test)]
mod tests {
    use super::{candidates, native_arg};
    use std::ffi::OsString;

    #[test]
    fn explicit_paths_are_used_verbatim() {
        let path = if cfg!(windows) { r"C:\Program Files\Code.exe" } else { "/usr/bin/code" };
        assert_eq!(candidates(path), vec![OsString::from(path)]);
    }

    #[test]
    fn bare_names_are_kept_as_a_candidate() {
        assert!(candidates("code.exe").contains(&OsString::from("code.exe")));
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_falls_back_to_the_unsuffixed_command() {
        assert!(candidates("Code.exe").contains(&OsString::from("Code")));
    }

    #[test]
    fn ipc_paths_get_native_separators_on_windows() {
        let native = native_arg("F:/projects/openidea/AGENTS.md");
        #[cfg(windows)]
        assert_eq!(native, r"F:\projects\openidea\AGENTS.md");
        #[cfg(not(windows))]
        assert_eq!(native, "F:/projects/openidea/AGENTS.md");
        assert_eq!(native_arg("--line"), "--line");
    }
}
