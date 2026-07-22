//! macOS 풍의 전환 안내 HUD 오버레이.
//!
//! 한/영 전환(짧게 누름) 및 Caps Lock 토글(길게 누름) 시 활성 모니터 중앙에
//! 반투명 둥근 박스로 "한 / A" 또는 "CAPS ON / OFF" 를 잠깐 띄웠다가 페이드아웃한다.
//!
//! ## 설계 요점
//! - 전용 레이어드 창(click-through, no-activate, topmost)을 메인 스레드에 미리 만들어 둔다.
//! - 훅 콜백에서는 atomic 갱신 + `PostMessage` 만 하고 즉시 반환한다(콜백 내 I/O 금지 §16.4).
//! - **언어(한/영) 전환은 "떼는 즉시", 라벨은 "사전조회 후 표시"** 한다:
//!   1. `VK_HANGUL` 전송은 **떼는 순간 훅 콜백에서 즉시**(SendInput) 한다 — 전환이
//!      다음 물리 키보다 확실히 앞서 입력 큐에 들어가므로, 떼자마자 빠르게 타이핑해도
//!      첫 글자가 전환 전에 입력되는 일이 없다.
//!   2. 라벨용 실제 IME 변환 모드 조회는 **누르고 있는 동안(dwell)** 미리 해 둔다
//!      ([`request_language_preread`]). 느릴 수 있는 이 조회를 전환 직전(임계 경로)에서
//!      빼고 dwell 시간 안으로 숨기는 것이 핵심이다(단일 스레드라 사전조회는 KeyUp
//!      콜백보다 먼저 처리되어 캐시는 항상 "전송 전" 상태를 담는다).
//!   3. 떼는 순간([`request_language_show`])엔 사전조회 결과(전송 전 상태)의 부정으로
//!      올바른 라벨을 **한 번에** 띄운다(중간 깜빡임 없음). 조회 실패 시 추정값 폴백.
//! - **Caps Lock 라벨**도 동일하게 토글 키 전송 **직전 실제 토글 상태**를 읽어 결정한다.
//!   (`GetKeyState` 의 lock 비트는 포커스 없는 백그라운드 스레드에서도 전역 상태를 반영한다.)
//!   매번 새로 읽으므로 권한 격리(UIPI)로 주입이 무시되거나 화면 키보드·원격 세션 등으로
//!   외부에서 Caps 가 바뀌어도 추정값이 누적으로 어긋나 영구히 뒤집히는 일이 없다.
//! - 핵심 토글이 오버레이에 종속되지 않도록, 오버레이 미준비/커스텀 키일 때는
//!   호출 측(훅)이 직접 키를 전송한다([`is_ready`]).
//! - HiDPI: 프로세스를 Per-Monitor-V2 DPI aware 로 설정(`win32::set_dpi_aware`)하고,
//!   표시할 때마다 모니터 DPI 에 맞춰 박스/폰트 크기를 스케일(`win32::scale_px` /
//!   `win32::create_ui_font`)해 또렷하게 렌더링한다(재표시 방식이라 WM_DPICHANGED 불필요).

use core::ffi::c_void;
use std::mem::size_of;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{COLORREF, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    GetMonitorInfoW, InvalidateRect, MonitorFromPoint, MonitorFromWindow, SelectObject, SetBkMode,
    SetTextColor, SetWindowRgn, DT_CENTER, DT_SINGLELINE, DT_VCENTER, HFONT, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, PAINTSTRUCT, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, GetCursorPos, GetForegroundWindow, KillTimer,
    PostMessageW, RegisterClassW, SetLayeredWindowAttributes, SetTimer,
    SetWindowPos, ShowWindow, HWND_TOPMOST, LWA_ALPHA, SWP_NOACTIVATE, SW_HIDE,
    SW_SHOWNOACTIVATE, WM_APP, WM_PAINT, WM_TIMER, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::{input, state, win32};

// ── 라벨 코드 (WM_APP_SHOW 의 wParam / show_label 인자) ─────────────────────
const LBL_HANGUL: u32 = 1; // "한"
const LBL_ENGLISH: u32 = 2; // "A"
const LBL_CAPS_ON: u32 = 3; // "CAPS ON"
const LBL_CAPS_OFF: u32 = 4; // "CAPS OFF"

// ── 윈도우 메시지 / 타이머 ID ────────────────────────────────────────────────
/// 라벨을 그대로 표시(Caps 즉시 표시용). wParam=라벨 코드.
const WM_APP_SHOW: u32 = WM_APP;
/// 한/영 사전조회: KeyDown 시 dwell 동안 실제 IME 상태를 미리 읽어 캐시한다.
const WM_APP_LANG_PREREAD: u32 = WM_APP + 1;
/// 한/영 라벨 표시: KeyUp(짧게) 시 — 전환 키 전송은 콜백에서 이미 끝났고 라벨만 띄운다.
const WM_APP_LANG_SHOW: u32 = WM_APP + 2;
const TIMER_HOLD: usize = 1; // 표시 유지 후 페이드 시작
const TIMER_FADE: usize = 2; // 알파를 단계적으로 줄여 숨김
const TIMER_CAPS: usize = 3; // Caps Lock 임계 시간 도달 감지(누르고 있는 동안)

// ── 타이밍/스타일 상수 ───────────────────────────────────────────────────────
const HOLD_MS: u32 = 750; // 완전 표시 유지 시간
const FADE_STEP_MS: u32 = 20; // 페이드 틱 간격
const FADE_DEC: i32 = 22; // 틱당 알파 감소량
const BASE_ALPHA: u8 = 150; // 표시 시 기본 알파(0~255). 낮을수록 더 비침(화면을 덜 가림)

// ── 전역 상태 (메인 스레드 전용이지만 콜백 공유 위해 atomic 사용) ─────────────
static OVERLAY_HWND: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static OVERLAY_FONT: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static DARK_BRUSH: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// 현재 표시 중인 라벨 코드(WM_PAINT 가 읽음).
static CURRENT_LABEL: AtomicU32 = AtomicU32::new(0);
/// 페이드 진행 중 알파값.
static FADE_ALPHA: AtomicU32 = AtomicU32::new(0);

/// 한/영 추정 상태(true=한글). IME 조회가 실패할 때만 폴백으로 사용.
static LANG_HANGUL: AtomicBool = AtomicBool::new(false);
/// KeyDown 사전조회 결과(전송 전 상태). 0=미확정, 1=한글, 2=영문.
/// KeyUp 의 라벨 표시가 `swap(0)` 으로 소비하며, 매 KeyDown 이 항상 새로 덮어써
/// (긴 누름 등으로) 소비되지 못한 이전 값이 다음 짧게 누름에 새지 않게 한다.
static PREREAD: AtomicU32 = AtomicU32::new(0);
/// Caps Lock 표시 캐시(true=ON). 길게 누름마다 **실제 토글 상태를 다시 읽어** 갱신한다.
/// 추정 누적이 아니므로 외부 변경이 있어도 다음 토글에서 자동으로 맞춰진다.
static CAPS_ON: AtomicBool = AtomicBool::new(false);

/// 현재 OS Caps Lock 토글 상태(true=ON)를 실제로 읽는다.
///
/// `GetKeyState` 의 **lock(low) 비트**는 눌림(high) 비트와 달리 전역 토글 상태를 반영하므로,
/// 포커스 없는(메시지를 안 받는) 이 오버레이 스레드에서도 정확하다(실측 확인).
#[inline]
fn caps_lock_on() -> bool {
    // SAFETY: GetKeyState 는 부작용 없는 단순 조회.
    unsafe { (GetKeyState(VK_CAPITAL as i32) & 0x0001) != 0 }
}

/// 오버레이 창을 생성하고 전역에 등록한다. 메시지 루프 진입 전에 한 번 호출한다.
///
/// 실패해도 치명적이지 않으며, 이 경우 [`is_ready`] 가 false 를 반환해
/// 호출 측이 HUD 없이 동작하도록 한다.
pub fn init() -> Result<(), u32> {
    // Caps Lock 표시 캐시를 실제 토글 상태로 seed(이후 매 토글마다 실제값을 다시 읽는다).
    CAPS_ON.store(caps_lock_on(), Ordering::SeqCst);

    let class_name = w!("CapsHangulOverlayWindow");

    // SAFETY: 표준 윈도우 클래스 등록 + 창 생성 시퀀스.
    unsafe {
        let hinstance = GetModuleHandleW(ptr::null());

        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(wnd_proc);
        wc.hInstance = hinstance;
        wc.lpszClassName = class_name;
        // hbrBackground 는 null: 배경 지우기를 직접 WM_PAINT 에서 처리한다.
        if RegisterClassW(&wc) == 0 {
            return Err(windows_sys::Win32::Foundation::GetLastError());
        }

        let ex_style = WS_EX_LAYERED
            | WS_EX_TRANSPARENT   // click-through: 마우스 입력을 가로채지 않음
            | WS_EX_TOPMOST
            | WS_EX_NOACTIVATE    // 포커스를 빼앗지 않음
            | WS_EX_TOOLWINDOW;   // 작업 표시줄/Alt-Tab 에 노출 안 함

        let hwnd = CreateWindowExW(
            ex_style,
            class_name,
            w!("Caps Hangul Overlay"),
            WS_POPUP,
            0,
            0,
            10,
            10,
            ptr::null_mut(),
            ptr::null_mut(),
            hinstance,
            ptr::null(),
        );
        if hwnd.is_null() {
            return Err(windows_sys::Win32::Foundation::GetLastError());
        }

        // 어두운 반투명 배경 브러시(한 번만 생성). RGB(28,28,30).
        DARK_BRUSH.store(CreateSolidBrush(rgb(48, 48, 52)), Ordering::SeqCst);
        OVERLAY_HWND.store(hwnd, Ordering::SeqCst);
    }

    Ok(())
}

/// 오버레이 창이 준비됐는지 여부. false 면 호출 측이 직접 키를 전송해야 한다.
pub fn is_ready() -> bool {
    !OVERLAY_HWND.load(Ordering::SeqCst).is_null()
}

/// 한/영 사전조회 요청. **KeyDown 시 훅 콜백에서 호출**한다.
///
/// 메시지 루프가 dwell(키를 누르고 있는 동안) 실제 IME 변환 모드를 미리 읽어 캐시한다.
/// 떼는 순간의 전환 키 전송은 호출 측(훅)이 콜백에서 즉시 처리하므로, 느릴 수 있는 이
/// 조회는 전환 직전(임계 경로)에서 빠진다. 단일 스레드 모델이라 이 사전조회는 KeyUp
/// 콜백보다 먼저 처리되어, 캐시는 항상 "전송 전" 상태를 담는다.
///
/// 호출 측은 [`is_ready`] 가 true 이고 단축키가 `VK_HANGUL` 일 때만 호출한다.
pub fn request_language_preread() {
    let hwnd = OVERLAY_HWND.load(Ordering::SeqCst);
    if hwnd.is_null() {
        return;
    }
    // SAFETY: 유효한 창. PostMessage 는 콜백에서 안전.
    unsafe {
        PostMessageW(hwnd, WM_APP_LANG_PREREAD, 0, 0);
    }
}

/// 한/영 라벨 표시 요청. **KeyUp(짧게) 시 훅 콜백에서 호출**한다(전환 키 전송 직후).
///
/// 토글은 이미 콜백에서 끝났으므로, 핸들러는 사전조회 캐시(전송 전 상태)로 결과
/// 라벨만 표시한다.
///
/// 호출 측은 [`is_ready`] 가 true 이고 단축키가 `VK_HANGUL` 일 때만 호출한다.
pub fn request_language_show() {
    let hwnd = OVERLAY_HWND.load(Ordering::SeqCst);
    if hwnd.is_null() {
        return;
    }
    // SAFETY: 유효한 창. PostMessage 는 콜백에서 안전.
    unsafe {
        PostMessageW(hwnd, WM_APP_LANG_SHOW, 0, 0);
    }
}

/// Caps Lock 임계 시간 타이머를 건다(첫 KeyDown 시 **훅 콜백에서 호출**).
///
/// 떼기 전에 임계 시간을 넘기면 [`TIMER_CAPS`] 가 그 순간 토글+표시를 확정한다.
/// 오버레이 미준비 시 no-op(이 경우 호출 측이 KeyUp 에서 직접 처리).
pub fn arm_caps_long_press(threshold_ms: u32) {
    let hwnd = OVERLAY_HWND.load(Ordering::SeqCst);
    if hwnd.is_null() {
        return;
    }
    // SAFETY: 유효한 창. SetTimer 는 가벼운 호출이라 콜백에서 허용.
    unsafe {
        SetTimer(hwnd, TIMER_CAPS, threshold_ms, None);
    }
}

/// Caps Lock 임계 시간 타이머를 취소한다(KeyUp 시 호출). no-op 안전.
pub fn cancel_caps_long_press() {
    let hwnd = OVERLAY_HWND.load(Ordering::SeqCst);
    if hwnd.is_null() {
        return;
    }
    // SAFETY: 유효한 창. 걸려 있지 않아도 안전.
    unsafe {
        KillTimer(hwnd, TIMER_CAPS);
    }
}

/// COLORREF(0x00BBGGRR) 생성.
#[inline]
fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

/// 라벨별 기본(96 DPI) 박스/폰트 메트릭. 반환: (너비, 높이, 모서리반경, 폰트px, 한글폰트여부)
fn metrics_96(label: u32) -> (i32, i32, i32, i32, bool) {
    match label {
        LBL_HANGUL => (88, 88, 22, 52, true),
        LBL_ENGLISH => (88, 88, 22, 52, false),
        // CAPS ON / OFF
        _ => (200, 88, 22, 32, false),
    }
}

/// 라벨 코드에 대응하는 표시 텍스트(컴파일 타임 널 종료 UTF-16 포인터, 힙 할당 없음).
fn label_wide(label: u32) -> *const u16 {
    match label {
        LBL_HANGUL => w!("한"),
        LBL_ENGLISH => w!("A"),
        LBL_CAPS_ON => w!("CAPS ON"),
        LBL_CAPS_OFF => w!("CAPS OFF"),
        _ => w!(""),
    }
}

/// 활성 모니터(포그라운드 창 기준, 없으면 커서 기준)의 사각형과 DPI 를 구한다.
unsafe fn active_monitor(hwnd_self: *mut c_void) -> (RECT, u32) {
    let fg = GetForegroundWindow();
    let hmon = if !fg.is_null() && fg != hwnd_self {
        MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST)
    } else {
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
    };

    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(hmon, &mut mi);

    let mut dpi_x: u32 = 96;
    let mut dpi_y: u32 = 96;
    // 실패 시 dpi_x 는 그대로(96). Per-Monitor 인식 프로세스에서 모니터별 실제 DPI.
    let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
    if dpi_x == 0 {
        dpi_x = 96;
    }
    (mi.rcMonitor, dpi_x)
}

/// 지정 라벨을 활성 모니터 중앙에 표시한다.
unsafe fn show_label(hwnd: *mut c_void, label: u32) {
    CURRENT_LABEL.store(label, Ordering::SeqCst);

    let (rc, dpi) = active_monitor(hwnd);
    let (bw, bh, brad, bfont, hangul_font) = metrics_96(label);
    let w = win32::scale_px(bw, dpi);
    let h = win32::scale_px(bh, dpi);
    let rad = win32::scale_px(brad, dpi);
    let font_px = win32::scale_px(bfont, dpi);

    let x = rc.left + ((rc.right - rc.left) - w) / 2;
    let y = rc.top + ((rc.bottom - rc.top) - h) / 2;

    // 폰트 (DPI 에 맞춰 새로 생성, 이전 폰트 해제). face 는 컴파일 타임 UTF-16(할당 없음).
    let face_w = if hangul_font { w!("Malgun Gothic") } else { w!("Segoe UI") };
    let font: HFONT = win32::create_ui_font(font_px, 600, false, face_w); // 600=SemiBold
    let old_font = OVERLAY_FONT.swap(font, Ordering::SeqCst);
    if !old_font.is_null() {
        DeleteObject(old_font);
    }

    // 이전 사이클 타이머 정리 후 재표시.
    KillTimer(hwnd, TIMER_HOLD);
    KillTimer(hwnd, TIMER_FADE);

    // 1) 숨긴 채 위치/크기 지정 → 2) 둥근 모서리 region → 3) 알파 → 4) 표시.
    //    region 을 표시 전에 적용해, 사각 모서리가 한 프레임 보이는 현상을 막는다.
    SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE);
    let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, rad, rad);
    SetWindowRgn(hwnd, rgn, 0); // 창이 region 소유권을 가져감(직접 해제 금지).
    SetLayeredWindowAttributes(hwnd, 0, BASE_ALPHA, LWA_ALPHA);
    FADE_ALPHA.store(BASE_ALPHA as u32, Ordering::SeqCst);
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    InvalidateRect(hwnd, ptr::null(), 1);

    SetTimer(hwnd, TIMER_HOLD, HOLD_MS, None);
}

/// WM_APP_LANG_PREREAD: 전송 전 실제 한/영 상태를 읽어 캐시한다(dwell 동안 수행).
///
/// 포커스 스레드 주입 리더로 조회(아직 키 미주입 → 신뢰 가능). Teams 등 TSF/Chromium
/// 앱 포함 정확. 주입 불가/미초기화 시 `None` → 0(미확정)으로 두어 표시 단계에서 폴백.
unsafe fn handle_language_preread() {
    let v = match crate::ime::read_focus_conversion() {
        Some(true) => 1,  // 한글
        Some(false) => 2, // 영문
        None => 0,        // 미확정 → 폴백
    };
    PREREAD.store(v, Ordering::SeqCst);
}

/// WM_APP_LANG_SHOW: 전환 키 전송은 콜백에서 이미 끝났다. 사전조회 캐시(전송 전 상태)의
/// 부정으로 결과를 구해 올바른 라벨을 한 번에 띄우고 추정값을 갱신한다.
unsafe fn handle_language_show(hwnd: *mut c_void) {
    let new_hangul = match PREREAD.swap(0, Ordering::SeqCst) {
        1 => {
            // 전송 전 한글 → 전송 후 영문.
            LANG_HANGUL.store(false, Ordering::SeqCst);
            false
        }
        2 => {
            // 전송 전 영문 → 전송 후 한글.
            LANG_HANGUL.store(true, Ordering::SeqCst);
            true
        }
        _ => {
            // 사전조회 실패(미확정) → 내부 추정값을 토글해 폴백.
            let was = LANG_HANGUL.fetch_xor(true, Ordering::SeqCst);
            !was
        }
    };

    show_label(hwnd, if new_hangul { LBL_HANGUL } else { LBL_ENGLISH });
}

/// WM_PAINT: 어두운 배경 + 중앙 정렬 텍스트.
unsafe fn paint(hwnd: *mut c_void) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);

    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut rc);

    let brush = DARK_BRUSH.load(Ordering::SeqCst);
    if !brush.is_null() {
        FillRect(hdc, &rc, brush);
    }

    let font = OVERLAY_FONT.load(Ordering::SeqCst);
    let old_font = if !font.is_null() {
        SelectObject(hdc, font)
    } else {
        ptr::null_mut()
    };

    SetBkMode(hdc, TRANSPARENT as i32);
    SetTextColor(hdc, rgb(255, 255, 255));

    let text = label_wide(CURRENT_LABEL.load(Ordering::SeqCst));
    DrawTextW(
        hdc,
        text,
        -1,
        &mut rc,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
    );

    if !old_font.is_null() {
        SelectObject(hdc, old_font);
    }
    EndPaint(hwnd, &ps);
}

/// WM_TIMER(TIMER_FADE): 알파를 줄이다가 0 이 되면 숨긴다.
unsafe fn fade_tick(hwnd: *mut c_void) {
    let a = FADE_ALPHA.load(Ordering::SeqCst) as i32 - FADE_DEC;
    if a <= 0 {
        KillTimer(hwnd, TIMER_FADE);
        FADE_ALPHA.store(0, Ordering::SeqCst);
        ShowWindow(hwnd, SW_HIDE);
    } else {
        FADE_ALPHA.store(a as u32, Ordering::SeqCst);
        SetLayeredWindowAttributes(hwnd, 0, a as u8, LWA_ALPHA);
    }
}

/// 오버레이 창 프로시저.
unsafe extern "system" fn wnd_proc(
    hwnd: *mut c_void,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_APP_LANG_PREREAD => {
            handle_language_preread();
            0
        }
        WM_APP_LANG_SHOW => {
            handle_language_show(hwnd);
            0
        }
        WM_APP_SHOW => {
            show_label(hwnd, wparam as u32);
            0
        }
        WM_TIMER => {
            match wparam {
                TIMER_HOLD => {
                    KillTimer(hwnd, TIMER_HOLD);
                    SetTimer(hwnd, TIMER_FADE, FADE_STEP_MS, None);
                }
                TIMER_FADE => fade_tick(hwnd),
                TIMER_CAPS => {
                    // 누른 채 임계 시간 도달 → 떼기 전에 길게 누름(Caps 토글)을 확정.
                    KillTimer(hwnd, TIMER_CAPS);
                    // 우리 훅이 물리 Caps 입력을 차단하므로 OS 의 키 down 상태
                    // (GetAsyncKeyState/GetKeyState)는 Caps 를 절대 "눌림"으로 보지 않는다.
                    // 따라서 "지금도 눌려 있는지"는 우리 자신의 추적(CAPS_DOWN)으로만 판정한다.
                    // KeyUp 이 이미 처리됐다면 CAPS_DOWN=false → 토글하지 않는다(KillTimer 가
                    // 이미 게시된 WM_TIMER 를 제거 못 해 뒤늦게 도착한 스테일 발화를 거른다).
                    if state::CAPS_DOWN.load(Ordering::SeqCst) {
                        state::LONG_FIRED.store(true, Ordering::SeqCst);
                        let vk = state::LONG_PRESS_VK.load(Ordering::SeqCst);
                        if vk == VK_CAPITAL {
                            // 전송 직전 실제 토글 상태를 읽어, 전송 후 상태(=부정)로 라벨 결정.
                            let next_on = !caps_lock_on();
                            input::send_key(vk);
                            CAPS_ON.store(next_on, Ordering::SeqCst);
                            show_label(hwnd, if next_on { LBL_CAPS_ON } else { LBL_CAPS_OFF });
                        } else {
                            input::send_key(vk);
                        }
                    }
                    // else: KeyUp 이 이미 처리된 뒤 도착한 스테일 WM_TIMER → 무시.
                }
                _ => {}
            }
            0
        }
        WM_PAINT => {
            paint(hwnd);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
