//! Windows system tray icon (`Shell_NotifyIconW`) with a hidden message-only window.
//!
//! Runs a dedicated Win32 message-loop thread so tray clicks and context menu
//! interactions stay responsive even while the egui window is hidden or minimized.
//! On non-Windows platforms this module compiles as a clean no-op stub.

/// User actions triggered from the system tray icon or its popup menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum TrayAction {
    Show,
    ToggleShare,
    Quit,
}

#[cfg(windows)]
mod imp {
    use super::TrayAction;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
        NOTIFYICONDATAW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
        DispatchMessageW, GetCursorPos, GetMessageW, GetWindowLongPtrW, LoadIconW, PostMessageW,
        PostQuitMessage, RegisterClassW, SetForegroundWindow, SetWindowLongPtrW, TrackPopupMenu,
        TranslateMessage, UnregisterClassW, CREATESTRUCTW, GWLP_USERDATA, HWND_MESSAGE,
        IDI_APPLICATION, MF_SEPARATOR, MF_STRING, MSG, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
        TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
        WM_CANCELMODE, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP,
        WM_NCCREATE, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
    };

    const TRAY_ICON_ID: u32 = 1;
    const WM_TRAY_CALLBACK: u32 = WM_APP + 1;
    const WM_TRAY_UPDATE_TIP: u32 = WM_APP + 2;

    const CMD_SHOW: usize = 1001;
    const CMD_TOGGLE_SHARE: usize = 1002;
    const CMD_QUIT: usize = 1003;

    static NEXT_CLASS_ID: AtomicUsize = AtomicUsize::new(1);

    struct TrayState {
        tx: mpsc::Sender<TrayAction>,
        ctx: eframe::egui::Context,
        tooltip: Arc<Mutex<String>>,
        icon_added: bool,
    }

    pub struct TrayHandle {
        rx: mpsc::Receiver<TrayAction>,
        hwnd: isize,
        tooltip: Arc<Mutex<String>>,
        thread: Option<JoinHandle<()>>,
    }

    impl TrayHandle {
        pub fn spawn(ctx: eframe::egui::Context) -> Option<Self> {
            let (tx, rx) = mpsc::channel();
            let (ready_tx, ready_rx) = mpsc::sync_channel::<Option<isize>>(1);
            let tooltip = Arc::new(Mutex::new(String::from("RemoteView")));
            let thread_tooltip = Arc::clone(&tooltip);

            let thread = thread::Builder::new()
                .name("rv-tray".into())
                .spawn(move || {
                    run_tray_thread(tx, ctx, thread_tooltip, ready_tx);
                })
                .ok()?;

            match ready_rx.recv().ok().flatten() {
                Some(hwnd) => Some(Self {
                    rx,
                    hwnd,
                    tooltip,
                    thread: Some(thread),
                }),
                None => {
                    let _ = thread.join();
                    None
                }
            }
        }

        pub fn try_recv(&self) -> Option<TrayAction> {
            self.rx.try_recv().ok()
        }

        pub fn set_tooltip(&self, text: &str) {
            if let Ok(mut guard) = self.tooltip.lock() {
                guard.clear();
                guard.push_str(text);
            }
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(self.hwnd as *mut c_void)),
                    WM_TRAY_UPDATE_TIP,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }

    impl Drop for TrayHandle {
        fn drop(&mut self) {
            let hwnd = HWND(self.hwnd as *mut c_void);
            unsafe {
                // Dismiss any open popup menu so WM_CLOSE is processed promptly.
                let _ = PostMessageW(Some(hwnd), WM_CANCELMODE, WPARAM(0), LPARAM(0));
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn run_tray_thread(
        tx: mpsc::Sender<TrayAction>,
        ctx: eframe::egui::Context,
        tooltip: Arc<Mutex<String>>,
        ready_tx: mpsc::SyncSender<Option<isize>>,
    ) {
        let class_id = NEXT_CLASS_ID.fetch_add(1, Ordering::Relaxed);
        let class_name = encode_wide(&format!("RemoteViewTrayWindow_{class_id}"));
        let window_name = encode_wide("RemoteViewTray");

        let hinstance: Option<HINSTANCE> = unsafe { GetModuleHandleW(None) }.ok().map(Into::into);

        let wc = WNDCLASSW {
            lpfnWndProc: Some(tray_wnd_proc),
            hInstance: hinstance.unwrap_or_default(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };

        let atom = unsafe { RegisterClassW(&wc) };
        if atom == 0 {
            let _ = ready_tx.send(None);
            return;
        }

        let state_ptr = Box::into_raw(Box::new(TrayState {
            tx,
            ctx,
            tooltip: Arc::clone(&tooltip),
            icon_added: false,
        }));

        let hwnd = match unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                PCWSTR(class_name.as_ptr()),
                PCWSTR(window_name.as_ptr()),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                hinstance,
                Some(state_ptr as *const c_void),
            )
        } {
            Ok(h) => h,
            Err(_) => {
                unsafe {
                    drop(Box::from_raw(state_ptr));
                    let _ = UnregisterClassW(PCWSTR(class_name.as_ptr()), hinstance);
                }
                let _ = ready_tx.send(None);
                return;
            }
        };

        // Load embedded PE icon resource ID 1 (MAKEINTRESOURCEW(1)), falling back to the standard application icon.
        #[allow(clippy::manual_dangling_ptr)]
        let hicon = unsafe {
            LoadIconW(hinstance, PCWSTR(1 as *const u16))
                .or_else(|_| LoadIconW(None, IDI_APPLICATION))
                .unwrap_or_default()
        };

        let mut nid = base_nid(hwnd);
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY_CALLBACK;
        nid.hIcon = hicon;
        if let Ok(tip) = tooltip.lock() {
            write_tip(&mut nid.szTip, &tip);
        }

        let added = unsafe { Shell_NotifyIconW(NIM_ADD, &nid) }.as_bool();
        if !added {
            unsafe {
                let _ = DestroyWindow(hwnd);
                drop(Box::from_raw(state_ptr));
                let _ = UnregisterClassW(PCWSTR(class_name.as_ptr()), hinstance);
            }
            let _ = ready_tx.send(None);
            return;
        }

        unsafe {
            (*state_ptr).icon_added = true;
        }

        if ready_tx.send(Some(hwnd.0 as isize)).is_err() {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        } else {
            let mut msg = MSG::default();
            loop {
                let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
                if ret.0 <= 0 {
                    break;
                }
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }

        // Ensure the tray icon is always removed even if the loop exited abnormally.
        unsafe {
            if (*state_ptr).icon_added {
                let del_nid = base_nid(hwnd);
                let _ = Shell_NotifyIconW(NIM_DELETE, &del_nid);
                (*state_ptr).icon_added = false;
            }
            drop(Box::from_raw(state_ptr));
            let _ = UnregisterClassW(PCWSTR(class_name.as_ptr()), hinstance);
        }
    }

    unsafe extern "system" fn tray_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_NCCREATE {
            let cs = lparam.0 as *const CREATESTRUCTW;
            if !cs.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*cs).lpCreateParams as isize);
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }

        let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayState;

        match msg {
            WM_TRAY_CALLBACK => {
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    let event = (lparam.0 as usize & 0xFFFF) as u32;
                    match event {
                        WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                            emit_action(state, TrayAction::Show);
                        }
                        WM_RBUTTONUP | WM_CONTEXTMENU => {
                            show_context_menu(hwnd, state);
                        }
                        _ => {}
                    }
                }
                LRESULT(0)
            }
            WM_TRAY_UPDATE_TIP => {
                if !state_ptr.is_null() {
                    let state = &*state_ptr;
                    if state.icon_added {
                        let mut nid = base_nid(hwnd);
                        nid.uFlags = NIF_TIP;
                        if let Ok(tip) = state.tooltip.lock() {
                            write_tip(&mut nid.szTip, &tip);
                        }
                        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
                    }
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    if state.icon_added {
                        let nid = base_nid(hwnd);
                        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
                        state.icon_added = false;
                    }
                }
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe fn show_context_menu(hwnd: HWND, state: &TrayState) {
        let Ok(hmenu) = CreatePopupMenu() else {
            return;
        };

        let lbl_show = encode_wide("Mở RemoteView");
        let lbl_toggle = encode_wide("Bắt đầu / Dừng chia sẻ");
        let lbl_quit = encode_wide("Thoát");

        let _ = AppendMenuW(hmenu, MF_STRING, CMD_SHOW, PCWSTR(lbl_show.as_ptr()));
        let _ = AppendMenuW(
            hmenu,
            MF_STRING,
            CMD_TOGGLE_SHARE,
            PCWSTR(lbl_toggle.as_ptr()),
        );
        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(hmenu, MF_STRING, CMD_QUIT, PCWSTR(lbl_quit.as_ptr()));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);

        // Required by Win32 so clicking outside the popup menu dismisses it reliably.
        let _ = SetForegroundWindow(hwnd);

        let selected = TrackPopupMenu(
            hmenu,
            TPM_BOTTOMALIGN | TPM_LEFTALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );

        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(hmenu);

        match selected.0 as usize {
            CMD_SHOW => emit_action(state, TrayAction::Show),
            CMD_TOGGLE_SHARE => emit_action(state, TrayAction::ToggleShare),
            CMD_QUIT => emit_action(state, TrayAction::Quit),
            _ => {}
        }
    }

    fn emit_action(state: &TrayState, action: TrayAction) {
        let _ = state.tx.send(action);
        state.ctx.request_repaint();
    }

    fn base_nid(hwnd: HWND) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ICON_ID,
            ..Default::default()
        }
    }

    fn encode_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn write_tip(dst: &mut [u16; 128], text: &str) {
        dst.fill(0);
        let mut units: Vec<u16> = text.encode_utf16().take(127).collect();
        // Avoid truncating in the middle of a UTF-16 surrogate pair.
        if let Some(&last) = units.last() {
            if (0xD800..=0xDBFF).contains(&last) {
                units.pop();
            }
        }
        dst[..units.len()].copy_from_slice(&units);
    }
}

#[cfg(not(windows))]
mod imp {
    use super::TrayAction;

    pub struct TrayHandle {
        _priv: (),
    }

    impl TrayHandle {
        pub fn spawn(_ctx: eframe::egui::Context) -> Option<Self> {
            None
        }

        pub fn try_recv(&self) -> Option<TrayAction> {
            None
        }

        pub fn set_tooltip(&self, _text: &str) {}
    }
}

pub use imp::TrayHandle;
