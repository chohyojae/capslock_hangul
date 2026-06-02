//! 전역 상태 관리 (§7).
//!
//! Win32 콜백(`extern "system" fn`)에서 접근해야 하므로 전역 static atomic 을 사용한다.
//! 훅 콜백 내부에서는 동적 메모리 할당이나 lock 을 피하기 위해 모두 lock-free atomic 으로 둔다.

use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};

use crate::config::Config;

/// Caps Lock 키가 (우리 훅 추적상) 눌려 있는지 여부 (§7.2). KeyDown 에서 true,
/// KeyUp 에서 false 로 둔다. 두 가지 용도:
/// 1) auto-repeat KeyDown 억제 — 이미 true 면 반복 입력으로 보고 무시.
/// 2) 임계 타이머 발화 시 "지금도 눌려 있는지" 판정. 물리 키를 훅이 차단하므로 OS 키 상태
///    API(GetAsyncKeyState/GetKeyState)로는 알 수 없어, 이 추적값이 유일한 근거다.
pub static CAPS_DOWN: AtomicBool = AtomicBool::new(false);

/// Caps Lock KeyDown 이 처음 감지된 시각 (GetTickCount64 기준 ms) (§7.2, §7.3).
pub static CAPS_DOWN_TIME_MS: AtomicU64 = AtomicU64::new(0);

/// SendInput 으로 합성 입력을 보내는 중인지 여부 (§7.2, §8.4 재진입 방지 보조).
pub static INJECTING: AtomicBool = AtomicBool::new(false);

/// 길게 누름(Caps Lock 토글)이 임계 시간 타이머에서 이미 확정·실행됐는지 여부.
/// 떼기 전에 임계 시간을 넘기면 타이머가 동작을 실행하고 이 값을 true 로 둔다.
/// KeyUp 은 이 값을 확인·리셋하여 중복 실행을 막는다.
pub static LONG_FIRED: AtomicBool = AtomicBool::new(false);

/// 길게 누름 판정 기준 시간 (ms). `init` 에서 설정값으로 갱신한다.
pub static THRESHOLD_MS: AtomicU64 = AtomicU64::new(crate::config::LONG_PRESS_THRESHOLD_MS);

/// 짧게 누름 시 전송할 virtual-key.
pub static SHORT_PRESS_VK: AtomicU16 = AtomicU16::new(0);

/// 길게 누름 시 전송할 virtual-key.
pub static LONG_PRESS_VK: AtomicU16 = AtomicU16::new(0);

/// 설정값을 전역 상태에 반영한다. 메시지 루프 진입 전에 한 번 호출한다.
pub fn init(config: &Config) {
    THRESHOLD_MS.store(config.long_press_threshold_ms, Ordering::SeqCst);
    SHORT_PRESS_VK.store(config.short_press_vk, Ordering::SeqCst);
    LONG_PRESS_VK.store(config.long_press_vk, Ordering::SeqCst);
}
