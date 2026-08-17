mod service;

use std::{sync::Mutex, time::Duration};
use tauri::{
    menu::{CheckMenuItem, CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, RunEvent, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

const WINDOW_LABEL: &str = "main";
const TRAY_ID: &str = "main-tray";

/// 持有 dsh web 子进程，应用退出时统一回收。
struct ServiceProcess(Mutex<Option<service::SpawnedService>>);

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
        .invoke_handler(tauri::generate_handler![start_service, detect_modes])
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
            // 持有服务子进程（初始为空），待前端选择页调用 start_service 后拉起
            app.manage(ServiceProcess(Mutex::new(None)));
            build_tray(app)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let RunEvent::Exit = event {
                if let Some(state) = app_handle.try_state::<ServiceProcess>() {
                    if let Some(mut spawned) = state.0.lock().unwrap().take() {
                        service::kill_tree(&mut spawned);
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

/// 供启动选择页检测各启动模式的可用性。
#[tauri::command]
fn detect_modes() -> service::ModeAvailability {
    service::detect()
}

/// 按所选模式拉起 dsh 服务，就绪后将窗口导航到服务地址。
#[tauri::command]
async fn start_service(app: tauri::AppHandle, mode: service::LaunchMode) -> Result<(), String> {
    let state = app.state::<ServiceProcess>();
    if state.0.lock().unwrap().is_some() {
        return Err("服务已在运行".into());
    }

    // 先登记子进程再等待就绪，等待期间退出应用也能回收
    let spawned = service::spawn(mode).map_err(|e| e.to_string())?;
    *state.0.lock().unwrap() = Some(spawned);

    if service::wait_for_service_ready(Duration::from_secs(60)).await {
        let url = tauri::Url::parse(service::SERVICE_URL).map_err(|e| e.to_string())?;
        if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
            let _ = window.navigate(url);
        }
        Ok(())
    } else {
        if let Some(mut spawned) = state.0.lock().unwrap().take() {
            service::kill_tree(&mut spawned);
        }
        Err(service::timeout_hint(mode).into())
    }
}
