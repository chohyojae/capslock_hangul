//! 스파이크 #2b — 주입 드라이버 (포커스 스레드 추적판).
//!
//! 스파이크 #2 결과: 포그라운드 *최상위* 창의 스레드(ms-teams.exe tid=7072)에 주입해
//! 변환 모드 compartment 를 읽으면 **VT_EMPTY** 였다. 즉 그 스레드엔 IME 상태가 없다.
//! 새 Teams 는 WebView2 기반이라 실제 포커스 입력은 **자식 msedgewebview2.exe** 스레드에 있다.
//! (대조군 explorer.exe 는 VT_I4 로 토글을 정확히 추종 → 주입·읽기 메커니즘 자체는 정상.)
//!
//! 그래서 이 판은 최상위 창이 아니라 **진짜 포커스 창의 스레드**(다른 프로세스일 수 있음)를
//! `AttachThreadInput` + `GetFocus` 로 찾아 거기에 주입한다. 포그라운드/포커스 토폴로지를 함께
//! 출력해 상태가 실제로 어느 프로세스·스레드에 사는지 눈으로 확인한다.
//!
//! 판정:
//!   - 포커스 스레드(예: msedgewebview2.exe) 주입에서 native 가 Teams 한/영 토글을 정확히
//!     추종하고 vt=3 이면 → ✅ A 성립 + 해법 경로 확정(포커스 스레드 타게팅).
//!
//! 빌드/실행 (spikes/ 안에서): cargo run -p inject_driver   |   Ctrl+C 종료.

use std::thread;
use std::time::Duration;

use windows::core::{s, w, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HINSTANCE, HWND, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, FILE_MAP_ALL_ACCESS, PAGE_READWRITE,
};
use windows::Win32::System::Threading::{
    AttachThreadInput, CreateEventW, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
    ResetEvent, WaitForSingleObject, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, PostMessageW, SetWindowsHookExW,
    UnhookWindowsHookEx, HHOOK, HOOKPROC, WH_GETMESSAGE, WM_NULL,
};

/// DLL 과 합의된 공유 메모리 레이아웃 (tsf_probe_dll 의 Shared 와 동일해야 함).
#[repr(C)]
#[derive(Clone, Copy)]
struct Shared {
    seq: u32,
    tid: u32,
    hr: i32,
    vt: u32,
    mode: i32,
    native: u32,
}

fn wstr_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// pid → 실행 파일 베이스 이름.
unsafe fn exe_name(pid: u32) -> String {
    if pid == 0 {
        return format!("pid:{pid}");
    }
    if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
        let mut buf = [0u16; 260];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size);
        let _ = CloseHandle(h);
        if ok.is_ok() {
            let full = wstr_to_string(&buf[..size as usize]);
            return full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string();
        }
    }
    format!("pid:{pid}")
}

/// 창의 클래스 이름.
unsafe fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = GetClassNameW(hwnd, &mut buf);
    if n > 0 {
        wstr_to_string(&buf[..n as usize])
    } else {
        String::new()
    }
}

/// 진짜 포커스 창과 그 (tid, pid) 를 구한다. cross-process 포커스(WebView2 등)도 추적.
/// 실패 시 최상위 포그라운드 창으로 폴백.
unsafe fn focus_target(fg: HWND, fg_tid: u32) -> (HWND, u32, u32) {
    let me = GetCurrentThreadId();
    let _ = AttachThreadInput(me, fg_tid, true);
    let focus = GetFocus();
    let _ = AttachThreadInput(me, fg_tid, false);

    let target = if focus.0.is_null() { fg } else { focus };
    let mut pid = 0u32;
    let tid = GetWindowThreadProcessId(target, Some(&mut pid));
    (target, tid, pid)
}

fn main() -> windows::core::Result<()> {
    unsafe {
        let size = std::mem::size_of::<Shared>();

        let hmap = CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            None,
            PAGE_READWRITE,
            0,
            size as u32,
            w!("CapsHangulTsfProbeMap"),
        )?;
        let view = MapViewOfFile(hmap, FILE_MAP_ALL_ACCESS, 0, 0, size);
        if view.Value.is_null() {
            eprintln!("MapViewOfFile 실패");
            return Ok(());
        }
        let shared = view.Value as *mut Shared;
        std::ptr::write_bytes(shared as *mut u8, 0, size);

        let hevt = CreateEventW(None, false, false, w!("CapsHangulTsfProbeEvt"))?;

        let hmod = LoadLibraryW(w!("tsf_probe_dll.dll"))?;
        let proc = GetProcAddress(hmod, s!("get_message_proc"));
        if proc.is_none() {
            eprintln!("get_message_proc export 를 찾지 못함");
            return Ok(());
        }
        let hookproc: HOOKPROC = std::mem::transmute(proc);
        let hinst = HINSTANCE(hmod.0);

        println!("=== 스파이크 #2b: 포커스 스레드 추적 + in-process TSF 읽기 ===");
        println!("Teams 검색/메시지창을 클릭하고 한/영을 토글하며 native 가 따라가는지 보세요.");
        println!("형식: [N] fg={{최상위}} focus={{진짜포커스}}(class) → native/mode/vt/hr. Ctrl+C 종료.\n");

        let mut hooked_tid: u32 = 0;
        let mut hook = HHOOK::default();

        let mut tick = 0u64;
        loop {
            let fg: HWND = GetForegroundWindow();
            if fg.0.is_null() {
                println!("[{tick}] fg=<none>");
                tick += 1;
                thread::sleep(Duration::from_millis(800));
                continue;
            }
            let mut fg_pid = 0u32;
            let fg_tid = GetWindowThreadProcessId(fg, Some(&mut fg_pid));
            let fg_name = exe_name(fg_pid);

            // 진짜 포커스 창(다른 프로세스일 수 있음)을 찾는다.
            let (focus_hwnd, tid, pid) = focus_target(fg, fg_tid);
            let focus_name = exe_name(pid);
            let focus_class = class_name(focus_hwnd);
            let topo = format!(
                "fg={fg_name}(t{fg_tid}) focus={focus_name}(t{tid},\"{focus_class}\")"
            );

            // 포커스 스레드가 바뀌면 재-훅.
            if tid != hooked_tid {
                if !hook.is_invalid() {
                    let _ = UnhookWindowsHookEx(hook);
                }
                match SetWindowsHookExW(WH_GETMESSAGE, hookproc, Some(hinst), tid) {
                    Ok(h) => {
                        hook = h;
                        hooked_tid = tid;
                    }
                    Err(e) => {
                        hook = HHOOK::default();
                        hooked_tid = 0;
                        println!("[{tick}] {topo} 훅 설치 실패: {e}");
                        tick += 1;
                        thread::sleep(Duration::from_millis(800));
                        continue;
                    }
                }
            }

            let _ = ResetEvent(hevt);
            let prev_seq = (*shared).seq;
            // 포커스 창에 WM_NULL 을 보내 그 스레드가 메시지를 꺼내게 한다.
            let _ = PostMessageW(
                Some(focus_hwnd),
                WM_NULL,
                windows::Win32::Foundation::WPARAM(0),
                windows::Win32::Foundation::LPARAM(0),
            );

            let waited = WaitForSingleObject(hevt, 300);
            if waited == WAIT_OBJECT_0 {
                let s = *shared;
                let label = match (s.hr, s.vt, s.native) {
                    (0, 3, 1) => "한글(NATIVE)",
                    (0, 3, 0) => "영문",
                    (0, 0, _) => "VT_EMPTY(이 스레드에 상태 없음)",
                    _ => "읽기실패/기타",
                };
                println!(
                    "[{tick}] {topo} → native={} mode=0x{:x} vt={} hr=0x{:08x} [{}] (seq {}→{})",
                    s.native, s.mode, s.vt, s.hr as u32, label, prev_seq, s.seq
                );
            } else {
                println!(
                    "[{tick}] {topo} → 응답 없음(주입 안됨/펌프 안함). seq {}→{}",
                    prev_seq,
                    (*shared).seq
                );
            }

            tick += 1;
            thread::sleep(Duration::from_millis(800));
        }
    }
}
