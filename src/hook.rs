//! WH_KEYBOARD_LL 훅 설치/해제 및 LowLevelKeyboardProc 구현 (§5.2 hook.rs, §6, §19.3).

use core::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use windows_sys::Win32::Foundation::{GetLastError, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_CAPITAL, VK_HANGUL};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT,
    LLKHF_INJECTED, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::config::{classify_press, PressKind};
use crate::{input, overlay, state, win32};

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
        // KeyDown: 최초 눌림 시각 기록 + 임계 시간 타이머 무장, 원래 입력 차단(§6.1, §6.2).
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            // 최초 눌림에서만 시각 기록 + 타이머 무장한다. CAPS_DOWN 이 이미 true 인
            // KeyDown 은 auto-repeat(누른 채 유지)이므로 무시한다 — Caps Lock 도 길게 누르면
            // 반복 KeyDown 이 온다. 이를 무시하지 않으면 (1) 반복마다 타이머가 다시 무장돼
            // 길게 누름이 영영 확정되지 않거나, (2) 타이머 발화 후 도착한 반복이 LONG_FIRED 를
            // 리셋해 떼는 시점에 짧게 누름(한/영)으로 잘못 처리된다.
            if !state::CAPS_DOWN.swap(true, Ordering::SeqCst) {
                state::CAPS_DOWN_TIME_MS.store(win32::now_ms(), Ordering::SeqCst);
                state::LONG_FIRED.store(false, Ordering::SeqCst);
                // 누른 채 임계 시간을 넘기면 떼기 전에 길게 누름(Caps 토글+오버레이)을 확정한다.
                let threshold = state::THRESHOLD_MS.load(Ordering::SeqCst);
                overlay::arm_caps_long_press(threshold as u32);
            }
            1 // non-zero: 원래 Caps Lock KeyDown 차단
        }
        // KeyUp: 길게 누름이 타이머에서 이미 처리됐으면 무시. 아니면 짧게 누름으로 처리
        // (오버레이 준비 시 길게 누름은 타이머 전담). 오버레이 미준비 시에만 경과시간 폴백.
        WM_KEYUP | WM_SYSKEYUP => {
            let down_time = state::CAPS_DOWN_TIME_MS.load(Ordering::SeqCst);
            state::CAPS_DOWN.store(false, Ordering::SeqCst);
            overlay::cancel_caps_long_press();

            // 임계 시간 타이머에서 길게 누름을 이미 확정·실행했으면 떼는 시점엔 아무 동작 안 함.
            if state::LONG_FIRED.swap(false, Ordering::SeqCst) {
                return 1;
            }

            // 길게 누름은 (오버레이 준비 시) 임계 타이머가 전담한다. 여기까지 왔다는 건
            // 타이머가 길게 누름을 확정하지 않았다는 뜻 = 임계 시간 전에 뗐다는 의미이므로
            // 짧게 누름으로 처리한다. 이렇게 하면 빠른 연타 중 놓친 KeyUp 으로 down_time 이
            // 옛 값에 고정돼도(경과시간이 부풀어도) 길게 누름으로 오판하지 않는다.
            // 오버레이 미준비(타이머 없음)일 때만 경과시간으로 길게/짧게 판정한다(폴백).
            let kind = if overlay::is_ready() {
                PressKind::Short
            } else {
                let elapsed = win32::now_ms().saturating_sub(down_time);
                let threshold = state::THRESHOLD_MS.load(Ordering::SeqCst);
                classify_press(elapsed, threshold)
            };

            match kind {
                PressKind::Long => {
                    // 오버레이 미준비/커스텀 키 폴백: 콜백에서 직접 전송한다.
                    // (오버레이 준비 시 Caps 길게 누름은 위 타이머 경로가 처리한다.)
                    let vk = state::LONG_PRESS_VK.load(Ordering::SeqCst);
                    input::send_key(vk);
                }
                PressKind::Short => {
                    // 한/영(VK_HANGUL)일 때는 오버레이 핸들러가 (실제 IME 조회 → 키 전송 →
                    // 표시) 순서로 처리하도록 위임한다. 그래야 라벨이 실제 상태와 일치한다.
                    // 오버레이 미준비/커스텀 키일 때는 콜백에서 직접 전송(폴백).
                    let vk = state::SHORT_PRESS_VK.load(Ordering::SeqCst);
                    if vk == VK_HANGUL && overlay::is_ready() {
                        overlay::request_language_toggle();
                    } else {
                        input::send_key(vk);
                    }
                }
            }

            1 // non-zero: 원래 Caps Lock KeyUp 차단
        }
        _ => CallNextHookEx(ptr::null_mut(), code, wparam, lparam),
    }
}
