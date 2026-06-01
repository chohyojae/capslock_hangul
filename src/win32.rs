//! Win32 API 의 얇은 wrapper (§5.2 win32.rs).
//!
//! unsafe 호출을 이 모듈에 한정하여 나머지 코드의 안전성을 높인다.

use std::ptr;

use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, TranslateMessage, MSG,
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

/// 표준 Win32 메시지 루프 (§5.1, §10.2).
///
/// WM_QUIT 를 수신하기 전까지 블로킹되며, idle 상태에서는 CPU 를 거의 사용하지 않는다.
/// 저수준 키보드 훅 콜백은 이 스레드가 메시지를 디스패치할 때 호출된다.
pub fn run_message_loop() {
    // SAFETY: msg 는 GetMessageW 가 채워주는 출력 버퍼이며, 루프 동안만 사용된다.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        // GetMessageW: 오류 시 -1, WM_QUIT 시 0, 그 외 양수.
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
