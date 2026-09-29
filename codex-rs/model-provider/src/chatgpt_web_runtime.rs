use std::env;
use std::fs;
use std::io;
use std::io::Read;
use std::io::Write;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::TcpStream;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
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

use serde_json::Value;

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

    pub(crate) async fn logout_account(&self) -> io::Result<()> {
        self.ensure_ready().await?;
        let token = read_control_token()?;
        tokio::task::spawn_blocking(move || post_control_request("/admin/account/logout", &token))
            .await
            .map_err(io::Error::other)?
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

pub(crate) fn chatgpt_web_login_state_exists() -> bool {
    runtime_home()
        .as_deref()
        .is_some_and(chatgpt_web_login_state_exists_in)
}

fn chatgpt_web_login_state_exists_in(home: &Path) -> bool {
    let Ok(config) = read_runtime_config_in(home) else {
        return false;
    };
    let Some(storage_path) = config
        .get("storageStatePath")
        .and_then(Value::as_str)
        .and_then(expand_user_path)
    else {
        return false;
    };
    if !storage_path.is_file() {
        return false;
    }
    let marker_path = PathBuf::from(format!("{}.verified.json", storage_path.display()));
    let Ok(marker_text) = fs::read_to_string(marker_path) else {
        return false;
    };
    let Ok(marker) = serde_json::from_str::<Value>(&marker_text) else {
        return false;
    };
    marker.get("version").and_then(Value::as_u64) == Some(1)
        && marker.get("authenticated").and_then(Value::as_bool) == Some(true)
        && marker
            .get("verifiedAt")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
}

fn runtime_home() -> Option<PathBuf> {
    non_empty_env_path("CODEX_CHATGPT_WEB_HOME").or_else(default_runtime_home)
}

fn read_runtime_config() -> io::Result<Value> {
    let home = runtime_home().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not resolve ChatGPT Web runtime home",
        )
    })?;
    read_runtime_config_in(&home)
}

fn read_runtime_config_in(home: &Path) -> io::Result<Value> {
    let contents = fs::read_to_string(home.join("config.json"))?;
    let contents = contents.strip_prefix('\u{feff}').unwrap_or(&contents);
    serde_json::from_str(contents).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid ChatGPT Web runtime config: {error}"),
        )
    })
}

fn read_control_token() -> io::Result<String> {
    let config = read_runtime_config()?;
    let token = config
        .get("controlToken")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| {
            value.len() >= 40
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "ChatGPT Web control token is missing or invalid",
            )
        })?;
    Ok(token.to_owned())
}

fn expand_user_path(value: &str) -> Option<PathBuf> {
    if value == "~" {
        return user_home();
    }
    if let Some(rest) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        return user_home().map(|home| home.join(rest));
    }
    Some(PathBuf::from(value))
}

fn user_home() -> Option<PathBuf> {
    if cfg!(windows) {
        non_empty_env_path("USERPROFILE")
    } else {
        non_empty_env_path("HOME")
    }
}

fn post_control_request(path: &str, token: &str) -> io::Result<()> {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), CHATGPT_WEB_PORT);
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{CHATGPT_WEB_PORT}\r\nAuthorization: Bearer {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes())?;
    stream.flush()?;

    let mut response = [0_u8; 512];
    let read = stream.read(&mut response)?;
    let status_line = std::str::from_utf8(&response[..read])
        .ok()
        .and_then(|text| text.lines().next())
        .unwrap_or_default();
    if status_line.starts_with("HTTP/1.1 200 ") || status_line.starts_with("HTTP/1.0 200 ") {
        return Ok(());
    }
    Err(io::Error::other(
        "ChatGPT Web runtime rejected account logout",
    ))
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
fn launcher_command(launcher: &Path) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let mut command = Command::new("cmd.exe");
        command
            .creation_flags(CREATE_NO_WINDOW)
            .args(["/d", "/c"])
            .raw_arg(format!("call \"{}\" serve", launcher.display()));
        command
    }

    #[cfg(not(windows))]
    {
        let mut command = Command::new(launcher);
        command.arg("serve");
        command
    }
}

fn spawn_launcher(launcher: &Path) -> io::Result<Child> {
    let mut command = launcher_command(launcher);

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
