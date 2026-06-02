//! 다른 비트니스 포커스 프로세스용 주입 헬퍼(브로커).
//!
//! `SetWindowsHookEx` 는 **호출 프로세스·주입 DLL·대상 프로세스의 비트니스가 모두 같아야** 한다
//! (DLL 이 호출자에 먼저 `LoadLibrary` 되고, 그 in-process 훅 프로시저 주소를 넘기기 때문 — README 참조).
//! 따라서 본체(`caps-hangul.exe`)가 자신과 다른 비트니스의 포커스 프로세스를 만나면, 그 비트니스로
//! 빌드된 이 헬퍼를 실행해 주입을 대신 시킨다.
//!
//! 동작:
//! 1. 자기 아키텍처용 리더 DLL(`caps-hangul-tsf-<arch>.dll`)을 로드,
//! 2. 인자로 받은 스레드에 `WH_GETMESSAGE` 훅으로 주입,
//! 3. 대상 창에 `WM_NULL` 을 보내 훅을 깨우면 DLL 이 in-process 로 한/영 변환 모드를 읽어
//!    **본체가 만든** named shared memory 에 쓰고 named event 로 신호,
//! 4. 훅을 해제하고 종료.
//!
//! 본체는 이 프로세스의 **종료를 기다린 뒤** 공유 메모리를 읽는다(공유 메모리/이벤트의 소유자는 본체).
//! 주입 불가/타임아웃이면 공유 메모리 seq 가 그대로라 본체가 추정값으로 폴백한다.
//!
//! 사용: `caps-hangul-reader <target_thread_id> <target_hwnd>`

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use core::ffi::c_void;
use std::iter::once;
use std::mem::transmute;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Threading::{OpenEventW, ResetEvent, WaitForSingleObject};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    PostMessageW, SetWindowsHookExW, UnhookWindowsHookEx, HOOKPROC, WH_GETMESSAGE, WM_NULL,
};

/// 리더 DLL 이 export 하는 훅 프로시저 이름(널 종료). DLL 측과 일치해야 한다.
const HOOK_EXPORT: &[u8] = b"caps_hangul_ime_hook\0";
/// 본체가 만든 named event 이름(본체 `src/ime.rs` / DLL 측과 일치).
const EVT_NAME: &str = "CapsHangulImeReadEvt";

// OpenEventW 접근 권한(stable ABI 값 — 모듈/피처 의존을 피해 직접 정의).
const EVENT_MODIFY_STATE: u32 = 0x0002;
const SYNCHRONIZE: u32 = 0x0010_0000;

/// DLL 이 변환 모드를 읽어 공유 메모리에 쓰기까지 기다리는 최대 시간(ms).
const READ_TIMEOUT_MS: u32 = 200;

/// UTF-8 → 널 종료 UTF-16.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(once(0)).collect()
}

/// 이 헬퍼 자신의 아키텍처에 맞는 리더 DLL 파일명.
fn dll_name() -> &'static str {
    if cfg!(target_arch = "x86") {
        "caps-hangul-tsf-x86.dll"
    } else if cfg!(target_arch = "aarch64") {
        "caps-hangul-tsf-arm64.dll"
    } else {
        "caps-hangul-tsf-x64.dll"
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(tid) = args.next().and_then(|s| s.parse::<u32>().ok()) else {
        std::process::exit(2);
    };
    let Some(hwnd_val) = args.next().and_then(|s| s.parse::<usize>().ok()) else {
        std::process::exit(2);
    };
    // tid==0 은 SetWindowsHookEx 에서 전역(모든 스레드) 훅을 의미한다. 본체는 항상 실제 포커스
    // 스레드 id 를 넘기므로 0 은 오용이다 — 전역 주입을 막기 위해 거부한다.
    if tid == 0 {
        std::process::exit(2);
    }

    // SAFETY: 표준 로드/주입/대기 시퀀스. 실패 경로마다 정리 후 즉시 종료(본체가 폴백).
    unsafe {
        // 이 헬퍼와 같은 폴더에서 자기-아키텍처 DLL 을 로드(배포 이름 우선, cargo 산출물 이름 폴백).
        let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        else {
            std::process::exit(1);
        };
        let mut hmod: *mut c_void = std::ptr::null_mut();
        for name in [dll_name(), "caps_hangul_tsf.dll"] {
            let path = dir.join(name);
            let wpath: Vec<u16> = path.as_os_str().encode_wide().chain(once(0)).collect();
            let h = LoadLibraryW(wpath.as_ptr());
            if !h.is_null() {
                hmod = h;
                break;
            }
        }
        if hmod.is_null() {
            std::process::exit(1);
        }

        let proc = GetProcAddress(hmod, HOOK_EXPORT.as_ptr());
        if proc.is_none() {
            std::process::exit(1);
        }
        let hookproc: HOOKPROC = transmute::<_, HOOKPROC>(proc);

        let evt_name = wide(EVT_NAME);
        let hevt = OpenEventW(EVENT_MODIFY_STATE | SYNCHRONIZE, 0, evt_name.as_ptr());
        if hevt.is_null() {
            std::process::exit(1);
        }
        ResetEvent(hevt);

        // 포커스 스레드에 리더 DLL 주입.
        let hook = SetWindowsHookExW(WH_GETMESSAGE, hookproc, hmod, tid);
        if hook.is_null() {
            // 주입 불가(AppContainer/상위 무결성 등) → 본체가 폴백.
            CloseHandle(hevt);
            std::process::exit(1);
        }

        // 대상 창에 WM_NULL 을 보내 GetMessage 를 돌려 훅(=DLL 읽기)을 발화시킨다.
        PostMessageW(hwnd_val as *mut c_void, WM_NULL, 0, 0);
        let _ = WaitForSingleObject(hevt, READ_TIMEOUT_MS);

        // 읽었든 못 읽었든 훅은 즉시 해제(상주 금지). 결과 판정은 본체가 공유 메모리로 수행한다.
        UnhookWindowsHookEx(hook);
        CloseHandle(hevt);
    }
    // DLL 이 공유 메모리에 결과를 적었다(또는 못 적었다). 본체가 이 프로세스 종료를 기다린 뒤 읽는다.
}
