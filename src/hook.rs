//! WH_KEYBOARD_LL 훅 설치/해제 및 LowLevelKeyboardProc 구현 (§5.2 hook.rs, §6, §19.3).

use core::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use windows_sys::Win32::Foundation::{GetLastError, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_CAPITAL;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT,
    LLKHF_INJECTED, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::config::{classify_press, PressKind};
use crate::{input, state, win32};

/// 설치된 훅 핸들. 종료 시 해제하기 위해 전역에 보관한다.
static HOOK_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// RAII 가드. Drop 시점에 훅을 해제한다(§3.3, §10.3).
pub struct KeyboardHook(());

impl KeyboardHook {
    /// WH_KEYBOARD_LL 훅을 설치한다. 실패 시 Win32 error code 를 반환한다.
    pub fn install() -> Result<Self, u32> {
        // SAFETY: 표준 Win32 훅 설치 시퀀스. 콜백은 'static 함수 포인터.
        unsafe {
            let hmod = GetModuleHandleW(ptr::null());
            let handle = SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(low_level_keyboard_proc),
                hmod,
                0,
            );
            if handle.is_null() {
                return Err(GetLastError());
            }
            HOOK_HANDLE.store(handle, Ordering::SeqCst);
            Ok(KeyboardHook(()))
        }
    }
}

impl Drop for KeyboardHook {
    fn drop(&mut self) {
        // SAFETY: 설치에 성공한 핸들만 보관되어 있으며, 한 번만 해제한다.
        unsafe {
            let handle = HOOK_HANDLE.swap(ptr::null_mut(), Ordering::SeqCst);
            if !handle.is_null() {
                UnhookWindowsHookEx(handle);
            }
        }
    }
}

/// 저수준 키보드 훅 콜백 (§6, §19.3).
///
/// 콜백 내부에서는 I/O / 동적 할당 / 로깅을 하지 않고 최대한 빠르게 반환한다(§3.1, §16.4).
unsafe extern "system" fn low_level_keyboard_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // code < 0 (HC_ACTION 이 아님)이면 반드시 다음 훅으로 넘긴다.
    if code != HC_ACTION as i32 {
        return CallNextHookEx(ptr::null_mut(), code, wparam, lparam);
    }

    // SAFETY: HC_ACTION 인 경우 lParam 은 KBDLLHOOKSTRUCT 를 가리킨다.
    let kb = &*(lparam as *const KBDLLHOOKSTRUCT);
    let vk = kb.vkCode as u16;

    // 재진입 방지(§8.4, §16.1):
    // - 우리가 합성한 입력(INJECTING) 또는 injected flag 가 설정된 입력은 그대로 통과시킨다.
    // - Caps Lock 이외의 키는 원래 흐름을 유지한다(§6.3).
    let injected = (kb.flags & LLKHF_INJECTED) != 0;
    if injected || state::INJECTING.load(Ordering::SeqCst) || vk != VK_CAPITAL {
        return CallNextHookEx(ptr::null_mut(), code, wparam, lparam);
    }

    match wparam as u32 {
        // KeyDown: 최초 눌림 시각만 기록하고 원래 입력을 차단(§6.1, §6.2).
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if !state::CAPS_DOWN.swap(true, Ordering::SeqCst) {
                state::CAPS_DOWN_TIME_MS.store(win32::now_ms(), Ordering::SeqCst);
            }
            1 // non-zero: 원래 Caps Lock KeyDown 차단
        }
        // KeyUp: elapsed 계산 후 짧게/길게에 따라 합성 입력 전송, 원래 입력 차단.
        WM_KEYUP | WM_SYSKEYUP => {
            let down_time = state::CAPS_DOWN_TIME_MS.load(Ordering::SeqCst);
            state::CAPS_DOWN.store(false, Ordering::SeqCst);

            let elapsed = win32::now_ms().saturating_sub(down_time);
            let threshold = state::THRESHOLD_MS.load(Ordering::SeqCst);

            let vk_to_send = match classify_press(elapsed, threshold) {
                PressKind::Long => state::LONG_PRESS_VK.load(Ordering::SeqCst),
                PressKind::Short => state::SHORT_PRESS_VK.load(Ordering::SeqCst),
            };
            input::send_key(vk_to_send);

            1 // non-zero: 원래 Caps Lock KeyUp 차단
        }
        _ => CallNextHookEx(ptr::null_mut(), code, wparam, lparam),
    }
}
