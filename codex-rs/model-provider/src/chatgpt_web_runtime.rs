use std::env;
use std::fs;
use std::io;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::TcpStream;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use tokio::sync::Mutex as AsyncMutex;
use tokio::time::sleep;

const CHATGPT_WEB_PORT: u16 = 17_841;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const PROBE_TIMEOUT: Duration = Duration::from_millis(150);
const PROBE_INTERVAL: Duration = Duration::from_millis(100);

static PROCESS_RUNTIME: OnceLock<Arc<ChatGptWebRuntime>> = OnceLock::new();

pub(crate) fn process_chatgpt_web_runtime() -> Arc<ChatGptWebRuntime> {
    PROCESS_RUNTIME
        .get_or_init(|| Arc::new(ChatGptWebRuntime::new()))
        .clone()
}

#[derive(Debug)]
pub(crate) struct ChatGptWebRuntime {
    startup: AsyncMutex<()>,
    child: Mutex<Option<Child>>,
}
impl ChatGptWebRuntime {
    fn new() -> Self {
        Self {
            startup: AsyncMutex::new(()),
            child: Mutex::new(None),
        }
    }

    pub(crate) async fn ensure_ready(&self) -> io::Result<()> {
        if endpoint_ready() {
            return Ok(());
        }

        let _startup = self.startup.lock().await;
        if endpoint_ready() {
            return Ok(());
        }

        self.clear_exited_child()?;
        if self.has_managed_child()? {
            return self.wait_until_ready().await;
        }

        let launcher = locate_launcher()?;
        let child = spawn_launcher(&launcher)?;
        *self.child.lock().map_err(poisoned_mutex)? = Some(child);
        self.wait_until_ready().await
    }

    fn clear_exited_child(&self) -> io::Result<()> {
        let mut child = self.child.lock().map_err(poisoned_mutex)?;
        if child
            .as_mut()
            .is_some_and(|child| child.try_wait().ok().flatten().is_some())
        {
            *child = None;
        }
        Ok(())
    }

    fn has_managed_child(&self) -> io::Result<bool> {
        Ok(self.child.lock().map_err(poisoned_mutex)?.is_some())
    }

    async fn wait_until_ready(&self) -> io::Result<()> {
        let started = Instant::now();
        while started.elapsed() < STARTUP_TIMEOUT {
            if endpoint_ready() {
                return Ok(());
            }
            if let Some(status) = self.managed_child_exit_status()? {
                return Err(io::Error::other(format!(
                    "ChatGPT Web runtime exited during startup with status {status}"
                )));
            }
            sleep(PROBE_INTERVAL).await;
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "ChatGPT Web runtime did not become ready on 127.0.0.1:17841",
        ))
    }
    fn managed_child_exit_status(&self) -> io::Result<Option<std::process::ExitStatus>> {
        let mut child = self.child.lock().map_err(poisoned_mutex)?;
        child
            .as_mut()
            .map(Child::try_wait)
            .transpose()
            .map(Option::flatten)
    }
}

fn endpoint_ready() -> bool {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), CHATGPT_WEB_PORT);
    TcpStream::connect_timeout(&address, PROBE_TIMEOUT).is_ok()
}

fn locate_launcher() -> io::Result<PathBuf> {
    if let Some(path) = non_empty_env_path("CODEX_CHATGPT_WEB_LAUNCHER") {
        return validate_launcher(path);
    }

    let home = non_empty_env_path("CODEX_CHATGPT_WEB_HOME")
        .or_else(default_runtime_home)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "could not resolve the ChatGPT Web runtime home",
            )
        })?;

    latest_launcher_in(&home).ok_or_else(|| io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "ChatGPT Web runtime is not installed under {}. Install or bundle codex-chatgpt-web first",
            home.display()
        ),
    ))
}

fn non_empty_env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn default_runtime_home() -> Option<PathBuf> {
    let home = if cfg!(windows) {
        non_empty_env_path("USERPROFILE")
    } else {
        non_empty_env_path("HOME")
    }?;
    Some(home.join(".codex-chatgpt-web"))
}
fn latest_launcher_in(home: &Path) -> Option<PathBuf> {
    let versions = home.join("versions");
    let mut version_dirs = fs::read_dir(versions)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().ok().is_some_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();

    version_dirs.sort_by(|left, right| runtime_version(right).cmp(&runtime_version(left)));
    let launcher_name = if cfg!(windows) {
        "codex-chatgpt-web.cmd"
    } else {
        "codex-chatgpt-web"
    };
    version_dirs
        .into_iter()
        .map(|version| version.join("bin").join(launcher_name))
        .find(|candidate| candidate.is_file())
}

fn runtime_version(path: &Path) -> Vec<u64> {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.split('-').next())
        .map(|version| {
            version
                .split('.')
                .map(|part| part.parse::<u64>().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default()
}

fn validate_launcher(path: PathBuf) -> io::Result<PathBuf> {
    path.is_file().then_some(path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "CODEX_CHATGPT_WEB_LAUNCHER does not point to a file",
        )
    })
}
fn spawn_launcher(launcher: &Path) -> io::Result<Child> {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("cmd.exe");
        command
            .arg("/d")
            .arg("/c")
            .arg(format!("call \"{}\" serve", launcher.display()));
        command
    } else {
        let mut command = Command::new(launcher);
        command.arg("serve");
        command
    };

    command
        .env("CODEX_CHATGPT_WEB_NATIVE", "1")
        .env(
            "CODEX_CHATGPT_WEB_PARENT_PID",
            std::process::id().to_string(),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

fn poisoned_mutex<T>(_: std::sync::PoisonError<T>) -> io::Error {
    io::Error::other("ChatGPT Web runtime process lock was poisoned")
}

#[cfg(test)]
#[path = "chatgpt_web_runtime_tests.rs"]
mod tests;
