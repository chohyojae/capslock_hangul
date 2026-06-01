//! SendInput 기반 키 입력 합성 (§5.2 input.rs, §8).

use std::mem::size_of;
use std::sync::atomic::Ordering;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
};

use crate::state;

/// 지정한 virtual-key 를 down/up 쌍으로 한 번 전송한다(§8.3).
///
/// 합성 입력이 다시 훅에 들어와도 재처리되지 않도록 `INJECTING` 플래그를 설정한다(§8.4).
/// 실제 재진입 방지의 1차 방어선은 훅에서의 injected-flag 확인이다(§16.1).
pub fn send_key(vk: u16) {
    if vk == 0 {
        return;
    }

    state::INJECTING.store(true, Ordering::SeqCst);

    let mut inputs = [make_key_input(vk, false), make_key_input(vk, true)];

    // SAFETY: inputs 는 유효한 INPUT 배열이며 cbSize 도 정확히 전달한다.
    unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_mut_ptr(),
            size_of::<INPUT>() as i32,
        );
    }

    state::INJECTING.store(false, Ordering::SeqCst);
}

/// 단일 키보드 INPUT 구조체를 만든다.
fn make_key_input(vk: u16, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if key_up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
