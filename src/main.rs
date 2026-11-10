//! Desktop entry point: local API + app window + tray icon.
#![windows_subsystem = "windows"]

mod engine;
mod server;
mod ytdl;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use wry::WebViewBuilder;

use server::PORT;

enum UserEvent {
    Show,
    Quit,
}

fn http(request: &str) -> Option<String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], PORT));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(1)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    s.write_all(request.as_bytes()).ok()?;
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    Some(out)
}

fn already_running() -> bool {
    http(&format!("GET /api/ping HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\nConnection: close\r\n\r\n"))
        .is_some_and(|r| r.contains("download-manager"))
}

fn ask_existing_to_show() {
    http(&format!("POST /api/show HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\nOrigin: http://127.0.0.1:{PORT}\r\n\
                   Content-Length: 0\r\nConnection: close\r\n\r\n"));
}

/// Green disc with a white download arrow, drawn at any size.
fn icon_rgba(size: u32) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let k = 64.0 / size as f32;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = ((x as f32 + 0.5) * k, (y as f32 + 0.5) * k);
            let d = ((fx - 32.0).powi(2) + (fy - 32.0).powi(2)).sqrt();
            if d > 30.0 { continue; }
            let white = (28.0..36.0).contains(&fx) && (12.0..36.0).contains(&fy)
                || (32.0..48.0).contains(&fy) && (fx - 32.0).abs() <= 48.0 - fy
                || (16.0..48.0).contains(&fx) && (50.0..54.0).contains(&fy);
            let c = if white { [255, 255, 255, 255] } else { [63, 174, 73, 255] };
            let i = ((y * size + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&c);
        }
    }
    px
}

fn main() {
    let minimized = std::env::args().any(|a| a == "--minimized");
    if already_running() {
        if !minimized { ask_existing_to_show(); }
        return;
    }
    let listener = match std::net::TcpListener::bind(("127.0.0.1", PORT)) {
        Ok(l) => l,
        Err(_) => {
            rfd::MessageDialog::new().set_title("Download Manager").set_level(rfd::MessageLevel::Error)
                .set_description(format!("Port {PORT} is in use by another program.")).show();
            return;
        }
    };
    listener.set_nonblocking(true).expect("nonblocking");

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("runtime");
    let manager = {
        let _g = rt.enter();
        engine::Manager::new()
    };

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let (p1, p2) = (event_loop.create_proxy(), event_loop.create_proxy());
    rt.spawn(server::serve(listener, server::AppState {
        manager: manager.clone(),
        on_show: Arc::new(move || { let _ = p1.send_event(UserEvent::Show); }),
        on_quit: Arc::new(move || { let _ = p2.send_event(UserEvent::Quit); }),
    }));

    let window = WindowBuilder::new()
        .with_title("Download Manager")
        .with_inner_size(LogicalSize::new(1150.0, 720.0))
        .with_min_inner_size(LogicalSize::new(760.0, 480.0))
        .with_window_icon(tao::window::Icon::from_rgba(icon_rgba(48), 48, 48).ok())
        .with_visible(!minimized)
        .build(&event_loop)
        .expect("window");
    // WebView2 keeps its profile next to the exe by default, which is read-only under Program Files.
    std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", engine::app_dir().join("webview2"));
    let builder = WebViewBuilder::new().with_url(format!("http://127.0.0.1:{PORT}/"));
    #[cfg(not(target_os = "linux"))]
    let _webview = builder.build(&window).expect("webview");
    // GTK needs the webview packed into the window's box (also the only way that works on Wayland).
    #[cfg(target_os = "linux")]
    let _webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        builder.build_gtk(window.default_vbox().expect("vbox")).expect("webview")
    };

    let menu = Menu::new();
    let show_item = MenuItem::new("Göster", true, None);
    let quit_item = MenuItem::new("Çıkış", true, None);
    let _ = menu.append_items(&[&show_item, &quit_item]);
    let (show_id, quit_id) = (show_item.id().clone(), quit_item.id().clone());
    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("Download Manager")
        .with_icon(tray_icon::Icon::from_rgba(icon_rgba(32), 32, 32).expect("icon"))
        .build()
        .expect("tray");
    let p = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == show_id { let _ = p.send_event(UserEvent::Show); }
        if e.id == quit_id { let _ = p.send_event(UserEvent::Quit); }
    }));
    let p = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        let open = matches!(e, TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
            | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. });
        if open { let _ = p.send_event(UserEvent::Show); }
    }));

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            // Like IDM: closing the window keeps the app in the tray so browser downloads are still caught.
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => window.set_visible(false),
            Event::UserEvent(UserEvent::Show) => {
                window.set_visible(true);
                window.set_minimized(false);
                window.set_focus();
            }
            Event::UserEvent(UserEvent::Quit) => {
                manager.stop_all();
                std::thread::sleep(Duration::from_millis(400)); // let workers flush their buffers
                manager.save();
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}
