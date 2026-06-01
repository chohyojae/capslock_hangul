//! TSF cross-process 읽기 스파이크 (#1).
//!
//! 목적: `ITfLangBarMgr::GetThreadLangBarItemMgr` 로 **포그라운드(Teams 등) 스레드**의
//!       입력 모드(한/영) 상태를 우리 같은 **외부 프로세스**에서 읽을 수 있는지 실측한다.
//!       성공하면 이것이 IMM32 를 대체할 TSF-native cross-process 조회의 근본 해법이 된다.
//!
//! 빌드/실행:
//!   cargo run --example tsf_spike
//!
//! 사용법:
//!   1. 실행하면 0.8초마다 현재 포그라운드 창의 langbar 아이템을 덤프한다.
//!   2. **Teams 로 포커스를 옮긴 뒤** 한/영을 토글(Caps Lock 또는 우-Alt)하면서
//!      콘솔의 INPUTMODE text 가 "한" ↔ "A"(또는 "가"/"영문") 로 따라 바뀌는지 관찰한다.
//!   3. 판정:
//!      - 따라 바뀌면  → ✅ TSF langbar cross-process 읽기 성공 (근본 해법 가능)
//!      - 항상 같거나 빈 문자열 → ❌ Chromium 에서 langbar 미노출 (이 경로 실패)
//!      - GetThreadLangBarItemMgr 자체가 에러 → ❌ 경로 자체 불가
//!   4. 비교군으로 메모장(Notepad)·워드패드 등 네이티브 앱에서도 같은지 확인한다.
//!
//! Ctrl+C 로 종료.

use std::thread;
use std::time::Duration;

use windows::core::Interface;
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::TextServices::{
    IEnumTfLangBarItems, ITfLangBarItem, ITfLangBarItemButton, ITfLangBarItemMgr, ITfLangBarMgr,
    ITfThreadMgr, CLSID_TF_LangBarMgr, CLSID_TF_ThreadMgr, GUID_LBI_INPUTMODE, TF_LANGBARITEMINFO,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

fn wstr_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// 포그라운드 창의 (프로세스 베이스 이름, pid, tid) 를 구한다.
unsafe fn foreground_info(hwnd: HWND) -> (String, u32, u32) {
    let mut pid: u32 = 0;
    let tid = GetWindowThreadProcessId(hwnd, Some(&mut pid));

    let mut name = format!("pid:{pid}");
    if pid != 0 {
        if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            let mut buf = [0u16; 260];
            let mut size = buf.len() as u32;
            let pw = windows::core::PWSTR(buf.as_mut_ptr());
            if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, pw, &mut size).is_ok() {
                let full = wstr_to_string(&buf[..size as usize]);
                name = full
                    .rsplit(['\\', '/'])
                    .next()
                    .unwrap_or(&full)
                    .to_string();
            }
            let _ = CloseHandle(h);
        }
    }
    (name, pid, tid)
}

/// enum 의 아이템 개수를 센다(baseline 자가 점검용).
unsafe fn count_items(e: &IEnumTfLangBarItems) -> usize {
    let mut n = 0usize;
    loop {
        let mut slot: [Option<ITfLangBarItem>; 1] = [None];
        let mut fetched: u32 = 0;
        if e.Next(&mut slot, &mut fetched).is_err() || fetched == 0 {
            break;
        }
        n += 1;
    }
    n
}

/// 자기 스레드(tid=0) baseline: 호출 규약이 정상인지 + in-process 동작 확인.
/// 이게 성공하면, 다른 프로세스 tid 에서의 E_FAIL 은 진짜 cross-process 경계 거부다.
unsafe fn self_check(mgr: &ITfLangBarMgr) {
    let mut im: Option<ITfLangBarItemMgr> = None;
    let mut out_tid: u32 = 0;
    match mgr.GetThreadLangBarItemMgr(0, &mut im, &mut out_tid) {
        Ok(()) => {
            let n = im
                .as_ref()
                .and_then(|m| m.EnumItems().ok())
                .map(|e| count_items(&e));
            match n {
                Some(c) => println!(
                    "[self-check] GetThreadLangBarItemMgr(0)=OK (our thread), out_tid={out_tid}, items={c} \
                     → 호출 규약 정상. cross-process E_FAIL 은 경계 거부로 해석."
                ),
                None => println!(
                    "[self-check] GetThreadLangBarItemMgr(0)=OK 이나 EnumItems 실패 (out_tid={out_tid})"
                ),
            }
        }
        Err(e) => println!("[self-check] GetThreadLangBarItemMgr(0)=ERR {e} (우리 스레드엔 TSF 입력이 없어 실패일 수 있음)"),
    }
}

/// langbar 아이템 매니저를 열거해 (입력모드 텍스트, 입력모드 status, 전체 요약) 반환.
unsafe fn read_items(im: &ITfLangBarItemMgr) -> (String, u32, Vec<String>) {
    let mut inputmode_text = String::from("<none>");
    let mut inputmode_status = 0u32;
    let mut all = Vec::new();
    let Ok(enum_items) = im.EnumItems() else {
        return (inputmode_text, inputmode_status, all);
    };
    loop {
        let mut slot: [Option<ITfLangBarItem>; 1] = [None];
        let mut fetched = 0u32;
        if enum_items.Next(&mut slot, &mut fetched).is_err() || fetched == 0 {
            break;
        }
        let Some(item) = slot[0].take() else { break };
        let mut info = TF_LANGBARITEMINFO::default();
        if item.GetInfo(&mut info).is_err() {
            continue;
        }
        let desc = wstr_to_string(&info.szDescription);
        let status = item.GetStatus().unwrap_or(0);
        let text = item
            .cast::<ITfLangBarItemButton>()
            .ok()
            .and_then(|b| b.GetText().ok())
            .map(|b| b.to_string())
            .unwrap_or_default();
        let is_inputmode = info.guidItem == GUID_LBI_INPUTMODE;
        if is_inputmode {
            inputmode_text = if text.is_empty() { "<empty>".into() } else { text.clone() };
            inputmode_status = status;
        }
        let tag = if is_inputmode { "*IM*" } else { "" };
        all.push(format!("{tag}\"{desc}\"=\"{text}\"(0x{status:x})"));
    }
    (inputmode_text, inputmode_status, all)
}

/// 우리 자신 스레드(in-process)의 입력모드를 읽는다. 전역 동기화 여부 판별의 핵심.
unsafe fn read_own(tm: &ITfThreadMgr) -> String {
    match tm.cast::<ITfLangBarItemMgr>() {
        Ok(im) => {
            let (txt, st, all) = read_items(&im);
            format!("OWN inputmode=\"{txt}\"(0x{st:x}) all=[{}]", all.join(" "))
        }
        Err(e) => format!("OWN QI ERR: {e}"),
    }
}

/// 포그라운드 스레드의 입력모드를 cross-process 로 읽기 시도한 요약 문자열.
unsafe fn fg_summary(mgr: &ITfLangBarMgr) -> String {
    let hwnd = GetForegroundWindow();
    if hwnd.0.is_null() {
        return "fg=<none>".into();
    }
    let (exe, _pid, tid) = foreground_info(hwnd);
    let mut item_mgr: Option<ITfLangBarItemMgr> = None;
    let mut out_tid: u32 = 0;
    match mgr.GetThreadLangBarItemMgr(tid, &mut item_mgr, &mut out_tid) {
        Err(_) => format!("fg={exe}(tid={tid}) xproc=E_FAIL"),
        Ok(()) => match item_mgr {
            None => format!("fg={exe}(tid={tid}) xproc=OK-but-None"),
            Some(im) => {
                let (txt, st, _all) = read_items(&im);
                format!("fg={exe}(tid={tid}) xproc=OK inputmode=\"{txt}\"(0x{st:x})")
            }
        },
    }
}

fn main() -> windows::core::Result<()> {
    unsafe {
        // TSF 는 STA. 외부(outgoing) 호출만 하므로 메시지 펌프 없이도 마샬링은 COM 이 처리한다.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let mgr: ITfLangBarMgr = CoCreateInstance(&CLSID_TF_LangBarMgr, None, CLSCTX_INPROC_SERVER)?;

        println!("=== TSF langbar cross-process 읽기 스파이크 ===");

        // 양성 대조(positive control): 우리 스레드에 TSF thread manager 를 활성화한 뒤
        // tid=0(자기 스레드)를 읽어 본다. 이게 OK 면 호출 규약은 확정 정상이고,
        // 그 뒤 다른 프로세스 tid 에서의 E_FAIL 은 진짜 cross-process 경계 거부다.
        // `tm` 은 프로그램 끝까지 살려 둬 활성 상태를 유지하고, 매 틱 OWN 읽기에 쓴다.
        let tm: Option<ITfThreadMgr> =
            match CoCreateInstance::<_, ITfThreadMgr>(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER) {
                Ok(tm) => match tm.Activate() {
                    Ok(cid) => {
                        println!("[self-check] ITfThreadMgr.Activate OK (client_id={cid})");
                        Some(tm)
                    }
                    Err(e) => {
                        println!("[self-check] ITfThreadMgr.Activate ERR: {e}");
                        Some(tm)
                    }
                },
                Err(e) => {
                    println!("[self-check] CoCreateInstance(ThreadMgr) ERR: {e}");
                    None
                }
            };

        // 교과서적 in-process 경로: ITfThreadMgr 를 ITfLangBarItemMgr 로 QI 하여 직접 열거.
        // 이게 OK(아이템 수 무관)면 langbar 메커니즘은 in-process 로 도달 가능 → cross-process
        // GetThreadLangBarItemMgr 의 E_FAIL 은 진짜 프로세스 경계 거부임이 확정된다.
        match tm.as_ref().and_then(|t| t.cast::<ITfLangBarItemMgr>().ok()) {
            Some(im) => match im.EnumItems() {
                Ok(e) => println!(
                    "[self-check] ITfThreadMgr→ITfLangBarItemMgr QI OK, in-proc items={}",
                    count_items(&e)
                ),
                Err(e) => println!("[self-check] QI OK 이나 EnumItems ERR: {e}"),
            },
            None => println!("[self-check] ITfThreadMgr→ITfLangBarItemMgr QI 실패"),
        }

        self_check(&mgr);
        println!("Teams 로 포커스를 옮긴 뒤 한/영을 토글하며 INPUTMODE text 가 따라 바뀌는지 보세요.");
        println!("클래식 Win32 대조군: Win+R 실행창 / regedit 찾기창 / 탐색기 주소창. Ctrl+C 로 종료.\n");

        let mut tick = 0u64;
        loop {
            let fg = fg_summary(&mgr);
            let own = tm
                .as_ref()
                .map(|t| read_own(t))
                .unwrap_or_else(|| "OWN=<no tm>".into());
            println!("[{tick}] {fg} | {own}");
            tick += 1;
            thread::sleep(Duration::from_millis(800));
        }
    }
}
