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
//!   다이얼로그는 지속형 창이라 모니터 간 이동 시 `WM_GETDPISCALEDSIZE`(정확한 목표 외곽
//!   크기 통보) + `WM_DPICHANGED`(제안 rect 적용 후 [`apply_dialog_dpi`] 로 폰트/아이콘/
//!   컨트롤 재계산)로 배율 변경에 대응한다.

use core::ffi::c_void;
use std::mem::size_of;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{
    GetLastError, COLORREF, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    DeleteObject, GetMonitorInfoW, GetSysColorBrush, InvalidateRect, MonitorFromPoint, SetBkMode,
    SetTextColor, COLOR_WINDOW, MONITORINFO, MONITOR_DEFAULTTONEAREST, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu,
    DestroyWindow, GetCursorPos, GetDlgItem, GetSystemMetrics, KillTimer, LoadCursorW, LoadIconW,
    LoadImageW, MoveWindow, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
    SendMessageW, SetCursor, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow,
    TrackPopupMenu, HICON, ICON_BIG, ICON_SMALL, IDC_ARROW, IDC_HAND, IMAGE_ICON, LR_DEFAULTCOLOR,
    MF_STRING, SM_CXSMICON, SM_CYSMICON, STM_SETICON, SWP_NOACTIVATE, SWP_NOZORDER, SW_SHOW,
    SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CLOSE, WM_COMMAND,
    WM_CTLCOLORSTATIC, WM_DESTROY, WM_DPICHANGED, WM_GETDPISCALEDSIZE, WM_LBUTTONDBLCLK,
    WM_NCDESTROY, WM_NULL, WM_RBUTTONUP, WM_SETCURSOR, WM_SETFONT, WM_SETICON, WM_TIMER,
    WNDCLASSW, WS_CAPTION, WS_CHILD, WS_EX_DLGMODALFRAME, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP,
    WS_VISIBLE,
};

// ── 트레이/메뉴/컨트롤 식별자 ────────────────────────────────────────────────
/// 트레이 아이콘이 마우스 이벤트를 보낼 콜백 메시지(이 창 전용 → 충돌 없음).
const WM_TRAY: u32 = WM_APP + 0x20;
/// 트레이 아이콘 ID(창당 1개).
const TRAY_UID: u32 = 1;
/// 트레이 등록 재시도 타이머 ID.
const TIMER_TRAY_RETRY: usize = 1;
/// Explorer/알림 영역 준비를 기다리는 재시도 간격.
const TRAY_RETRY_MS: u32 = 3_000;
/// 컨텍스트 메뉴 명령 ID.
const ID_INFO: u32 = 1001;
const ID_EXIT: u32 = 1002;
/// 다이얼로그 컨트롤 ID. 닫기 버튼은 IDCANCEL(=2)로 둬 Esc 도 동일 처리되게 한다.
/// 나머지 STATIC 들은 DPI 변경 시 `GetDlgItem` 으로 되찾아 재배치하기 위한 ID
/// (SS_NOTIFY 없는 STATIC 은 WM_COMMAND 를 보내지 않으므로 명령 분기와 충돌 없음).
const ID_LINK: u32 = 101;
const ID_CLOSE: u32 = 2;
const ID_HEADER_ICON: u32 = 102;
const ID_TITLE: u32 = 103;
const ID_VERSION: u32 = 104;
const ID_LICENSE: u32 = 105;

// ── 정보 다이얼로그 레이아웃(96-DPI 논리 px 단일 소스) ──────────────────────
// 생성(show_info_dialog)과 DPI 재배치(apply_dialog_dpi)·외곽 재계산(WM_GETDPISCALEDSIZE)이
// 모두 이 값을 공유한다 — 경로가 갈라져 드리프트하지 않게 하기 위함.
/// 다이얼로그 창 스타일 / 확장 스타일.
const DLG_STYLE: u32 = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
const DLG_EX: u32 = WS_EX_DLGMODALFRAME;
/// 클라이언트 영역 논리 크기 (폭, 높이).
const DLG_CLIENT_96: (i32, i32) = (440, 258);
/// 자식 컨트롤 배치 `(ID, x, y, w, h)`. 유도 근거: 바깥 여백 18, 헤더 아이콘 48,
/// 텍스트 시작 x = 18+48+14 = 80, 제목/버전 폭 = 440-80-18 = 342,
/// 전체폭 라인 폭 = 440-2*18 = 404, 버튼 96x30 중앙 = (440-96)/2 = 172.
const DLG_LAYOUT_96: [(u32, i32, i32, i32, i32); 6] = [
    (ID_HEADER_ICON, 18, 20, 48, 48),
    (ID_TITLE, 80, 22, 342, 30),
    (ID_VERSION, 80, 56, 342, 22),
    (ID_LICENSE, 18, 100, 404, 44),
    (ID_LINK, 18, 158, 404, 22),
    (ID_CLOSE, 172, 208, 96, 30),
];

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
static TASKBAR_CREATED_MSG: AtomicU32 = AtomicU32::new(0);

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
    let class_tray = w!("CapsHangulTrayWindow");
    let class_dlg = w!("CapsHangulInfoDialog");

    // SAFETY: 표준 클래스 등록 + 창 생성 + Shell_NotifyIcon 시퀀스.
    unsafe {
        let hinstance = GetModuleHandleW(ptr::null());

        // 트레이 메시지 수신용 숨김 창 클래스.
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(tray_wnd_proc);
        wc.hInstance = hinstance;
        wc.lpszClassName = class_tray;
        if RegisterClassW(&wc) == 0 {
            return Err(GetLastError());
        }

        // 정보 다이얼로그 클래스(흰 배경 + 화살표 커서).
        let mut wcd: WNDCLASSW = std::mem::zeroed();
        wcd.lpfnWndProc = Some(dialog_wnd_proc);
        wcd.hInstance = hinstance;
        wcd.lpszClassName = class_dlg;
        wcd.hCursor = LoadCursorW(ptr::null_mut(), IDC_ARROW);
        wcd.hbrBackground = GetSysColorBrush(COLOR_WINDOW);
        if RegisterClassW(&wcd) == 0 {
            return Err(GetLastError());
        }

        // Explorer 가 재시작되면 기존 알림 영역 아이콘은 사라진다. Shell 이 브로드캐스트하는
        // TaskbarCreated 를 받아 같은 아이콘을 다시 등록한다.
        let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
        if taskbar_created != 0 {
            TASKBAR_CREATED_MSG.store(taskbar_created, Ordering::SeqCst);
        }

        // 숨김 트레이 창(표시 안 함) — 콜백 수신 + 메뉴/포그라운드 소유자.
        let hwnd = CreateWindowExW(
            0,
            class_tray,
            w!("Caps Hangul Tray"),
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
        let mut hicon = LoadImageW(
            hinstance,
            1 as *const u16,
            IMAGE_ICON,
            cx,
            cy,
            LR_DEFAULTCOLOR,
        ) as HICON;
        if hicon.is_null() {
            hicon = LoadIconW(hinstance, 1 as *const u16); // 폴백(기본 크기).
        }
        TRAY_ICON.store(hicon, Ordering::SeqCst);

        retry_add_tray_icon(hwnd);
    }
    Ok(())
}

/// 현재 숨김 창/아이콘 핸들로 알림 영역 아이콘을 추가한다.
unsafe fn add_tray_icon(hwnd: *mut c_void) -> Result<(), u32> {
    let hicon = TRAY_ICON.load(Ordering::SeqCst);
    if hicon.is_null() {
        return Err(GetLastError());
    }

    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_UID;
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = hicon;
    set_sz(
        &mut nid.szTip,
        "caps-hangul-rs — Han/Eng · Caps Lock toggle",
    );
    if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
        return Err(GetLastError());
    }
    Ok(())
}

/// 알림 영역 준비 전 로그온 경합이나 Explorer 재시작 뒤 아이콘 유실을 복구한다.
unsafe fn retry_add_tray_icon(hwnd: *mut c_void) {
    if hwnd.is_null() || TRAY_ADDED.load(Ordering::SeqCst) {
        return;
    }

    match add_tray_icon(hwnd) {
        Ok(()) => {
            TRAY_ADDED.store(true, Ordering::SeqCst);
            KillTimer(hwnd, TIMER_TRAY_RETRY);
        }
        Err(_) => {
            SetTimer(hwnd, TIMER_TRAY_RETRY, TRAY_RETRY_MS, None);
        }
    }
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
    if msg == TASKBAR_CREATED_MSG.load(Ordering::SeqCst) {
        TRAY_ADDED.store(false, Ordering::SeqCst);
        retry_add_tray_icon(hwnd);
        return 0;
    }

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
        WM_TIMER if wparam == TIMER_TRAY_RETRY => {
            retry_add_tray_icon(hwnd);
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
    AppendMenuW(menu, MF_STRING, ID_INFO as usize, w!("&Info"));
    AppendMenuW(menu, MF_STRING, ID_EXIT as usize, w!("E&xit"));

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
    let s = |v: i32| crate::win32::scale_px(v, dpi_x);

    // 클라이언트(논리 px) → DPI 보정한 외곽 크기.
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: s(DLG_CLIENT_96.0),
        bottom: s(DLG_CLIENT_96.1),
    };
    AdjustWindowRectExForDpi(&mut rc, DLG_STYLE, 0, DLG_EX, dpi_x);
    let ww = rc.right - rc.left;
    let wh = rc.bottom - rc.top;
    let wx = mi.rcWork.left + ((mi.rcWork.right - mi.rcWork.left) - ww) / 2;
    let wy = mi.rcWork.top + ((mi.rcWork.bottom - mi.rcWork.top) - wh) / 2;

    let class_dlg = w!("CapsHangulInfoDialog");
    let title = wide(&format!("About {PROGRAM_NAME}"));
    let dlg = CreateWindowExW(
        DLG_EX,
        class_dlg,
        title.as_ptr(),
        DLG_STYLE,
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

    // 타이틀바 아이콘(공유 리소스 → 해제 불필요).
    let title_icon = LoadIconW(hinstance, 1 as *const u16);
    if !title_icon.is_null() {
        SendMessageW(dlg, WM_SETICON, ICON_BIG as usize, title_icon as isize);
        SendMessageW(dlg, WM_SETICON, ICON_SMALL as usize, title_icon as isize);
    }

    // 자식 컨트롤 생성 헬퍼. 위치/크기는 0 으로 두고 폰트/헤더 아이콘과 함께
    // apply_dialog_dpi 가 일괄 적용한다(생성 경로와 DPI 변경 경로의 단일화).
    let mk = |class: &str, text: &str, extra: u32, id: u32| {
        // SAFETY: 닫힌-환경 클로저는 unsafe 컨텍스트를 상속하지 않으므로 명시 블록.
        unsafe {
            CreateWindowExW(
                0,
                wide(class).as_ptr(),
                wide(text).as_ptr(),
                WS_CHILD | WS_VISIBLE | extra,
                0,
                0,
                0,
                0,
                dlg,
                id as usize as *mut c_void,
                hinstance,
                ptr::null(),
            )
        }
    };

    mk("STATIC", "", SS_ICON, ID_HEADER_ICON); // 헤더 아이콘
    mk("STATIC", PROGRAM_NAME, 0, ID_TITLE); // 제목
    mk("STATIC", &format!("Version {VERSION}"), 0, ID_VERSION); // 버전
    // 라이선스 고지문(2줄).
    mk("STATIC", "MIT License\r\nCopyright © 2026 Hyojae Cho", 0, ID_LICENSE);
    // GitHub 저장소 하이퍼링크.
    let link = mk("STATIC", REPO_URL, SS_NOTIFY, ID_LINK);
    LINK_HWND.store(link, Ordering::SeqCst);
    // 닫기 버튼(기본 버튼 → Enter, IDCANCEL → Esc).
    let btn = mk("BUTTON", "Close", BS_DEFPUSHBUTTON | WS_TABSTOP, ID_CLOSE);

    // DPI 종속 자원(폰트/헤더 아이콘) 생성 + 자식 배치.
    apply_dialog_dpi(dlg, dpi_x);

    INFO_DLG.store(dlg, Ordering::SeqCst);
    ShowWindow(dlg, SW_SHOW);
    SetForegroundWindow(dlg);
    SetFocus(btn);
}

/// 다이얼로그의 DPI 종속 자원(폰트 3종·헤더 아이콘)과 자식 컨트롤 배치를 지정 DPI 로
/// (재)적용한다. 생성 직후와 WM_DPICHANGED 가 공유하는 단일 경로.
///
/// 폰트/아이콘은 **새것을 컨트롤에 적용한 뒤 옛것을 파괴**한다 — 컨트롤이 파괴된
/// 핸들을 참조하는 순간이 없도록. 전역(DLG_FONT_*/DLG_ICON)은 항상 최신 핸들을
/// 담으므로 WM_NCDESTROY 의 해제 경로도 그대로 유효하다.
unsafe fn apply_dialog_dpi(dlg: *mut c_void, dpi: u32) {
    let s = |v: i32| crate::win32::scale_px(v, dpi);
    let hinstance = GetModuleHandleW(ptr::null());

    // 폰트 3종(제목 굵게 / 본문 / 링크 밑줄).
    let f_title = crate::win32::create_ui_font(s(21), 700, false, w!("Segoe UI"));
    let f_text = crate::win32::create_ui_font(s(15), 400, false, w!("Segoe UI"));
    let f_link = crate::win32::create_ui_font(s(15), 400, true, w!("Segoe UI"));

    // 헤더 아이콘(48 논리 px).
    let icon_px = s(48);
    let header_icon =
        LoadImageW(hinstance, 1 as *const u16, IMAGE_ICON, icon_px, icon_px, LR_DEFAULTCOLOR)
            as HICON;

    // 자식 재배치 + 새 폰트/아이콘 적용.
    for (id, x, y, w, h) in DLG_LAYOUT_96 {
        let ctl = GetDlgItem(dlg, id as i32);
        if ctl.is_null() {
            continue;
        }
        MoveWindow(ctl, s(x), s(y), s(w), s(h), 1);
        let font = match id {
            ID_HEADER_ICON => {
                if !header_icon.is_null() {
                    SendMessageW(ctl, STM_SETICON, header_icon as usize, 0);
                }
                continue; // 아이콘 컨트롤은 폰트 불필요.
            }
            ID_TITLE => f_title,
            ID_LINK => f_link,
            _ => f_text,
        };
        SendMessageW(ctl, WM_SETFONT, font as usize, 1);
    }

    // 전역을 새 핸들로 교체하고, 더는 참조되지 않는 이전 핸들을 파괴.
    // 아이콘은 로드가 실패(null)했으면 교체하지 않는다 — 컨트롤이 여전히 옛 아이콘을
    // 표시 중이므로 파괴하면 안 되고, 옛 핸들은 WM_NCDESTROY 가 해제한다.
    if !header_icon.is_null() {
        let old_icon = DLG_ICON.swap(header_icon, Ordering::SeqCst);
        if !old_icon.is_null() {
            DestroyIcon(old_icon);
        }
    }
    for (slot, new) in [
        (&DLG_FONT_TITLE, f_title),
        (&DLG_FONT_TEXT, f_text),
        (&DLG_FONT_LINK, f_link),
    ] {
        let old = slot.swap(new, Ordering::SeqCst);
        if !old.is_null() {
            DeleteObject(old);
        }
    }
    InvalidateRect(dlg, ptr::null(), 1);
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
        WM_GETDPISCALEDSIZE => {
            // DPI 변경 확정 전 OS 질의(wParam=새 DPI, lParam=*mut SIZE 입출력).
            // 클라이언트 논리 크기(440x258)를 새 DPI 로 정확히 유지한 외곽 크기를 알려 줘,
            // 뒤이어 오는 WM_DPICHANGED 의 제안 rect 가 우리 레이아웃과 픽셀 단위로
            // 일치하게 한다(논클라이언트 비선형 스케일 오차 제거).
            let new_dpi = wparam as u32;
            let mut rc = RECT {
                left: 0,
                top: 0,
                right: crate::win32::scale_px(DLG_CLIENT_96.0, new_dpi),
                bottom: crate::win32::scale_px(DLG_CLIENT_96.1, new_dpi),
            };
            if AdjustWindowRectExForDpi(&mut rc, DLG_STYLE, 0, DLG_EX, new_dpi) == 0 {
                return 0; // 실패 → OS 기본(선형 스케일)에 맡긴다. SIZE 미기록 시 1 반환 금지.
            }
            let size = lparam as *mut SIZE;
            (*size).cx = rc.right - rc.left;
            (*size).cy = rc.bottom - rc.top;
            1
        }
        WM_DPICHANGED => {
            // 모니터 간 이동 등으로 DPI 가 바뀜(wParam 하위 워드=새 DPI, lParam=제안 RECT).
            // 제안 rect 는 **가공 없이 그대로** 적용한다 — 여기서 크기를 자체 재계산하면
            // 모니터 경계에 걸친 창이 DPI 왕복 진동(핑퐁)에 빠질 수 있다. 크기 정밀도는
            // WM_GETDPISCALEDSIZE 가 이미 보장한다.
            let new_dpi = (wparam & 0xFFFF) as u32;
            let rc = &*(lparam as *const RECT);
            SetWindowPos(
                hwnd,
                ptr::null_mut(),
                rc.left,
                rc.top,
                rc.right - rc.left,
                rc.bottom - rc.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            apply_dialog_dpi(hwnd, new_dpi);
            0
        }
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
            // 다이얼로그가 건드린 GDI/컨트롤 페이지를 다시 idle 로 돌려보낸다(작업 집합 복귀).
            crate::win32::trim_working_set();
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 시스템 기본 브라우저로 URL 을 연다.
unsafe fn open_url(url: &str) {
    let op = w!("open");
    let u = wide(url);
    ShellExecuteW(
        ptr::null_mut(),
        op,
        u.as_ptr(),
        ptr::null(),
        ptr::null(),
        SW_SHOWNORMAL,
    );
}
