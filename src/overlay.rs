//! macOS 풍의 전환 안내 HUD 오버레이.
//!
//! 한/영 전환(짧게 누름) 및 Caps Lock 토글(길게 누름) 시 활성 모니터 중앙에
//! 반투명 둥근 박스로 "한 / A" 또는 "CAPS ON / OFF" 를 잠깐 띄웠다가 페이드아웃한다.
//!
//! ## 설계 요점
//! - 전용 레이어드 창(click-through, no-activate, topmost)을 메인 스레드에 미리 만들어 둔다.
//! - 훅 콜백에서는 atomic 갱신 + `PostMessage` 만 하고 즉시 반환한다(콜백 내 I/O 금지 §16.4).
//! - **언어(한/영) 라벨은 "확인 후 표시"** 한다:
//!   1. 토글 키를 **콜백이 아닌 오버레이 핸들러**에서 보낸다([`request_language_toggle`]).
//!   2. 핸들러는 키를 보내기 **직전에** 실제 IME 변환 모드를 조회한다(아직 아무것도
//!      주입하지 않았으므로 신뢰할 수 있는 현재 상태).
//!   3. `VK_HANGUL` 전송 후 결과(= `!이전상태`)로 **한 번에** 올바른 라벨을 띄운다.
//!   → 추정 표시 후 보정하던 방식의 중간 깜빡임이 없고, IME 로 직접 전환한 뒤에도
//!     매번 실제 상태를 새로 읽으므로 라벨이 어긋나지 않는다.
//! - **Caps Lock 라벨**은 토글이 결정적이므로 추정값(시작 시 실제값 seed)으로 즉시 표시한다.
//! - 핵심 토글이 오버레이에 종속되지 않도록, 오버레이 미준비/커스텀 키일 때는
//!   호출 측(훅)이 직접 키를 전송한다([`is_ready`]).
//! - HiDPI: 프로세스를 Per-Monitor-V2 DPI aware 로 설정(`win32::set_dpi_aware`)하고,
//!   모니터 DPI 에 맞춰 박스/폰트 크기를 스케일해 또렷하게 렌더링한다.

use core::ffi::c_void;
use std::mem::size_of;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{COLORREF, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, DrawTextW,
    EndPaint, FillRect, GetMonitorInfoW, InvalidateRect, MonitorFromPoint, MonitorFromWindow,
    SelectObject, SetBkMode, SetTextColor, SetWindowRgn, DT_CENTER, DT_SINGLELINE, DT_VCENTER,
    HFONT, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT, TRANSPARENT,
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

use crate::{input, state};

// ── 라벨 코드 (WM_APP_SHOW 의 wParam / show_label 인자) ─────────────────────
const LBL_HANGUL: u32 = 1; // "한"
const LBL_ENGLISH: u32 = 2; // "A"
const LBL_CAPS_ON: u32 = 3; // "CAPS ON"
const LBL_CAPS_OFF: u32 = 4; // "CAPS OFF"

// ── 윈도우 메시지 / 타이머 ID ────────────────────────────────────────────────
/// 라벨을 그대로 표시(Caps 즉시 표시용). wParam=라벨 코드.
const WM_APP_SHOW: u32 = WM_APP;
/// 언어 전환: 핸들러가 (IME 조회 → VK_HANGUL 전송 → 표시) 순서로 처리.
const WM_APP_LANG: u32 = WM_APP + 1;
const TIMER_HOLD: usize = 1; // 표시 유지 후 페이드 시작
const TIMER_FADE: usize = 2; // 알파를 단계적으로 줄여 숨김
const TIMER_CAPS: usize = 3; // Caps Lock 임계 시간 도달 감지(누르고 있는 동안)

// ── 타이밍/스타일 상수 ───────────────────────────────────────────────────────
const HOLD_MS: u32 = 750; // 완전 표시 유지 시간
const FADE_STEP_MS: u32 = 20; // 페이드 틱 간격
const FADE_DEC: i32 = 22; // 틱당 알파 감소량
const BASE_ALPHA: u8 = 235; // 표시 시 기본 알파(0~255)

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
/// Caps Lock 추정 상태(true=ON). 시작 시 실제값으로 seed 하고 길게 누름마다 토글한다.
static CAPS_ON: AtomicBool = AtomicBool::new(false);

/// UTF-8 → 널 종료 UTF-16.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 오버레이 창을 생성하고 전역에 등록한다. 메시지 루프 진입 전에 한 번 호출한다.
///
/// 실패해도 치명적이지 않으며, 이 경우 [`is_ready`] 가 false 를 반환해
/// 호출 측이 HUD 없이 동작하도록 한다.
pub fn init() -> Result<(), u32> {
    // Caps Lock 추정값을 실제 토글 상태로 seed.
    // SAFETY: GetKeyState 는 부작용 없는 단순 조회.
    let caps = unsafe { (GetKeyState(VK_CAPITAL as i32) & 0x0001) != 0 };
    CAPS_ON.store(caps, Ordering::SeqCst);

    let class_name = wide("CapsHangulOverlayWindow");

    // SAFETY: 표준 윈도우 클래스 등록 + 창 생성 시퀀스.
    unsafe {
        let hinstance = GetModuleHandleW(ptr::null());

        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(wnd_proc);
        wc.hInstance = hinstance;
        wc.lpszClassName = class_name.as_ptr();
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
            class_name.as_ptr(),
            wide("Caps Hangul Overlay").as_ptr(),
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
        DARK_BRUSH.store(CreateSolidBrush(rgb(28, 28, 30)), Ordering::SeqCst);
        OVERLAY_HWND.store(hwnd, Ordering::SeqCst);
    }

    Ok(())
}

/// 오버레이 창이 준비됐는지 여부. false 면 호출 측이 직접 키를 전송해야 한다.
pub fn is_ready() -> bool {
    !OVERLAY_HWND.load(Ordering::SeqCst).is_null()
}

/// 한/영 전환 요청. **훅 콜백에서 호출**하며, 실제 IME 조회·키 전송·표시는
/// 오버레이 핸들러(WM_APP_LANG)가 콜백 밖에서 수행한다.
///
/// 호출 측은 [`is_ready`] 가 true 이고 단축키가 `VK_HANGUL` 일 때만 호출한다.
pub fn request_language_toggle() {
    let hwnd = OVERLAY_HWND.load(Ordering::SeqCst);
    if hwnd.is_null() {
        return;
    }
    // SAFETY: 유효한 창. PostMessage 는 콜백에서 안전.
    unsafe {
        PostMessageW(hwnd, WM_APP_LANG, 0, 0);
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

/// Caps Lock 토글을 안내한다(라벨은 결정적이라 즉시 표시). **훅 콜백에서 호출**.
/// 실제 `VK_CAPITAL` 전송은 호출 측(훅)에서 이미 수행한 뒤다.
pub fn notify_caps() {
    let hwnd = OVERLAY_HWND.load(Ordering::SeqCst);
    if hwnd.is_null() {
        return;
    }
    let was_on = CAPS_ON.fetch_xor(true, Ordering::SeqCst);
    let label = if !was_on { LBL_CAPS_ON } else { LBL_CAPS_OFF };
    // SAFETY: 유효한 창. PostMessage 는 콜백에서 안전.
    unsafe {
        PostMessageW(hwnd, WM_APP_SHOW, label as WPARAM, 0);
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
        LBL_HANGUL => (150, 150, 30, 92, true),
        LBL_ENGLISH => (150, 150, 30, 92, false),
        // CAPS ON / OFF
        _ => (300, 130, 28, 46, false),
    }
}

/// 라벨 코드에 대응하는 표시 텍스트.
fn label_text(label: u32) -> &'static str {
    match label {
        LBL_HANGUL => "한",
        LBL_ENGLISH => "A",
        LBL_CAPS_ON => "CAPS ON",
        LBL_CAPS_OFF => "CAPS OFF",
        _ => "",
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
    let scale = dpi as f32 / 96.0;
    let (bw, bh, brad, bfont, hangul_font) = metrics_96(label);
    let w = (bw as f32 * scale).round() as i32;
    let h = (bh as f32 * scale).round() as i32;
    let rad = (brad as f32 * scale).round() as i32;
    let font_px = (bfont as f32 * scale).round() as i32;

    let x = rc.left + ((rc.right - rc.left) - w) / 2;
    let y = rc.top + ((rc.bottom - rc.top) - h) / 2;

    // 폰트 (DPI 에 맞춰 새로 생성, 이전 폰트 해제).
    let face = if hangul_font { "Malgun Gothic" } else { "Segoe UI" };
    let face_w = wide(face);
    // 인자: height(-px), width, escapement, orientation, weight(600=SemiBold),
    //       italic, underline, strikeout, charset(1=DEFAULT),
    //       outprec(0), clipprec(0), quality(5=CLEARTYPE), pitch&family(0), facename
    let font: HFONT = CreateFontW(
        -font_px, 0, 0, 0, 600, 0, 0, 0, 1, 0, 0, 5, 0, face_w.as_ptr(),
    );
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

/// WM_APP_LANG: 실제 IME 상태를 **먼저 확인한 뒤** 토글하고 올바른 라벨로 표시한다.
unsafe fn handle_language_toggle(hwnd: *mut c_void) {
    // 1) 토글 직전 실제 한/영 상태를 포커스 스레드 주입 리더로 조회(아직 키 미주입 → 신뢰 가능).
    //    Teams 등 TSF/Chromium 앱 포함 정확. 주입 불가/미초기화 시 None → 추정값 폴백.
    let pre = crate::ime::read_focus_conversion();

    // 2) 한/영 토글 키 주입(핵심 동작).
    input::send_key(state::SHORT_PRESS_VK.load(Ordering::SeqCst));

    // 3) 결과 상태 = !이전상태. 조회 실패 시 내부 추정값을 폴백으로 토글.
    let new_hangul = match pre {
        Some(prev) => {
            let now = !prev;
            LANG_HANGUL.store(now, Ordering::SeqCst);
            now
        }
        None => {
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

    let mut text = wide(label_text(CURRENT_LABEL.load(Ordering::SeqCst)));
    DrawTextW(
        hdc,
        text.as_mut_ptr(),
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
        WM_APP_LANG => {
            handle_language_toggle(hwnd);
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
                    state::LONG_FIRED.store(true, Ordering::SeqCst);
                    let vk = state::LONG_PRESS_VK.load(Ordering::SeqCst);
                    input::send_key(vk);
                    if vk == VK_CAPITAL {
                        let was_on = CAPS_ON.fetch_xor(true, Ordering::SeqCst);
                        show_label(hwnd, if !was_on { LBL_CAPS_ON } else { LBL_CAPS_OFF });
                    }
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
