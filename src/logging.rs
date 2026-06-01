//! 간단한 로깅 (§5.2 logging.rs, §12).
//!
//! 메모리와 의존성을 줄이기 위해 로깅 프레임워크를 사용하지 않는다(§3.2).
//! 디버그 빌드에서만 stderr 로 출력하고, 릴리스 빌드에서는 비활성화한다(§12.2).
//!
//! 주의: 훅 콜백 내부에서는 절대 호출하지 않는다(§16.4 — 콜백 내 I/O 금지).

/// 디버그 빌드에서만 메시지를 stderr 로 출력한다.
#[inline]
pub fn log(message: &str) {
    #[cfg(debug_assertions)]
    {
        eprintln!("[caps-hangul-rs] {message}");
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = message;
    }
}
