use std::{
    process::{Child, Command},
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{
    menu::{CheckMenuItem, CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, RunEvent, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

const SERVICE_URL: &str = "http://127.0.0.1:3080";
const SERVICE_HOST: &str = "127.0.0.1";
const SERVICE_PORT: u16 = 3080;
const WINDOW_LABEL: &str = "main";
const TRAY_ID: &str = "main-tray";

/// Windows: 以无控制台窗口的方式启动子进程，避免弹出黑色控制台窗口。
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 持有 dsh web 子进程，应用退出时统一回收。
struct ServiceProcess(Mutex<Option<Child>>);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // 单例：二次启动时不新开进程，而是唤起已有实例的主窗口
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main_window(app);
        }))
        // 开机自启（托盘菜单提供开关）
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![]),
        ))
        // 点击关闭按钮仅隐藏到托盘，应用继续在后台运行
        .on_window_event(|window, event| {
            if window.label() == WINDOW_LABEL {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(|app| {
            match spawn_service() {
                Ok(child) => {
                    app.manage(ServiceProcess(Mutex::new(Some(child))));
                }
                Err(err) => {
                    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
                        set_status(&window, &err.to_string());
                    }
                    return Ok(());
                }
            }

            build_tray(app)?;

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if wait_for_service_ready(Duration::from_secs(60)).await {
                    if let Some(window) = handle.get_webview_window(WINDOW_LABEL) {
                        if let Ok(url) = tauri::Url::parse(SERVICE_URL) {
                            let _ = window.navigate(url);
                        }
                    }
                } else if let Some(window) = handle.get_webview_window(WINDOW_LABEL) {
                    set_status(
                        &window,
                        "服务启动超时，请确认本机已安装 Node.js / npx 后重试",
                    );
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let RunEvent::Exit = event {
                if let Some(state) = app_handle.try_state::<ServiceProcess>() {
                    if let Some(mut child) = state.0.lock().unwrap().take() {
                        kill_service_tree(&mut child);
                    }
                }
            }
        });
}

/// 构建系统托盘：左键单击显示主窗口，右键菜单提供「开机自启 / 显示主窗口 / 退出」。
fn build_tray<R: tauri::Runtime>(app: &tauri::App<R>) -> tauri::Result<()> {
    let autostart_item = CheckMenuItemBuilder::with_id("autostart", "开机自启")
        .checked(app.autolaunch().is_enabled().unwrap_or(false))
        .build(app)?;
    let show_item = MenuItemBuilder::with_id("show", "显示主窗口").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "退出").build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&autostart_item)
        .item(&show_item)
        .item(&quit_item)
        .build()?;

    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray-icon.png"))?;
    let autostart_handle = autostart_item.clone();

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("dsh-desktop")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().0.as_str() {
            "show" => show_main_window(app),
            "autostart" => toggle_autostart(app, &autostart_handle),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

/// 切换开机自启状态并同步托盘菜单勾选状态。
fn toggle_autostart<R: tauri::Runtime>(app: &tauri::AppHandle<R>, item: &CheckMenuItem<R>) {
    let launcher = app.autolaunch();
    match launcher.is_enabled() {
        Ok(true) => {
            if launcher.disable().is_ok() {
                let _ = item.set_checked(false);
            }
        }
        Ok(false) => {
            if launcher.enable().is_ok() {
                let _ = item.set_checked(true);
            }
        }
        Err(err) => eprintln!("query autostart state failed: {err}"),
    }
}

fn show_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn set_status<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>, message: &str) {
    let msg = serde_json::to_string(message).unwrap_or_else(|_| "\"unknown\"".into());
    let _ = window.eval(&format!(
        "var el = document.getElementById('status'); if (el) el.textContent = {};",
        msg
    ));
}

async fn wait_for_service_ready(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if tokio::net::TcpStream::connect((SERVICE_HOST, SERVICE_PORT))
            .await
            .is_ok()
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

fn spawn_service() -> std::io::Result<Child> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        Command::new("cmd")
            .args(["/C", "npx --yes @deepseek-ai/dsh web"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    }

    #[cfg(not(target_os = "windows"))]
    {
        Command::new("sh")
            .args(["-c", "npx --yes @deepseek-ai/dsh web"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    }
}

fn kill_service_tree(child: &mut Child) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let pid = child.id().to_string();
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", pid.as_str()])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        let _ = child.wait();
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}
