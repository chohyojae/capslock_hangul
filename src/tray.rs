//! 시스템 트레이(알림 영역) 아이콘 + 우클릭 메뉴 + 정보 다이얼로그.
//!
//! 상주 중인 본체가 작업 표시줄 알림 영역에 아이콘을 등록하고, 다음을 제공한다:
//! - **우클릭** → 컨텍스트 메뉴 `Info`(단축키 I) / `Exit`(단축키 X).
//! - **Info 클릭** 또는 **아이콘 더블클릭** → 정보 다이얼로그(프로그램 이름·버전·라이선스 고지문·
//!   GitHub 저장소 하이퍼링크·닫기 버튼). 링크 클릭 시 시스템 기본 브라우저로 저장소를 연다.
//! - **Exit 클릭** → 트레이 아이콘을 제거하고 메시지 루프를 종료(graceful shutdown)한다.
//!
//! ## 설계 요점
//! - 트레이 콜백 메시지를 받을 **숨김 창**(`CapsHangulTrayWindow`)을 메인 스레드에 만든다.
//!   `Shell_NotifyIcon(NIM_ADD)` 의 `uCallbackMessage` 로 마우스 이벤트를 이 창이 받는다.
//! - 메뉴 라벨의 `&` 니모닉(`&Info`, `E&xit`)으로 단축키 I/X 를 등록한다(Windows 표준 방식).
//! - 정보 다이얼로그는 **모드리스 창**으로 만들고, 메인 메시지 루프(`win32::run_message_loop`)가
//!   [`active_dialog`] 로 핸들을 얻어 `IsDialogMessageW` 에 위임한다 → Tab/Enter/Esc/기본 버튼이
//!   동작한다(별도 모달 루프 없이 키보드 훅·오버레이·트레이가 그대로 계속 동작).
//! - 하이퍼링크는 공용 컨트롤(SysLink, comctl32 v6 manifest 필요) 대신 **`STATIC`(SS_NOTIFY)**
//!   로 구현한다: 밑줄 폰트 + 파란 글자색(WM_CTLCOLORSTATIC) + 손 커서(WM_SETCURSOR), 클릭은
//!   `STN_CLICKED`(WM_COMMAND). manifest 없는 디버그/릴리스 모두에서 동일하게 동작한다.
//! - HiDPI: 프로세스가 Per-Monitor-V2 인식이므로(`win32::set_dpi_aware`), 대상 모니터 DPI 에
//!   맞춰 창/컨트롤/폰트 크기를 스케일하고 `AdjustWindowRectExForDpi` 로 외곽 크기를 보정한다.

use core::ffi::c_void;
use std::mem::size_of;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use windows_sys::Win32::Foundation::{COLORREF, GetLastError, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateFontW, DeleteObject, GetMonitorInfoW, GetSysColorBrush, MonitorFromPoint, SetBkMode,
    SetTextColor, COLOR_WINDOW, MONITORINFO, MONITOR_DEFAULTTONEAREST, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, ShellExecuteW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu,
    DestroyWindow, GetCursorPos, GetSystemMetrics, LoadCursorW, LoadIconW, LoadImageW,
    PostMessageW, PostQuitMessage, RegisterClassW, SendMessageW, SetCursor, SetForegroundWindow,
    ShowWindow, TrackPopupMenu, HICON, ICON_BIG, ICON_SMALL, IDC_ARROW,
    IDC_HAND, IMAGE_ICON, LR_DEFAULTCOLOR, MF_STRING, SM_CXSMICON, SM_CYSMICON, STM_SETICON,
    SW_SHOW, SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CLOSE, WM_COMMAND,
    WM_CTLCOLORSTATIC, WM_DESTROY, WM_LBUTTONDBLCLK, WM_NCDESTROY, WM_NULL, WM_RBUTTONUP,
    WM_SETCURSOR, WM_SETFONT, WM_SETICON, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_EX_DLGMODALFRAME,
    WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

// ── 트레이/메뉴/컨트롤 식별자 ────────────────────────────────────────────────
/// 트레이 아이콘이 마우스 이벤트를 보낼 콜백 메시지(이 창 전용 → 충돌 없음).
const WM_TRAY: u32 = WM_APP + 0x20;
/// 트레이 아이콘 ID(창당 1개).
const TRAY_UID: u32 = 1;
/// 컨텍스트 메뉴 명령 ID.
const ID_INFO: u32 = 1001;
const ID_EXIT: u32 = 1002;
/// 다이얼로그 컨트롤 ID. 닫기 버튼은 IDCANCEL(=2)로 둬 Esc 도 동일 처리되게 한다.
const ID_LINK: u32 = 101;
const ID_CLOSE: u32 = 2;

// ── 컨트롤 스타일(ABI 안정 값) ───────────────────────────────────────────────
// windows-sys 에서 SS_* 는 Win32_System_SystemServices 피처에 있어, 피처 추가를 피하려고
// 변하지 않는 Win32 ABI 값을 직접 정의한다(BS_DEFPUSHBUTTON 도 동일 맥락으로 명시).
const SS_ICON: u32 = 0x0000_0003;
const SS_NOTIFY: u32 = 0x0000_0100;
const BS_DEFPUSHBUTTON: u32 = 0x0000_0001;

// ── 표시 문자열 ──────────────────────────────────────────────────────────────
const PROGRAM_NAME: &str = "caps-hangul-rs";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO_URL: &str = "https://github.com/chohyojae/capslock_hangul";

// ── 전역 상태(메인 스레드 전용이지만 wnd_proc 공유 위해 atomic) ───────────────
static TRAY_HWND: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static TRAY_ICON: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static TRAY_ADDED: AtomicBool = AtomicBool::new(false);

/// 열려 있는 정보 다이얼로그 핸들(없으면 null). 메시지 루프가 `IsDialogMessageW` 위임에 쓴다.
static INFO_DLG: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
/// 다이얼로그 안 GitHub 링크(STATIC) 핸들. WM_CTLCOLORSTATIC/WM_SETCURSOR 비교용.
static LINK_HWND: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
/// 다이얼로그 폰트/아이콘(닫을 때 WM_NCDESTROY 에서 해제).
static DLG_FONT_TITLE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static DLG_FONT_TEXT: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static DLG_FONT_LINK: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static DLG_ICON: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// UTF-8 → 널 종료 UTF-16.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// COLORREF(0x00BBGGRR) 생성.
#[inline]
fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

/// 고정 길이 wide 배열(szTip 등)에 널 종료까지 복사한다(초과분은 잘림).
fn set_sz(dst: &mut [u16], s: &str) {
    let mut i = 0;
    for c in s.encode_utf16() {
        if i + 1 >= dst.len() {
            break; // 마지막 한 칸은 널 종료용으로 남긴다.
        }
        dst[i] = c;
        i += 1;
    }
    if i < dst.len() {
        dst[i] = 0;
    }
}

/// 트레이 아이콘과 숨김 창을 만든다. 메시지 루프 진입 전에 한 번 호출한다.
///
/// 실패해도 치명적이지 않으며(트레이 없이 동작), 이 경우 `Err(code)` 를 돌려준다.
pub fn init() -> Result<(), u32> {
    let class_tray = wide("CapsHangulTrayWindow");
    let class_dlg = wide("CapsHangulInfoDialog");

    // SAFETY: 표준 클래스 등록 + 창 생성 + Shell_NotifyIcon 시퀀스.
    unsafe {
        let hinstance = GetModuleHandleW(ptr::null());

        // 트레이 메시지 수신용 숨김 창 클래스.
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(tray_wnd_proc);
        wc.hInstance = hinstance;
        wc.lpszClassName = class_tray.as_ptr();
        if RegisterClassW(&wc) == 0 {
            return Err(GetLastError());
        }

        // 정보 다이얼로그 클래스(흰 배경 + 화살표 커서).
        let mut wcd: WNDCLASSW = std::mem::zeroed();
        wcd.lpfnWndProc = Some(dialog_wnd_proc);
        wcd.hInstance = hinstance;
        wcd.lpszClassName = class_dlg.as_ptr();
        wcd.hCursor = LoadCursorW(ptr::null_mut(), IDC_ARROW);
        wcd.hbrBackground = GetSysColorBrush(COLOR_WINDOW);
        if RegisterClassW(&wcd) == 0 {
            return Err(GetLastError());
        }

        // 숨김 트레이 창(표시 안 함) — 콜백 수신 + 메뉴/포그라운드 소유자.
        let hwnd = CreateWindowExW(
            0,
            class_tray.as_ptr(),
            wide("Caps Hangul Tray").as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            hinstance,
            ptr::null(),
        );
        if hwnd.is_null() {
            return Err(GetLastError());
        }
        TRAY_HWND.store(hwnd, Ordering::SeqCst);

        // 임베드된 앱 아이콘(RT_GROUP_ICON id=1)을 트레이용 작은 아이콘으로 로드.
        let cx = GetSystemMetrics(SM_CXSMICON);
        let cy = GetSystemMetrics(SM_CYSMICON);
        let mut hicon =
            LoadImageW(hinstance, 1 as *const u16, IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR) as HICON;
        if hicon.is_null() {
            hicon = LoadIconW(hinstance, 1 as *const u16); // 폴백(기본 크기).
        }
        TRAY_ICON.store(hicon, Ordering::SeqCst);

        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = TRAY_UID;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = hicon;
        set_sz(&mut nid.szTip, "caps-hangul-rs — Han/Eng · Caps Lock toggle");
        if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
            return Err(GetLastError());
        }
        TRAY_ADDED.store(true, Ordering::SeqCst);
    }
    Ok(())
}

/// 트레이 아이콘을 제거하고 아이콘 핸들을 해제한다. 여러 번 호출해도 안전(idempotent).
pub fn shutdown() {
    // SAFETY: 추가된 적이 있을 때만 NIM_DELETE; 아이콘 핸들도 한 번만 파괴.
    unsafe {
        if TRAY_ADDED.swap(false, Ordering::SeqCst) {
            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = TRAY_HWND.load(Ordering::SeqCst);
            nid.uID = TRAY_UID;
            Shell_NotifyIconW(NIM_DELETE, &nid);
        }
        let icon = TRAY_ICON.swap(ptr::null_mut(), Ordering::SeqCst);
        if !icon.is_null() {
            DestroyIcon(icon);
        }
    }
}

/// 현재 열린 정보 다이얼로그 핸들(없으면 null). 메시지 루프의 `IsDialogMessageW` 위임용.
pub fn active_dialog() -> *mut c_void {
    INFO_DLG.load(Ordering::SeqCst)
}

/// 트레이 숨김 창 프로시저: 트레이 콜백 + 종료 처리.
unsafe extern "system" fn tray_wnd_proc(
    hwnd: *mut c_void,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TRAY => {
            // 클래식(v3) 콜백: lParam 하위 워드가 마우스 메시지.
            match (lparam as u32) & 0xFFFF {
                WM_RBUTTONUP => show_context_menu(hwnd),
                WM_LBUTTONDBLCLK => show_info_dialog(hwnd),
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            shutdown();
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 우클릭 컨텍스트 메뉴(Info / Exit)를 띄우고 선택을 처리한다.
unsafe fn show_context_menu(hwnd: *mut c_void) {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }
    // `&` 니모닉 → 단축키 I / X. 표시 텍스트는 "Info" / "Exit".
    AppendMenuW(menu, MF_STRING, ID_INFO as usize, wide("&Info").as_ptr());
    AppendMenuW(menu, MF_STRING, ID_EXIT as usize, wide("E&xit").as_ptr());

    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);

    // 메뉴가 바깥 클릭으로 정상 닫히려면 포그라운드여야 한다(고전 트레이 메뉴 관용구).
    SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_RETURNCMD,
        pt.x,
        pt.y,
        0,
        hwnd,
        ptr::null(),
    );
    PostMessageW(hwnd, WM_NULL, 0, 0); // TrackPopupMenu 직후 빈 메시지(닫힘 버그 회피).
    DestroyMenu(menu);

    match cmd as u32 {
        ID_INFO => show_info_dialog(hwnd),
        ID_EXIT => {
            DestroyWindow(hwnd); // → WM_DESTROY → shutdown + PostQuitMessage.
        }
        _ => {} // 선택 없이 닫힘.
    }
}

/// 정보 다이얼로그(모드리스)를 띄운다. 이미 열려 있으면 앞으로 가져온다.
unsafe fn show_info_dialog(owner: *mut c_void) {
    let existing = INFO_DLG.load(Ordering::SeqCst);
    if !existing.is_null() {
        SetForegroundWindow(existing);
        return;
    }
    let hinstance = GetModuleHandleW(ptr::null());

    // 대상 모니터(커서 기준)의 작업 영역과 DPI.
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(hmon, &mut mi);
    let mut dpi_x: u32 = 96;
    let mut dpi_y: u32 = 96;
    let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
    if dpi_x == 0 {
        dpi_x = 96;
    }
    let scale = dpi_x as f32 / 96.0;
    let s = |v: i32| (v as f32 * scale).round() as i32;

    // 클라이언트(논리 px) → DPI 보정한 외곽 크기.
    let win_style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    let win_ex = WS_EX_DLGMODALFRAME;
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: s(440),
        bottom: s(280),
    };
    AdjustWindowRectExForDpi(&mut rc, win_style, 0, win_ex, dpi_x);
    let ww = rc.right - rc.left;
    let wh = rc.bottom - rc.top;
    let wx = mi.rcWork.left + ((mi.rcWork.right - mi.rcWork.left) - ww) / 2;
    let wy = mi.rcWork.top + ((mi.rcWork.bottom - mi.rcWork.top) - wh) / 2;

    let class_dlg = wide("CapsHangulInfoDialog");
    let title = wide(&format!("About {PROGRAM_NAME}"));
    let dlg = CreateWindowExW(
        win_ex,
        class_dlg.as_ptr(),
        title.as_ptr(),
        win_style,
        wx,
        wy,
        ww,
        wh,
        owner,
        ptr::null_mut(),
        hinstance,
        ptr::null(),
    );
    if dlg.is_null() {
        return;
    }

    // 폰트 3종(제목 굵게 / 본문 / 링크 밑줄). 닫을 때 해제.
    // CreateFontW 인자: height(-px), width, escapement, orientation, weight, italic,
    //   underline, strikeout, charset(1=DEFAULT), outprec, clipprec, quality(5=CLEARTYPE),
    //   pitch&family, facename.
    let f_title = CreateFontW(
        -s(21), 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, 5, 0, wide("Segoe UI").as_ptr(),
    );
    let f_text = CreateFontW(
        -s(15), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, wide("Segoe UI").as_ptr(),
    );
    let f_link = CreateFontW(
        -s(15), 0, 0, 0, 400, 0, 1, 0, 1, 0, 0, 5, 0, wide("Segoe UI").as_ptr(),
    );
    DLG_FONT_TITLE.store(f_title, Ordering::SeqCst);
    DLG_FONT_TEXT.store(f_text, Ordering::SeqCst);
    DLG_FONT_LINK.store(f_link, Ordering::SeqCst);

    // 헤더 아이콘(48 논리 px) + 타이틀바 아이콘(공유 리소스 → 해제 불필요).
    let icon_px = s(48);
    let header_icon =
        LoadImageW(hinstance, 1 as *const u16, IMAGE_ICON, icon_px, icon_px, LR_DEFAULTCOLOR)
            as HICON;
    DLG_ICON.store(header_icon, Ordering::SeqCst);
    let title_icon = LoadIconW(hinstance, 1 as *const u16);
    if !title_icon.is_null() {
        SendMessageW(dlg, WM_SETICON, ICON_BIG as usize, title_icon as isize);
        SendMessageW(dlg, WM_SETICON, ICON_SMALL as usize, title_icon as isize);
    }

    // 자식 컨트롤 생성 헬퍼(좌표/크기는 호출부에서 DPI 스케일된 값).
    let mk = |class: &str, text: &str, extra: u32, x: i32, y: i32, w: i32, h: i32, id: u32| {
        // SAFETY: 닫힌-환경 클로저는 unsafe 컨텍스트를 상속하지 않으므로 명시 블록.
        unsafe {
            CreateWindowExW(
                0,
                wide(class).as_ptr(),
                wide(text).as_ptr(),
                WS_CHILD | WS_VISIBLE | extra,
                x,
                y,
                w,
                h,
                dlg,
                id as usize as *mut c_void,
                hinstance,
                ptr::null(),
            )
        }
    };

    let m = 18; // 바깥 여백
    let icon_w = 48;
    let text_x = m + icon_w + 14; // 아이콘 오른쪽 텍스트 시작 x
    let body_w = 440 - text_x - m; // 제목/버전 폭
    let inner_w = 440 - 2 * m; // 전체폭 라인 폭

    // 헤더 아이콘.
    let ic = mk("STATIC", "", SS_ICON, s(m), s(20), s(icon_w), s(icon_w), 0);
    if !header_icon.is_null() {
        SendMessageW(ic, STM_SETICON, header_icon as usize, 0);
    }

    // 제목 / 버전.
    let title_ctl = mk("STATIC", PROGRAM_NAME, 0, s(text_x), s(22), s(body_w), s(30), 0);
    let ver_ctl = mk(
        "STATIC",
        &format!("Version {VERSION}"),
        0,
        s(text_x),
        s(56),
        s(body_w),
        s(22),
        0,
    );

    // 라이선스 고지문(2줄).
    let license = "MIT License\r\nCopyright © 2026 Hyojae Cho";
    let lic_ctl = mk("STATIC", license, 0, s(m), s(100), s(inner_w), s(44), 0);

    // GitHub 저장소 라벨 + 하이퍼링크.
    let gh_ctl = mk("STATIC", "GitHub repository", 0, s(m), s(158), s(inner_w), s(20), 0);
    let link = mk("STATIC", REPO_URL, SS_NOTIFY, s(m), s(180), s(inner_w), s(22), ID_LINK);
    LINK_HWND.store(link, Ordering::SeqCst);

    // 닫기 버튼(기본 버튼 → Enter, IDCANCEL → Esc).
    let btn_w = 96;
    let btn = mk(
        "BUTTON",
        "Close",
        BS_DEFPUSHBUTTON | WS_TABSTOP,
        s((440 - btn_w) / 2),
        s(230),
        s(btn_w),
        s(30),
        ID_CLOSE,
    );

    // 폰트 적용.
    SendMessageW(title_ctl, WM_SETFONT, f_title as usize, 1);
    SendMessageW(ver_ctl, WM_SETFONT, f_text as usize, 1);
    SendMessageW(lic_ctl, WM_SETFONT, f_text as usize, 1);
    SendMessageW(gh_ctl, WM_SETFONT, f_text as usize, 1);
    SendMessageW(link, WM_SETFONT, f_link as usize, 1);
    SendMessageW(btn, WM_SETFONT, f_text as usize, 1);

    INFO_DLG.store(dlg, Ordering::SeqCst);
    ShowWindow(dlg, SW_SHOW);
    SetForegroundWindow(dlg);
    SetFocus(btn);
}

/// 정보 다이얼로그 프로시저.
unsafe extern "system" fn dialog_wnd_proc(
    hwnd: *mut c_void,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CTLCOLORSTATIC => {
            // 흰 배경 위 텍스트; 링크는 파란 글자색.
            let hdc = wparam as *mut c_void;
            SetBkMode(hdc, TRANSPARENT as i32);
            if (lparam as *mut c_void) == LINK_HWND.load(Ordering::SeqCst) {
                SetTextColor(hdc, rgb(0, 102, 204));
            }
            GetSysColorBrush(COLOR_WINDOW) as LRESULT
        }
        WM_SETCURSOR => {
            // 링크 위에서는 손 모양 커서.
            if (wparam as *mut c_void) == LINK_HWND.load(Ordering::SeqCst) {
                SetCursor(LoadCursorW(ptr::null_mut(), IDC_HAND));
                return 1;
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_COMMAND => match (wparam & 0xFFFF) as u32 {
            ID_LINK => {
                open_url(REPO_URL);
                0
            }
            ID_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        },
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_NCDESTROY => {
            // 전역 초기화 + GDI 자원 해제.
            INFO_DLG.store(ptr::null_mut(), Ordering::SeqCst);
            LINK_HWND.store(ptr::null_mut(), Ordering::SeqCst);
            let icon = DLG_ICON.swap(ptr::null_mut(), Ordering::SeqCst);
            if !icon.is_null() {
                DestroyIcon(icon);
            }
            for f in [&DLG_FONT_TITLE, &DLG_FONT_TEXT, &DLG_FONT_LINK] {
                let h = f.swap(ptr::null_mut(), Ordering::SeqCst);
                if !h.is_null() {
                    DeleteObject(h);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 시스템 기본 브라우저로 URL 을 연다.
unsafe fn open_url(url: &str) {
    let op = wide("open");
    let u = wide(url);
    ShellExecuteW(
        ptr::null_mut(),
        op.as_ptr(),
        u.as_ptr(),
        ptr::null(),
        ptr::null(),
        SW_SHOWNORMAL,
    );
}
