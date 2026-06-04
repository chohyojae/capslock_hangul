//! 포커스 스레드 주입 기반 IME 한/영 상태 리더 (선택지 A) — 비트니스 라우팅 포함.
//!
//! ## 배경
//! Teams(WebView2/Chromium) 같은 앱은 IMM32 cross-process 조회로 한/영 상태를 읽을 수 없고
//! (항상 0=영문 반환), TSF 변환 모드는 **포커스를 가진 스레드의 입력 컨텍스트**에만 존재한다.
//! 그 스레드는 최상위 창과 다른 프로세스일 수 있다(예: `ms-teams.exe` → `msedgewebview2.exe`).
//! 배경·접근 비교는 `README.md` 의 "IME 한/영 상태 정확 조회 (TSF 리더 DLL)" 절 참조.
//!
//! ## 비트니스 제약 (왜 헬퍼가 필요한가)
//! `SetWindowsHookEx` 는 **호출 프로세스·주입 DLL·대상 프로세스의 비트니스가 모두 같아야** 한다
//! (DLL 이 호출자에 먼저 `LoadLibrary` 되고 그 in-process 훅 프로시저 주소를 넘기기 때문).
//! 따라서 본체 exe 와 **같은 비트니스 포커스**는 본체가 직접(in-process) 읽고, **다른 비트니스
//! 포커스**(예: x64 본체 + 32비트 앱)는 그 비트니스로 빌드된 헬퍼(`caps-hangul-reader-<arch>.exe`)를
//! 실행해 주입을 위임한다. 어느 경로든 결과는 본체가 만든 공유 메모리에 적힌다.
//!
//! ## 동작 (on-demand)
//! 사용자가 한/영을 토글하는 그 순간에만:
//! 1. 진짜 포커스 창을 `AttachThreadInput`+`GetFocus` 로 구하고(프로세스 경계 초월),
//! 2. 그 프로세스의 아키텍처를 `IsWow64Process2` 로 판별,
//! 3a. 본체와 같으면 그 스레드에 리더 DLL 을 직접 주입(in-process, 빠른 경로),
//! 3b. 다르면 같은-비트니스 헬퍼 exe 를 잠깐 실행해 주입을 위임,
//! 4. DLL 이 in-process 로 변환 모드를 읽어 공유 메모리에 쓰면 그 값을 읽고 즉시 정리(상주 없음).
//!
//! 주입이 막히는 포커스(AppContainer/상위 무결성: 예 `SearchHost.exe`)면 `None` 을 돌려주고,
//! 호출 측(overlay)이 내부 추정값으로 폴백한다.

use core::ffi::c_void;
use std::iter::once;
use std::mem::{size_of, transmute};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};
use windows_sys::Win32::System::Threading::{
    AttachThreadInput, CreateEventW, GetCurrentThreadId, IsWow64Process2, OpenProcess, ResetEvent,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, PostMessageW, SetWindowsHookExW,
    UnhookWindowsHookEx, HOOKPROC, WH_GETMESSAGE, WM_NULL,
};

use crate::logging;

/// 리더 DLL 과 합의된 공유 메모리 레이아웃. DLL 측(`crates/caps-hangul-tsf`)과 동일해야 한다.
/// 모든 필드가 4바이트 고정이라 x86/x64/arm64 간 레이아웃이 일치한다(cross-arch IPC 안전).
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
/// IPC 객체 이름(DLL/헬퍼 측과 일치).
const MAP_NAME: &str = "CapsHangulImeReadMap";
const EVT_NAME: &str = "CapsHangulImeReadEvt";

/// in-process 읽기에서 DLL 회신을 기다리는 최대 시간(ms). 정상 시 수 ms 내 완료.
const READ_TIMEOUT_MS: u32 = 120;
/// cross-arch 헬퍼(자식 프로세스)의 spawn+주입+읽기 전체를 기다리는 최대 시간(ms).
const SIDECAR_TIMEOUT_MS: u32 = 400;

/// OpenProcess 권한(아키텍처 조회용). stable ABI 값.
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
/// CreateProcess 플래그: 콘솔 창을 띄우지 않음. stable ABI 값.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// IMAGE_FILE_MACHINE_* (stable ABI 값 — 모듈/피처 의존을 피해 직접 정의).
const IMAGE_FILE_MACHINE_I386: u16 = 0x014C;
const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
const IMAGE_FILE_MACHINE_ARM64: u16 = 0xAA64;

// 메인(메시지 루프) 스레드에서만 접근한다. 핸들/포인터는 raw 이므로 atomic 으로 보관.
static DLL_MODULE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static HOOK_PROC: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static MAP_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static SHARED_VIEW: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static EVENT_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// 본체(이 exe) 자신의 아키텍처 접미사.
fn own_arch() -> &'static str {
    if cfg!(target_arch = "x86") {
        "x86"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    }
}

/// 머신 코드 → 아키텍처 접미사("x86"/"x64"/"arm64"). 모르는 값은 None.
fn machine_arch(machine: u16) -> Option<&'static str> {
    match machine {
        IMAGE_FILE_MACHINE_I386 => Some("x86"),
        IMAGE_FILE_MACHINE_AMD64 => Some("x64"),
        IMAGE_FILE_MACHINE_ARM64 => Some("arm64"),
        _ => None,
    }
}

/// 대상 프로세스의 실행 아키텍처를 접미사로 판별. 핸들 획득 실패/미지원이면 None.
unsafe fn process_arch(pid: u32) -> Option<&'static str> {
    let hproc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if hproc.is_null() {
        return None;
    }
    let mut proc_machine: u16 = 0;
    let mut native_machine: u16 = 0;
    let ok = IsWow64Process2(hproc, &mut proc_machine, &mut native_machine);
    CloseHandle(hproc);
    if ok == 0 {
        return None;
    }
    // proc_machine == UNKNOWN(0) → 네이티브 실행 → native_machine 이 곧 대상 아키텍처.
    let machine = if proc_machine == 0 {
        native_machine
    } else {
        proc_machine
    };
    machine_arch(machine)
}

/// exe 와 같은 폴더에서 본체 아키텍처에 맞는 리더 DLL 을 로드한다(in-process 경로용).
/// 배포: `caps-hangul-tsf-{x86|x64|arm64}.dll`. 개발(cargo): `caps_hangul_tsf.dll`.
unsafe fn load_dll() -> Option<*mut c_void> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidates = [
        format!("caps-hangul-tsf-{}.dll", own_arch()),
        "caps_hangul_tsf.dll".to_string(),
    ];
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

/// 리더를 초기화한다(시작 시 1회).
///
/// 1) IPC 객체(공유 메모리 + 이벤트)를 만든다 — in-process·헬퍼 경로 모두 이걸 공유한다.
/// 2) 본체 자기-아키텍처 DLL 을 로드한다(있으면 같은-비트니스 in-process 빠른 경로 활성화).
///    DLL 이 없어도 IPC 만 있으면 다른-비트니스 헬퍼 경로는 동작하므로 치명적이지 않다.
///
/// IPC 생성에 실패하면 false(이 경우 [`read_focus_conversion`] 이 항상 `None`).
pub fn init() -> bool {
    // SAFETY: 표준 매핑/이벤트 생성 시퀀스. 실패 경로마다 정리한다.
    unsafe {
        let hmap = CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_READWRITE,
            0,
            size_of::<Shared>() as u32,
            w!(MAP_NAME),
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

        // auto-reset(manual=false), 초기 non-signaled.
        let hevt = CreateEventW(ptr::null(), 0, 0, w!(EVT_NAME));
        if hevt.is_null() {
            UnmapViewOfFile(view);
            CloseHandle(hmap);
            logging::log("IME 리더 이벤트 생성 실패");
            return false;
        }

        MAP_HANDLE.store(hmap, Ordering::SeqCst);
        SHARED_VIEW.store(view.Value, Ordering::SeqCst);
        EVENT_HANDLE.store(hevt, Ordering::SeqCst);

        // 본체 자기-아키텍처 DLL(in-process 경로). 실패해도 헬퍼 경로로 동작.
        match load_dll() {
            Some(hmod) => match GetProcAddress(hmod, HOOK_EXPORT.as_ptr()) {
                Some(p) => {
                    DLL_MODULE.store(hmod, Ordering::SeqCst);
                    HOOK_PROC.store(transmute::<_, *mut c_void>(p), Ordering::SeqCst);
                }
                None => logging::log("IME 리더 DLL 에 훅 export 없음 (같은-비트니스 in-process 비활성)"),
            },
            None => logging::log(
                "본체 아키텍처 리더 DLL 없음 (같은-비트니스 in-process 비활성, 다른-비트니스 헬퍼는 동작)",
            ),
        }

        true
    }
}

/// 현재 포커스 입력의 실제 한/영 상태를 읽는다.
/// `Some(true)`=한글, `Some(false)`=영문, `None`=판단 불가(미초기화/주입 불가/타임아웃).
///
/// 메시지 루프 스레드(overlay 핸들러)에서만 호출한다.
pub fn read_focus_conversion() -> Option<bool> {
    let view = SHARED_VIEW.load(Ordering::SeqCst);
    let hevt = EVENT_HANDLE.load(Ordering::SeqCst);
    if view.is_null() || hevt.is_null() {
        return None; // IPC 미준비.
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

        // 대상 프로세스 비트니스가 본체와 다르면 같은-비트니스 헬퍼로 위임한다.
        if let Some(arch) = process_arch(tpid) {
            if arch != own_arch() {
                return read_via_sidecar(arch, ttid, target, view);
            }
        }

        // 같은 비트니스(또는 판별 불가) → 본체 in-process 경로(빠름). 자기-아키텍처 DLL 필요.
        let hmod = DLL_MODULE.load(Ordering::SeqCst);
        let raw_proc = HOOK_PROC.load(Ordering::SeqCst);
        if hmod.is_null() || raw_proc.is_null() {
            return None; // 본체 DLL 없음 → 폴백.
        }

        let hookproc: HOOKPROC = transmute::<*mut c_void, HOOKPROC>(raw_proc);
        let hook = SetWindowsHookExW(WH_GETMESSAGE, hookproc, hmod, ttid);
        if hook.is_null() {
            return None; // 주입 불가(AppContainer/상위 무결성 등) → 폴백.
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
        interpret(shared)
    }
}

/// 다른-비트니스 포커스를 위해 같은-비트니스 헬퍼 exe 를 잠깐 실행해 주입을 위임한다.
/// 헬퍼가 DLL 을 깨워 공유 메모리에 값을 쓰면, 헬퍼 종료를 기다린 뒤 그 값을 읽는다.
fn read_via_sidecar(arch: &str, tid: u32, hwnd: *mut c_void, view: *mut c_void) -> Option<bool> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    // 배포 이름(아키텍처 접미사) 우선, cargo 산출물 이름 폴백.
    let candidates = [
        dir.join(format!("caps-hangul-reader-{arch}.exe")),
        dir.join("caps-hangul-reader.exe"),
    ];

    // SAFETY: view 는 init 에서 매핑한 유효 포인터.
    unsafe {
        let shared = view as *mut Shared;
        let old_seq = (*shared).seq;

        let mut child = None;
        for path in &candidates {
            match Command::new(path)
                .arg(tid.to_string())
                .arg((hwnd as usize).to_string())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
            {
                Ok(c) => {
                    child = Some(c);
                    break;
                }
                Err(_) => continue,
            }
        }
        let mut child = child?;

        // 자식 종료를 타임아웃과 함께 대기(헬퍼가 주입·읽기·언훅까지 마치고 종료).
        let raw = child.as_raw_handle() as *mut c_void;
        let waited = WaitForSingleObject(raw, SIDECAR_TIMEOUT_MS);
        if waited != WAIT_OBJECT_0 {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        let _ = child.wait();

        // DLL 이 공유 메모리에 새 값을 적었는지(seq 변화) 확인 후 해석.
        if (*shared).seq == old_seq {
            return None;
        }
        interpret(shared)
    }
}

/// 공유 메모리의 회신을 한/영 결과로 해석한다.
/// vt=3(VT_I4) 이고 hr=0 일 때만 신뢰. (VT_EMPTY 등은 상태 부재 → None → 폴백.)
unsafe fn interpret(shared: *mut Shared) -> Option<bool> {
    let hr = (*shared).hr;
    let vt = (*shared).vt;
    let native = (*shared).native;
    if hr == 0 && vt == 3 {
        Some(native != 0)
    } else {
        None
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
