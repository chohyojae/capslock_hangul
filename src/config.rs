//! 설정값 정의 (§9, §18).
//!
//! 초기 버전에서는 설정 파일 없이 컴파일 타임 기본값을 사용한다.
//! 향후 TOML 설정 파일 로드 기능을 이 모듈에 추가할 수 있다.

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_CAPITAL, VK_HANGUL};

/// 짧게 누름 / 길게 누름을 가르는 기준 시간 (ms). §18: 250ms.
pub const LONG_PRESS_THRESHOLD_MS: u64 = 250;

/// 런타임 설정값.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// 길게 누름 판정 기준 시간 (ms).
    pub long_press_threshold_ms: u64,
    /// 짧게 누름 시 전송할 virtual-key (기본 VK_HANGUL = 0x15).
    pub short_press_vk: u16,
    /// 길게 누름 시 전송할 virtual-key (기본 VK_CAPITAL = 0x14).
    pub long_press_vk: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            long_press_threshold_ms: LONG_PRESS_THRESHOLD_MS,
            short_press_vk: VK_HANGUL,
            long_press_vk: VK_CAPITAL,
        }
    }
}

/// 누름 동작 분류 결과 (§15.1 단위 테스트 대상).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum PressKind {
    /// 짧게 누름 → 한/영 전환.
    Short,
    /// 길게 누름 → Caps Lock 토글.
    Long,
}

/// elapsed time 과 threshold 를 비교하여 짧게/길게 누름을 판정한다.
///
/// 경계값(elapsed == threshold)은 길게 누름으로 본다(§15.1).
pub fn classify_press(elapsed_ms: u64, threshold_ms: u64) -> PressKind {
    if elapsed_ms >= threshold_ms {
        PressKind::Long
    } else {
        PressKind::Short
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // §15.1 예시 테스트 케이스.
    #[test]
    fn elapsed_below_threshold_is_short() {
        assert_eq!(classify_press(100, 250), PressKind::Short);
    }

    #[test]
    fn elapsed_at_threshold_is_long() {
        assert_eq!(classify_press(250, 250), PressKind::Long);
    }

    #[test]
    fn elapsed_above_threshold_is_long() {
        assert_eq!(classify_press(500, 250), PressKind::Long);
    }
}
