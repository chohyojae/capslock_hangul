//! 중복 실행 방지 (§11). Windows named mutex 사용.
//!
//! 일반 사용자 환경을 우선 고려하여 `Local\` namespace 를 사용한다(§11.2).

use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;

/// 사용자 세션 단위 mutex 이름 (§11.2).
const MUTEX_NAME: &str = "Local\\CapsHangulRustMutex";

/// 보유 시 단일 인스턴스를 의미하는 RAII 가드. Drop 시 핸들을 닫는다.
pub struct SingleInstance {
    handle: HANDLE,
}

impl SingleInstance {
    /// mutex 를 생성하여 단일 인스턴스 여부를 판정한다.
    ///
    /// - `Ok(Some(_))`: 이 프로세스가 첫 인스턴스.
    /// - `Ok(None)`: 이미 다른 인스턴스가 실행 중.
    /// - `Err(code)`: mutex 생성 실패 (Win32 error code).
    pub fn acquire() -> Result<Option<Self>, u32> {
        // SAFETY: w!(MUTEX_NAME) 은 널 종료된 'static UTF-16 포인터다.
        unsafe {
            let handle = CreateMutexW(ptr::null(), 1, w!(MUTEX_NAME));
            if handle.is_null() {
                return Err(GetLastError());
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(handle);
                return Ok(None);
            }
            Ok(Some(SingleInstance { handle }))
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        // SAFETY: acquire 성공 시에만 생성되는 유효한 핸들.
        unsafe {
            CloseHandle(self.handle);
        }
    }
}
