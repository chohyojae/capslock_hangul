//! 스파이크 #2 — in-process TSF 변환 모드 리더 (주입 DLL 측).
//!
//! 이 DLL 은 `WH_GETMESSAGE` 스레드-지정 훅으로 **포그라운드 프로세스의 UI 스레드**에
//! 주입된다. 훅 프로시저(`get_message_proc`)는 그 스레드에서 실행되므로, 그 스레드의
//! TSF 입력 컨텍스트에 in-process 로 접근할 수 있다 — 이것이 cross-process 로는 불가능했던
//! (스파이크 #1 참조) "상태가 사는 곳에서 직접 읽기"다.
//!
//! 읽기 경로(정석):
//!   ITfThreadMgr ─QI→ ITfCompartmentMgr
//!     → GetCompartment(GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION)
//!     → GetValue() → VT_I4, IME_CMODE_NATIVE(0x1) 비트 → true=한글 / false=영문
//!
//! 결과는 드라이버가 만든 named shared memory 에 쓰고, named event 를 SetEvent 하여 알린다.
//! 드라이버와 합의된 이름/레이아웃은 아래 상수 및 `Shared` 구조체로 고정한다.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{Interface, GUID};
use windows::Win32::Foundation::{CloseHandle, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Memory::{
    MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
};
use windows::Win32::System::Threading::{
    GetCurrentThreadId, OpenEventW, SetEvent, EVENT_MODIFY_STATE,
};
use windows::Win32::UI::TextServices::{
    ITfCompartmentMgr, ITfThreadMgr, CLSID_TF_ThreadMgr,
};
use windows::Win32::UI::WindowsAndMessaging::CallNextHookEx;

/// 드라이버와 합의된 공유 메모리 레이아웃. 양쪽에서 동일하게 정의해야 한다.
#[repr(C)]
struct Shared {
    /// 매 쓰기마다 증가. 드라이버가 "정말 갱신됐는지" 판별.
    seq: u32,
    /// 읽기를 수행한 스레드 id (= 주입된 대상 UI 스레드).
    tid: u32,
    /// 읽기 시도 HRESULT (0 = 성공).
    hr: i32,
    /// 관측된 VARIANT 타입 태그 (정상이면 VT_I4 = 3).
    vt: u32,
    /// 변환 모드 원시값.
    mode: i32,
    /// (mode & IME_CMODE_NATIVE) != 0 → 1(한글) / 0(영문).
    native: u32,
}

/// GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION
/// = {CCF05DD8-4A87-11D7-A6E2-00065B84435C}
const GUID_KEYBOARD_INPUTMODE_CONVERSION: GUID =
    GUID::from_u128(0xCCF05DD8_4A87_11D7_A6E2_00065B84435C);

const IME_CMODE_NATIVE: i32 = 0x0001;

/// TSF in-process 읽기. 성공 시 (vt, mode) 반환.
unsafe fn read_conversion() -> windows::core::Result<(u32, i32)> {
    // 대상 UI 스레드는 이미 COM 초기화돼 있다. 한 번만 재확인(보통 S_FALSE).
    static COM_INIT: AtomicBool = AtomicBool::new(false);
    if !COM_INIT.swap(true, Ordering::SeqCst) {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    let tm: ITfThreadMgr = CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)?;
    let cm: ITfCompartmentMgr = tm.cast()?;
    let comp = cm.GetCompartment(&GUID_KEYBOARD_INPUTMODE_CONVERSION)?;
    let var = comp.GetValue()?;

    // VARIANT 은 Win32 ABI 로 고정 레이아웃: offset 0 = vt(u16), offset 8 = lVal(i32).
    // windows-rs 의 VARIANT 모델링과 무관하게 ABI 오프셋으로 직접 읽어 안정적으로 추출.
    let p = (&var as *const _) as *const u8;
    let vt = *(p as *const u16) as u32;
    let lval = *(p.add(8) as *const i32);
    Ok((vt, lval))
}

/// 공유 메모리에 결과를 쓰고 이벤트를 신호한다.
unsafe fn report() {
    let map_name = windows::core::w!("CapsHangulTsfProbeMap");
    let evt_name = windows::core::w!("CapsHangulTsfProbeEvt");

    let Ok(hmap) = OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, map_name) else {
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

    if let Ok(hevt) = OpenEventW(EVENT_MODIFY_STATE, false, evt_name) {
        let _ = SetEvent(hevt);
        let _ = CloseHandle(hevt);
    }
    let _ = UnmapViewOfFile(view);
    let _ = CloseHandle(hmap);
}

/// WH_GETMESSAGE 훅 프로시저. 대상 스레드가 메시지를 꺼낼 때마다 호출된다.
/// 매 호출마다 변환 모드를 읽어 보고한다(스파이크라 빈도/비용은 신경쓰지 않음).
///
/// # Safety
/// Win32 훅 콜백 규약. lpfn 으로 SetWindowsHookExW 에 전달된다.
#[no_mangle]
pub unsafe extern "system" fn get_message_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 {
        report();
    }
    CallNextHookEx(None, code, wparam, lparam)
}
