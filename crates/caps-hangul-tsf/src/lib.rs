//! 포커스 스레드 in-process TSF 변환 모드 리더 (주입 DLL).
//!
//! 본체(`caps-hangul.exe`)가 `WH_GETMESSAGE` 스레드-지정 훅으로 이 DLL 을 **진짜 포커스 창의
//! 스레드**(다른 프로세스일 수 있음 — 예: WebView2 기반 Teams 의 `msedgewebview2.exe`)에 주입한다.
//! 훅 프로시저가 그 스레드에서 실행되므로, 그 스레드의 TSF 입력 컨텍스트에 in-process 로 접근해
//! 한/영 변환 모드를 정확히 읽을 수 있다(외부/cross-process 로는 불가능 — docs 참조).
//!
//! 읽기 경로:
//!   ITfThreadMgr ─QI→ ITfCompartmentMgr
//!     → GetCompartment(GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION)
//!     → GetValue() → VT_I4, IME_CMODE_NATIVE(0x1) 비트 → 1=한글 / 0=영문
//!
//! 결과는 본체가 만든 named shared memory 에 쓰고 named event 로 알린다.
//!
//! # 안전성
//! 이 코드는 **남의 프로세스(Teams 등)** 안에서 돈다. 어떤 경우에도 호스트를 죽이면 안 되므로:
//! - 훅 프로시저 본문 전체를 `catch_unwind` 로 감싼다(패닉이 FFI 경계를 넘어 abort 되는 것 차단).
//! - COM 호출은 모두 `Result` 로 받고 `unwrap`/인덱싱/할당-패닉 경로를 두지 않는다.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{Interface, GUID};
use windows::Win32::Foundation::{CloseHandle, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Memory::{
    MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
};
use windows::Win32::System::Threading::{GetCurrentThreadId, OpenEventW, SetEvent, EVENT_MODIFY_STATE};
use windows::Win32::UI::TextServices::{ITfCompartmentMgr, ITfThreadMgr, CLSID_TF_ThreadMgr};
use windows::Win32::UI::WindowsAndMessaging::CallNextHookEx;

/// 본체와 합의된 공유 메모리 레이아웃. 양쪽이 동일하게 정의해야 한다.
/// (본체: `src/ime.rs` 의 `Shared`)
#[repr(C)]
struct Shared {
    /// 매 쓰기마다 증가(본체가 갱신 여부 확인용).
    seq: u32,
    /// 읽기를 수행한 스레드 id(= 주입된 포커스 스레드).
    tid: u32,
    /// 읽기 시도 HRESULT (0 = 성공).
    hr: i32,
    /// 관측된 VARIANT 타입 태그(정상이면 VT_I4 = 3).
    vt: u32,
    /// 변환 모드 원시값.
    mode: i32,
    /// (mode & IME_CMODE_NATIVE) != 0 → 1(한글) / 0(영문).
    native: u32,
}

/// GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION = {CCF05DD8-4A87-11D7-A6E2-00065B84435C}
const GUID_KEYBOARD_INPUTMODE_CONVERSION: GUID =
    GUID::from_u128(0xCCF05DD8_4A87_11D7_A6E2_00065B84435C);

const IME_CMODE_NATIVE: i32 = 0x0001;

/// 본체와 합의된 IPC 객체 이름.
const MAP_NAME: windows::core::PCWSTR = windows::core::w!("CapsHangulImeReadMap");
const EVT_NAME: windows::core::PCWSTR = windows::core::w!("CapsHangulImeReadEvt");

/// TSF in-process 읽기. 성공 시 (vt, mode) 반환. 실패는 HRESULT 로 전달.
unsafe fn read_conversion() -> windows::core::Result<(u32, i32)> {
    // 대상 UI 스레드는 이미 COM 초기화돼 있다. 프로세스당 한 번만 재확인(보통 S_FALSE).
    static COM_INIT: AtomicBool = AtomicBool::new(false);
    if !COM_INIT.swap(true, Ordering::SeqCst) {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    // TSF thread manager 는 스레드당 싱글톤 — CoCreateInstance 는 그 스레드의 기존 인스턴스를
    // 돌려준다(우리가 Activate 하지 않으므로 호스트 TSF 상태를 건드리지 않는 읽기 전용 접근).
    let tm: ITfThreadMgr = CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)?;
    let cm: ITfCompartmentMgr = tm.cast()?;
    let comp = cm.GetCompartment(&GUID_KEYBOARD_INPUTMODE_CONVERSION)?;
    let var = comp.GetValue()?;

    // VARIANT 은 Win32 ABI 고정 레이아웃: offset 0 = vt(u16), offset 8 = lVal(i32).
    // windows-rs 의 VARIANT 모델링과 무관하게 ABI 오프셋으로 직접 읽어 안정 추출.
    let p = (&var as *const _) as *const u8;
    let vt = *(p as *const u16) as u32;
    let lval = *(p.add(8) as *const i32);
    Ok((vt, lval))
}

/// 변환 모드를 읽어 공유 메모리에 쓰고 이벤트를 신호한다(패닉 없는 본문).
unsafe fn report() {
    let Ok(hmap) = OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, MAP_NAME) else {
        return;
    };
    let view = MapViewOfFile(hmap, FILE_MAP_ALL_ACCESS, 0, 0, std::mem::size_of::<Shared>());
    if view.Value.is_null() {
        let _ = CloseHandle(hmap);
        return;
    }
    let shared = view.Value as *mut Shared;

    let mut hr = 0i32;
    let mut vt = 0u32;
    let mut mode = 0i32;
    match read_conversion() {
        Ok((v, m)) => {
            vt = v;
            mode = m;
        }
        Err(e) => hr = e.code().0,
    }

    (*shared).tid = GetCurrentThreadId();
    (*shared).hr = hr;
    (*shared).vt = vt;
    (*shared).mode = mode;
    (*shared).native = if (mode & IME_CMODE_NATIVE) != 0 { 1 } else { 0 };
    (*shared).seq = (*shared).seq.wrapping_add(1);

    if let Ok(hevt) = OpenEventW(EVENT_MODIFY_STATE, false, EVT_NAME) {
        let _ = SetEvent(hevt);
        let _ = CloseHandle(hevt);
    }
    let _ = UnmapViewOfFile(view);
    let _ = CloseHandle(hmap);
}

/// WH_GETMESSAGE 훅 프로시저. 본체가 짧게 훅을 걸고 WM_NULL 을 보내는 동안만 몇 번 호출된다.
///
/// # Safety
/// Win32 훅 콜백 규약. 본체가 `SetWindowsHookExW` 에 lpfn 으로 전달한다.
#[no_mangle]
pub unsafe extern "system" fn caps_hangul_ime_hook(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 {
        // 호스트 프로세스 보호: 어떤 패닉도 FFI 경계 밖으로 새지 않게 잡는다.
        let _ = catch_unwind(AssertUnwindSafe(|| report()));
    }
    CallNextHookEx(None, code, wparam, lparam)
}
