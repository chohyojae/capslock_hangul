//! Win32 API 의 얇은 wrapper (§5.2 win32.rs).
//!
//! unsafe 호출을 이 모듈에 한정하여 나머지 코드의 안전성을 높인다.

use std::ptr;

use windows_sys::Win32::Graphics::Gdi::{CreateFontW, HFONT};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, IsDialogMessageW, TranslateMessage, MSG,
};

/// 단조 증가하는 millisecond tick 값 (§7.3 권장 GetTickCount64 기반).
#[inline]
pub fn now_ms() -> u64 {
    // SAFETY: GetTickCount64 는 인자가 없고 단순히 u64 를 반환한다.
    unsafe { GetTickCount64() }
}

/// 프로세스를 Per-Monitor-V2 DPI 인식으로 설정한다.
///
/// HUD 오버레이를 HiDPI 모니터에서 비트맵 확대(흐릿함) 없이 또렷하게 그리기 위함이다.
/// 어떤 창도 만들기 전에(메인 진입 직후) 호출해야 한다. Win10 1703 미만에서는
/// 함수 호출이 실패할 수 있으나 무시한다(시스템 DPI 인식으로 동작).
pub fn set_dpi_aware() {
    // SAFETY: 인자는 상수 컨텍스트 핸들이며, 반환값(성공 여부)은 무시한다.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// 96-DPI 논리 px 를 대상 DPI 의 물리 px 로 변환한다(가장 가까운 정수로 반올림).
///
/// overlay/tray 가 공유하는 스케일 함수. 반올림 방식(`f32::round`)을 바꾸면
/// 기존 렌더링과 1px 어긋날 수 있으므로 유지한다.
#[inline]
pub fn scale_px(v_96: i32, dpi: u32) -> i32 {
    (v_96 as f32 * (dpi as f32 / 96.0)).round() as i32
}

/// DPI 스케일된 UI 폰트를 만든다(overlay/tray 공용 관용구).
///
/// CreateFontW 인자: height(-px), width, escapement, orientation, weight, italic,
///   underline, strikeout, charset(1=DEFAULT), outprec, clipprec, quality(5=CLEARTYPE),
///   pitch&family, facename(널 종료 UTF-16, `w!` 리터럴 등 수명이 호출보다 긴 포인터).
pub fn create_ui_font(px: i32, weight: i32, underline: bool, face: *const u16) -> HFONT {
    // SAFETY: face 는 널 종료 UTF-16 포인터이며 CreateFontW 는 호출 중에만 읽는다.
    unsafe { CreateFontW(-px, 0, 0, 0, weight, 0, underline as u32, 0, 1, 0, 0, 5, 0, face) }
}

/// 현재 프로세스의 작업 집합(working set)을 트림한다.
///
/// `SetProcessWorkingSetSize(proc, (SIZE_T)-1, (SIZE_T)-1)` 은 "최소 크기로 줄여라"를
/// 뜻하는 표준 관용구로, 안 쓰는 페이지를 standby 목록으로 내려 보고되는 메모리를 낮춘다.
/// 상주 트레이 앱처럼 초기화 후 대부분 idle 인 프로세스에서 효과적이다. 페이지는 필요 시
/// 자동으로 다시 fault-in 되므로 동작 정확성에는 영향이 없다. 실패해도 무해(무시).
pub fn trim_working_set() {
    // SAFETY: GetCurrentProcess 는 의사 핸들을 돌려주고, (usize::MAX, usize::MAX) =
    // (SIZE_T)-1 은 문서화된 "최소로 트림" 신호다. 반환값(성공 여부)은 무시한다.
    unsafe {
        SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

/// 표준 Win32 메시지 루프 (§5.1, §10.2).
///
/// WM_QUIT 를 수신하기 전까지 블로킹되며, idle 상태에서는 CPU 를 거의 사용하지 않는다.
/// 저수준 키보드 훅 콜백은 이 스레드가 메시지를 디스패치할 때 호출된다.
///
/// 정보 다이얼로그(트레이 메뉴)가 열려 있으면 그 메시지를 `IsDialogMessageW` 에 먼저 위임해
/// Tab/Enter/Esc/기본 버튼 같은 다이얼로그 키보드 탐색이 동작하게 한다(다이얼로그가 없으면
/// `active_dialog` 가 null → 추가 비용 없이 평소대로 디스패치).
pub fn run_message_loop() {
    // SAFETY: msg 는 GetMessageW 가 채워주는 출력 버퍼이며, 루프 동안만 사용된다.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        // GetMessageW: 오류 시 -1, WM_QUIT 시 0, 그 외 양수.
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            let dlg = crate::tray::active_dialog();
            if !dlg.is_null() && IsDialogMessageW(dlg, &msg) != 0 {
                continue; // 다이얼로그가 처리함(이미 디스패치됨).
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
