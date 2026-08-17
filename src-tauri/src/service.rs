//! dsh web 服务的拉起、就绪探测与回收。
//!
//! Windows 下支持两种启动模式：
//! - 本机（Native）：直接 `cmd /C npx ...`，需本机安装 Node.js；
//! - WSL：经 `wsl bash -il` 交互式 login shell 会话在默认发行版内执行同一命令。
//!   交互式会话保证 mise / nvm / asdf / fnm 等经 shell 配置注入的
//!   Node 环境生效，并校验 npx 为发行版本地安装（避免经 Windows
//!   interop 静默回退到 Windows 的 npx）。
//!   WSL2 会把发行版内监听的 `127.0.0.1` 端口转发到 Windows 本机，
//!   因此服务地址（[`SERVICE_URL`]）在两种模式下保持不变。
//!
//! 启动前由前端选择页调用 [`detect`] 获取各模式可用性，
//! 用户选定后经 [`spawn`] 按所选模式拉起服务。

use std::process::{Child, Command, Stdio};

/// dsh web 服务地址。WSL 模式下同样适用（WSL2 localhost 转发）。
pub const SERVICE_URL: &str = "http://127.0.0.1:3080";
const SERVICE_HOST: &str = "127.0.0.1";
const SERVICE_PORT: u16 = 3080;

/// Windows：以无控制台窗口的方式启动子进程，避免弹出黑色控制台窗口。
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// WSL 环境探测命令：以交互式 login shell（`bash -il`）会话执行，
/// mise / nvm / asdf / fnm 等经 shell 配置注入的 Node 环境才会生效。
/// 输出用 `__PROBE__` 标记包裹，便于从终端转义序列中解析。
#[cfg(target_os = "windows")]
const WSL_PROBE_CMD: &str =
    "printf '__PROBE__%s__END__\\n' \"$(command -v npx 2>/dev/null || true)\"\nexit\n";

/// WSL 启动脚本：先记录前台进程组（供退出时整组回收），
/// 再校验 npx 为发行版本地安装（排除经 Windows interop
/// 回退到 `/mnt/` 下 Windows 版 npx 的情况），拉起 dsh 服务。
#[cfg(target_os = "windows")]
const WSL_LAUNCH_SCRIPT: &str = r#"echo $$ > /tmp/dsh-desktop.pid
NPX="$(command -v npx 2>/dev/null || true)"
if [ -z "$NPX" ]; then
  echo "WSL 发行版内未安装 Node.js（npx）" >&2
  exit 1
fi
case "$NPX" in
  /mnt/*)
    echo "WSL 发行版内未安装 Node.js，检测到的 npx 来自 Windows" >&2
    exit 1
    ;;
esac
exec "$NPX" --yes @deepseek-ai/dsh web
"#;

/// WSL 回收脚本：按启动时记录的进程组结束整组进程（npx → sh → node），
/// 并以 pkill 兜底（防 pidfile 丢失）。无需交互环境，用 `-lc` 执行。
#[cfg(target_os = "windows")]
const WSL_KILL_SCRIPT: &str = r#"if [ -f /tmp/dsh-desktop.pid ]; then
  PGID="$(cat /tmp/dsh-desktop.pid 2>/dev/null || true)"
  if [ -n "$PGID" ] && kill -0 -- "-$PGID" 2>/dev/null; then
    kill -TERM -- "-$PGID" 2>/dev/null || true
    sleep 1
    kill -KILL -- "-$PGID" 2>/dev/null || true
  fi
  rm -f /tmp/dsh-desktop.pid
fi
pkill -f '@deepseek-ai/dsh' 2>/dev/null
pkill -f '\.bin/dsh' 2>/dev/null
true
"#;

/// dsh 服务的启动模式（前端选择页通过 Tauri 命令传入）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LaunchMode {
    /// 本机 Node.js（`cmd /C npx ...`）。
    Native,
    /// 经 WSL 默认发行版执行（`wsl bash -lc ...`）。
    Wsl,
}

/// 各启动模式的可用性（供选择页渲染）。
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct ModeAvailability {
    /// 本机是否可用（`PATH` 中存在 npx）。
    pub native: bool,
    /// 是否可用 WSL 启动（存在已安装的发行版）。
    pub wsl: bool,
}

/// 已拉起的 dsh web 子进程及其启动模式，退出时统一回收。
pub struct SpawnedService {
    pub child: Child,
    pub mode: LaunchMode,
}

/// 检测各启动模式的可用性。
pub fn detect() -> ModeAvailability {
    #[cfg(target_os = "windows")]
    {
        ModeAvailability {
            native: has_command("npx"),
            wsl: wsl_npx_path().is_some(),
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        ModeAvailability {
            native: true,
            wsl: false,
        }
    }
}

/// 按所选模式拉起 dsh web 服务子进程。
pub fn spawn(mode: LaunchMode) -> std::io::Result<SpawnedService> {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = match mode {
            LaunchMode::Native => {
                let mut c = Command::new("cmd");
                c.args(["/C", "npx --yes @deepseek-ai/dsh web"]);
                c
            }
            LaunchMode::Wsl => {
                if wsl_npx_path().is_none() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "WSL 发行版内未检测到可用的 Node.js（npx），请先在发行版内安装 Node.js 后重试",
                    ));
                }
                let mut c = Command::new("wsl");
                c.args(["bash", "-il"]);
                c
            }
        };
        cmd.creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // WSL 模式经 stdin 管道喂命令，其余模式丢弃 stdin
        cmd.stdin(if mode == LaunchMode::Wsl {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut child = cmd.spawn()?;
        if mode == LaunchMode::Wsl {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(WSL_LAUNCH_SCRIPT.as_bytes());
            }
        }
        Ok(SpawnedService { child, mode })
    }

    #[cfg(not(target_os = "windows"))]
    {
        let child = Command::new("sh")
            .args(["-c", "npx --yes @deepseek-ai/dsh web"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(SpawnedService { child, mode })
    }
}

/// 检查 `PATH` 中是否存在指定命令。
#[cfg(target_os = "windows")]
fn has_command(name: &str) -> bool {
    Command::new("cmd")
        .args(["/C", "where", name])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// 探测 WSL 发行版内的 npx 绝对路径。
///
/// 以交互式 login shell（`bash -il`）会话执行，mise / nvm / asdf / fnm
/// 等经 shell 配置注入的 Node 环境才会生效；返回 `None` 表示发行版内
/// 没有可用的 Node（包括经 Windows interop 回退到 `/mnt/` 的情况）。
#[cfg(target_os = "windows")]
fn wsl_npx_path() -> Option<String> {
    let mut child = Command::new("wsl")
        .args(["bash", "-il"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    {
        use std::io::Write;
        let _ = child.stdin.as_mut()?.write_all(WSL_PROBE_CMD.as_bytes());
    }
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    // 从终端转义序列（OSC 等）中解析出标记包裹的路径
    let text = String::from_utf8_lossy(&output.stdout);
    let start = text.find("__PROBE__")? + "__PROBE__".len();
    let end = text[start..].find("__END__")?;
    let path = text[start..start + end].trim();
    if path.is_empty() || path.starts_with("/mnt/") {
        return None;
    }
    Some(path.to_string())
}

/// 回收 dsh 服务：结束子进程（Windows 下连同进程树）。
pub fn kill_tree(spawned: &mut SpawnedService) {
    #[cfg(target_os = "windows")]
    {
        if spawned.mode == LaunchMode::Wsl {
            // WSL 内的进程树不随 Windows 侧 taskkill 回收：
            // 按启动时记录的进程组（pidfile）结束整组进程，并用 pkill 兜底。
            let _ = Command::new("wsl")
                .args(["bash", "-lc", WSL_KILL_SCRIPT])
                .creation_flags(CREATE_NO_WINDOW)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .output();
        }
        let pid = spawned.child.id().to_string();
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", pid.as_str()])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        let _ = spawned.child.wait();
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = spawned.child.kill();
        let _ = spawned.child.wait();
    }
}

/// 轮询服务端口直至就绪，超时返回 false。
pub async fn wait_for_service_ready(timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if tokio::net::TcpStream::connect((SERVICE_HOST, SERVICE_PORT))
            .await
            .is_ok()
        {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    false
}

/// 服务启动超时时的用户提示。
pub fn timeout_hint(mode: LaunchMode) -> &'static str {
    if mode == LaunchMode::Wsl {
        "服务启动超时，请确认 WSL 发行版内已安装 Node.js（npx）后重试"
    } else {
        "服务启动超时，请确认本机已安装 Node.js / npx 后重试"
    }
}
