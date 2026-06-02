//! 포커스 스레드 주입 기반 IME 한/영 상태 리더 (선택지 A).
//!
//! ## 배경
//! Teams(WebView2/Chromium) 같은 앱은 IMM32 cross-process 조회로 한/영 상태를 읽을 수 없고
//! (항상 0=영문 반환), TSF 변환 모드는 **포커스를 가진 스레드의 입력 컨텍스트**에만 존재한다.
//! 그 스레드는 최상위 창과 다른 프로세스일 수 있다(예: `ms-teams.exe` → `msedgewebview2.exe`).
//! 배경·접근 비교는 `README.md` 의 "IME 한/영 상태 정확 조회 (TSF 리더 DLL)" 절 참조.
//!
//! ## 동작 (on-demand)
//! 사용자가 한/영을 토글하는 그 순간에만:
//! 1. 진짜 포커스 창을 `AttachThreadInput`+`GetFocus` 로 구하고(프로세스 경계 초월),
//! 2. 그 스레드에 `WH_GETMESSAGE` 훅으로 리더 DLL(`caps-hangul-tsf`)을 잠깐 주입,
//! 3. `WM_NULL` 을 보내 훅을 깨우면 DLL 이 in-process 로 변환 모드를 읽어 공유 메모리에 쓰고
//!    이벤트를 신호,
//! 4. 결과를 읽고 즉시 훅을 해제(상시 주입/상주 없음 — 호스트 부하·발자국 최소).
//!
//! 주입이 막히는 포커스(AppContainer/상위 무결성: 예 `SearchHost.exe`)면 `None` 을 돌려주고,
//! 호출 측(overlay)이 내부 추정값으로 폴백한다.

use core::ffi::c_void;
use std::iter::once;
use std::mem::{size_of, transmute};
use std::os::windows::ffi::OsStrExt;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};
use windows_sys::Win32::System::Threading::{
    AttachThreadInput, CreateEventW, GetCurrentThreadId, ResetEvent, WaitForSingleObject,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, PostMessageW, SetWindowsHookExW,
    UnhookWindowsHookEx, HOOKPROC, WH_GETMESSAGE, WM_NULL,
};

use crate::logging;

/// 리더 DLL 과 합의된 공유 메모리 레이아웃. DLL 측(`crates/caps-hangul-tsf`)과 동일해야 한다.
#[repr(C)]
struct Shared {
    seq: u32,
    tid: u32,
    hr: i32,
    vt: u32,
    mode: i32,
    native: u32,
}

/// 리더 DLL 이 export 하는 훅 프로시저 이름(널 종료).
const HOOK_EXPORT: &[u8] = b"caps_hangul_ime_hook\0";
/// IPC 객체 이름(DLL 측 `MAP_NAME`/`EVT_NAME` 과 일치).
const MAP_NAME: &str = "CapsHangulImeReadMap";
const EVT_NAME: &str = "CapsHangulImeReadEvt";

/// DLL 이 펌프되어 값을 회신하기까지 기다리는 최대 시간(ms).
/// 정상 주입 후 읽기는 수 ms 내에 끝난다. 초과 시 폴백(None).
const READ_TIMEOUT_MS: u32 = 120;

// 메인(메시지 루프) 스레드에서만 접근한다. 핸들/포인터는 raw 이므로 atomic 으로 보관.
static DLL_MODULE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static HOOK_PROC: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static MAP_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static SHARED_VIEW: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static EVENT_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// UTF-8 → 널 종료 UTF-16.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(once(0)).collect()
}

/// exe 와 같은 폴더에서 아키텍처에 맞는 리더 DLL 을 로드한다.
/// 배포(2파일): `caps-hangul-tsf-{x64|arm64}.dll`. 개발(cargo): `caps_hangul_tsf.dll`.
unsafe fn load_dll() -> Option<*mut c_void> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
    let candidates = [format!("caps-hangul-tsf-{arch}.dll"), "caps_hangul_tsf.dll".to_string()];
    for name in candidates {
        let path = dir.join(&name);
        let wpath: Vec<u16> = path.as_os_str().encode_wide().chain(once(0)).collect();
        let h = LoadLibraryW(wpath.as_ptr());
        if !h.is_null() {
            return Some(h);
        }
    }
    None
}

/// 리더를 초기화한다(시작 시 1회). 실패해도 치명적이지 않으며, 이 경우
/// [`read_focus_conversion`] 이 항상 `None` 을 돌려 호출 측이 추정값 폴백으로 동작한다.
pub fn init() -> bool {
    // SAFETY: 표준 로드/매핑/이벤트 생성 시퀀스. 실패 경로마다 정리한다.
    unsafe {
        let Some(hmod) = load_dll() else {
            logging::log("IME 리더 DLL 로드 실패 (추정값 폴백으로 동작)");
            return false;
        };

        let proc = GetProcAddress(hmod, HOOK_EXPORT.as_ptr());
        if proc.is_none() {
            logging::log("IME 리더 DLL 에 훅 export 없음");
            return false;
        }

        let map_name = wide(MAP_NAME);
        let hmap = CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_READWRITE,
            0,
            size_of::<Shared>() as u32,
            map_name.as_ptr(),
        );
        if hmap.is_null() {
            logging::log("IME 리더 공유 메모리 생성 실패");
            return false;
        }

        let view = MapViewOfFile(hmap, FILE_MAP_ALL_ACCESS, 0, 0, size_of::<Shared>());
        if view.Value.is_null() {
            CloseHandle(hmap);
            logging::log("IME 리더 공유 메모리 매핑 실패");
            return false;
        }
        ptr::write_bytes(view.Value as *mut u8, 0, size_of::<Shared>());

        let evt_name = wide(EVT_NAME);
        // auto-reset(manual=false), 초기 non-signaled.
        let hevt = CreateEventW(ptr::null(), 0, 0, evt_name.as_ptr());
        if hevt.is_null() {
            UnmapViewOfFile(view);
            CloseHandle(hmap);
            logging::log("IME 리더 이벤트 생성 실패");
            return false;
        }

        DLL_MODULE.store(hmod, Ordering::SeqCst);
        HOOK_PROC.store(transmute::<_, *mut c_void>(proc), Ordering::SeqCst);
        MAP_HANDLE.store(hmap, Ordering::SeqCst);
        SHARED_VIEW.store(view.Value, Ordering::SeqCst);
        EVENT_HANDLE.store(hevt, Ordering::SeqCst);
        true
    }
}

/// 현재 포커스 입력의 실제 한/영 상태를 읽는다.
/// `Some(true)`=한글, `Some(false)`=영문, `None`=판단 불가(미초기화/주입 불가/타임아웃).
///
/// 메시지 루프 스레드(overlay 핸들러)에서만 호출한다.
pub fn read_focus_conversion() -> Option<bool> {
    let hmod = DLL_MODULE.load(Ordering::SeqCst);
    let raw_proc = HOOK_PROC.load(Ordering::SeqCst);
    let view = SHARED_VIEW.load(Ordering::SeqCst);
    let hevt = EVENT_HANDLE.load(Ordering::SeqCst);
    if hmod.is_null() || raw_proc.is_null() || view.is_null() || hevt.is_null() {
        return None;
    }

    // SAFETY: 위에서 유효성 확인된 핸들/포인터만 사용한다.
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() {
            return None;
        }
        let mut fg_pid = 0u32;
        let fg_tid = GetWindowThreadProcessId(fg, &mut fg_pid);
        if fg_tid == 0 {
            return None;
        }

        // 진짜 포커스 창(다른 프로세스일 수 있음)을 찾는다.
        let me = GetCurrentThreadId();
        AttachThreadInput(me, fg_tid, 1);
        let focus = GetFocus();
        AttachThreadInput(me, fg_tid, 0);
        let target = if focus.is_null() { fg } else { focus };

        let mut tpid = 0u32;
        let ttid = GetWindowThreadProcessId(target, &mut tpid);
        if ttid == 0 {
            return None;
        }

        // 포커스 스레드에 리더 DLL 을 잠깐 주입.
        let hookproc: HOOKPROC = transmute::<*mut c_void, HOOKPROC>(raw_proc);
        let hook = SetWindowsHookExW(WH_GETMESSAGE, hookproc, hmod, ttid);
        if hook.is_null() {
            // 주입 불가(AppContainer/상위 무결성 등) → 폴백.
            return None;
        }

        let shared = view as *mut Shared;
        ResetEvent(hevt);
        // WM_NULL 로 대상 스레드가 메시지를 꺼내게 해 훅(=DLL 읽기)을 발화시킨다.
        PostMessageW(target, WM_NULL, 0, 0);
        let waited = WaitForSingleObject(hevt, READ_TIMEOUT_MS);

        // 읽었든 못 읽었든 훅은 즉시 해제(상주 금지).
        UnhookWindowsHookEx(hook);

        if waited != WAIT_OBJECT_0 {
            return None;
        }
        let hr = (*shared).hr;
        let vt = (*shared).vt;
        let native = (*shared).native;
        // vt=3(VT_I4) 이고 hr=0 일 때만 신뢰. (VT_EMPTY 등은 상태 부재 → 폴백.)
        if hr == 0 && vt == 3 {
            Some(native != 0)
        } else {
            None
        }
    }
}

/// 종료 시 정리(best-effort). per-toggle 로 훅을 해제하므로 상주 훅은 없다.
pub fn shutdown() {
    // SAFETY: 보관된 유효 핸들만 한 번씩 정리한다.
    unsafe {
        let view = SHARED_VIEW.swap(ptr::null_mut(), Ordering::SeqCst);
        if !view.is_null() {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: view });
        }
        let hevt = EVENT_HANDLE.swap(ptr::null_mut(), Ordering::SeqCst);
        if !hevt.is_null() {
            CloseHandle(hevt);
        }
        let hmap = MAP_HANDLE.swap(ptr::null_mut(), Ordering::SeqCst);
        if !hmap.is_null() {
            CloseHandle(hmap);
        }
    }
}
