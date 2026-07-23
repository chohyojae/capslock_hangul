//! 새 버전 릴리스 확인 (GitHub Releases, 시작 후 1회 lazy 체크).
//!
//! ## 설계 요점 — "비용 제로" 원칙에 맞춘 선택
//! - **의존성 0**: HTTP 클라이언트 크레이트 대신 OS 내장 WinHTTP(`winhttp.dll`)를 직접
//!   호출한다(windows-sys 피처 한 줄 추가). JSON 파서도 불필요 — GitHub API 대신
//!   `github.com/<owner>/<repo>/releases/latest` 가 최신 릴리스 페이지
//!   `…/releases/tag/<태그>` 로 302 리다이렉트하는 동작을 이용해, **리다이렉트를 끈
//!   HEAD 요청 한 번**으로 Location 헤더에서 태그만 읽는다. 응답 본문은 내려받지 않고
//!   API 사용량 제한(미인증 60회/시)도 받지 않는다.
//!   (릴리스가 하나도 없으면 `…/releases` 목록으로 리다이렉트 → 태그 없음 → 조용히 무시.)
//! - **메모리**: 확인은 짧게 사는 백그라운드 스레드 1개가 수행하고 끝나면 사라진다.
//!   상주 결과는 `OnceLock<String>`(태그 문자열 하나)뿐이다. 완료 통지
//!   [`WM_UPDATE_CHECKED`] 를 받은 트레이 창이 작업 집합을 다시 트림해, WinHTTP
//!   초기화로 올라온 1회성 페이지를 idle 복귀 전에 내려보낸다.
//! - **CPU**: 트레이 타이머 1회 → 요청 1회 → 끝. 폴링/재시도 없음. 실패(오프라인,
//!   릴리스 없음, 태그 형식 불일치)는 전부 조용히 무시한다 — 알림은 부가 기능이므로
//!   본체 동작에 영향을 주지 않는다.
//! - **자동 업데이트는 의도적으로 제공하지 않는다**: 다운로더/설치기 코드가 상주하면
//!   절대목표(메모리 최소화)에 반한다. 알림 UI(트레이 메뉴 항목 + About 다이얼로그
//!   링크)는 tray.rs 가 [`available_version`] 을 읽어 그리고, 클릭 시 브라우저로
//!   [`LATEST_RELEASE_URL`] 만 연다.

use core::ffi::c_void;
use std::mem::size_of;
use std::ptr;
use std::sync::OnceLock;

use windows_sys::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
    WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption, WinHttpSetTimeouts,
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_DISABLE_REDIRECTS,
    WINHTTP_FLAG_SECURE, WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_QUERY_LOCATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// 확인 스레드가 끝났을 때 트레이 숨김 창으로 보내는 통지. wParam=1 이면 새 버전 발견.
/// (tray.rs 의 `WM_TRAY` = WM_APP+0x20 다음 값. 트레이 창 전용이라 충돌 없음.)
pub const WM_UPDATE_CHECKED: u32 = WM_APP + 0x21;

/// 브라우저로 열어 줄 최신 릴리스 페이지. tray.rs 의 `REPO_URL` 과 같은 저장소를 가리켜야 한다.
pub const LATEST_RELEASE_URL: &str =
    "https://github.com/chohyojae/capslock_hangul/releases/latest";
/// HEAD 요청 대상(위 URL 의 호스트/경로 분해). [`LATEST_RELEASE_URL`] 과 함께 갱신할 것.
const HOST: &str = "github.com";
const PATH: &str = "/chohyojae/capslock_hangul/releases/latest";

/// 네트워크 타임아웃(resolve/connect/send/receive 공통, ms). 확인은 부가 기능이므로
/// 오래 기다리지 않는다(스레드가 이 시간 안에 반드시 끝나 자원을 돌려주게 하는 상한).
const TIMEOUT_MS: i32 = 10_000;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 발견된 새 버전 태그(예: "v0.1.5"). 현재 버전보다 새로울 때만 기록된다.
static LATEST_TAG: OnceLock<String> = OnceLock::new();

/// 새 버전이 확인됐으면 그 태그(예: "v0.1.5")를 돌려준다. UI(트레이 메뉴/About)가 읽는다.
pub fn available_version() -> Option<&'static str> {
    LATEST_TAG.get().map(|s| s.as_str())
}

/// 백그라운드 스레드에서 최신 릴리스를 1회 확인하고, 끝나면 `notify_hwnd` 로
/// [`WM_UPDATE_CHECKED`] 를 보낸다(wParam=1: 새 버전 발견). 실패는 조용히 무시.
pub fn spawn_check(notify_hwnd: *mut c_void) {
    // HWND 원시 포인터는 Send 가 아니므로 정수로 건넨다(PostMessageW 는 호출 스레드 무관).
    let hwnd = notify_hwnd as usize;
    // 기본 스택으로 생성한다: Windows 스택은 예약(reserve)만 크고 커밋은 접근한 페이지만
    // 이뤄지며, 스레드 종료 시 전부 반환된다 — 작게 지정해 TLS 핸드셰이크(schannel)에서
    // 오버플로 위험을 감수할 이유가 없다.
    let _ = std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let found = match fetch_latest_tag() {
                Some(tag) if is_newer(&tag, VERSION) => {
                    crate::logging::log(&format!("업데이트 확인: 새 버전 {tag} 발견"));
                    LATEST_TAG.set(tag).is_ok()
                }
                _ => false,
            };
            // SAFETY: PostMessageW 는 어느 스레드에서든 호출 가능하다. 창이 이미
            // 파괴됐다면 호출이 실패할 뿐이며(반환값 무시) 부작용이 없다.
            unsafe {
                PostMessageW(hwnd as *mut c_void, WM_UPDATE_CHECKED, found as usize, 0);
            }
        });
}

/// 닫기를 잊지 않기 위한 WinHTTP 핸들 RAII 래퍼.
struct HInternet(*mut c_void);
impl Drop for HInternet {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: WinHttpOpen/Connect/OpenRequest 가 돌려준 핸들을 한 번만 닫는다.
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

/// `https://HOST PATH` 로 리다이렉트 없는 HEAD 요청을 보내 Location 헤더에서
/// 최신 릴리스 태그를 추출한다. 어떤 단계든 실패하면 None(조용히 무시).
fn fetch_latest_tag() -> Option<String> {
    fetch_tag_from(w!(HOST), w!(PATH))
}

/// [`fetch_latest_tag`] 의 본체. host/path 를 받는 것은 라이브 테스트가 릴리스 있는
/// 다른 저장소로 리다이렉트 트릭을 검증하기 위함이다(둘 다 널 종료 UTF-16, `w!` 산출).
fn fetch_tag_from(host: *const u16, path: *const u16) -> Option<String> {
    // SAFETY: 표준 WinHTTP 동기 요청 시퀀스. 모든 핸들은 HInternet Drop 이 닫고,
    // 버퍼/문자열 포인터들은 호출 동안 유효하다(`w!` 는 'static).
    unsafe {
        // AUTOMATIC_PROXY: 시스템/WPAD 프록시를 자동 적용하고, 없으면 직접 연결(사내망 대응).
        let session = HInternet(WinHttpOpen(
            w!(concat!("caps-hangul-rs/", env!("CARGO_PKG_VERSION"))),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            ptr::null(), // WINHTTP_NO_PROXY_NAME
            ptr::null(), // WINHTTP_NO_PROXY_BYPASS
            0,           // 동기 모드
        ));
        if session.0.is_null() {
            return None;
        }
        WinHttpSetTimeouts(session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS);

        let conn = HInternet(WinHttpConnect(
            session.0,
            host,
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        ));
        if conn.0.is_null() {
            return None;
        }

        let req = HInternet(WinHttpOpenRequest(
            conn.0,
            w!("HEAD"),
            path,
            ptr::null(), // HTTP/1.1
            ptr::null(), // no referrer
            ptr::null(), // default accept types
            WINHTTP_FLAG_SECURE,
        ));
        if req.0.is_null() {
            return None;
        }

        // 302 를 따라가지 않고 Location 헤더 자체를 읽는 것이 목적이므로 리다이렉트를 끈다.
        let disable: u32 = WINHTTP_DISABLE_REDIRECTS;
        WinHttpSetOption(
            req.0,
            WINHTTP_OPTION_DISABLE_FEATURE,
            &disable as *const u32 as *const c_void,
            size_of::<u32>() as u32,
        );

        if WinHttpSendRequest(req.0, ptr::null(), 0, ptr::null(), 0, 0, 0) == 0 {
            return None;
        }
        if WinHttpReceiveResponse(req.0, ptr::null_mut()) == 0 {
            return None;
        }

        // Location 은 절대 URL(수십 바이트)이므로 스택 버퍼로 충분하다. 초과 시 실패 → None.
        let mut buf = [0u16; 512];
        let mut len = (buf.len() * size_of::<u16>()) as u32; // 입력: 버퍼 크기(바이트)
        if WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_LOCATION,
            ptr::null(),     // WINHTTP_HEADER_NAME_BY_INDEX
            buf.as_mut_ptr() as *mut c_void,
            &mut len,        // 출력: 데이터 길이(바이트, 널 종료 제외)
            ptr::null_mut(), // WINHTTP_NO_HEADER_INDEX
        ) == 0
        {
            return None;
        }
        let location = String::from_utf16_lossy(&buf[..(len as usize) / size_of::<u16>()]);
        tag_from_location(&location)
    }
}

/// `…/releases/tag/<태그>` 형태의 Location 에서 태그를 추출한다.
/// (릴리스가 없으면 `…/releases` 로 리다이렉트되어 None.)
fn tag_from_location(location: &str) -> Option<String> {
    let (_, tag) = location.rsplit_once("/releases/tag/")?;
    (!tag.is_empty()).then(|| tag.to_string())
}

/// `v?major.minor.patch` 를 숫자 3튜플로 파싱한다. 형식이 다르면 None(→ 알림 안 함).
fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.strip_prefix('v').or_else(|| s.strip_prefix('V')).unwrap_or(s);
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

/// 원격 태그가 로컬 버전보다 새 버전인지. 어느 쪽이든 파싱 불가면 false.
fn is_newer(remote_tag: &str, local: &str) -> bool {
    match (parse_semver(remote_tag), parse_semver(local)) {
        (Some(r), Some(l)) => r > l,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_tag_from_location() {
        assert_eq!(
            tag_from_location("https://github.com/o/r/releases/tag/v0.1.5").as_deref(),
            Some("v0.1.5")
        );
        // 릴리스 0개 → 목록 페이지로 리다이렉트(실측 동작) → 태그 없음.
        assert_eq!(tag_from_location("https://github.com/o/r/releases"), None);
        assert_eq!(tag_from_location("https://github.com/o/r/releases/tag/"), None);
    }

    #[test]
    fn compares_semver() {
        assert!(is_newer("v0.1.5", "0.1.4"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("v0.2.0", "0.1.19"));
        assert!(!is_newer("v0.1.4", "0.1.4")); // 동일 → 알림 없음
        assert!(!is_newer("v0.0.9", "0.1.4")); // 더 오래됨 → 알림 없음
        assert!(!is_newer("not-a-version", "0.1.4")); // 파싱 불가 → 알림 없음
        assert!(!is_newer("v1.0", "0.1.4")); // 3자리 아님 → 알림 없음
    }

    #[test]
    fn current_version_parses() {
        // CARGO_PKG_VERSION 이 3자리 semver 가 아니게 되면(AGENTS.md 규칙 위반) 여기서 잡는다.
        assert!(parse_semver(VERSION).is_some());
    }

    /// 리다이렉트 트릭 전체(WinHTTP HEAD → Location → 태그)를 실제 네트워크로 검증한다.
    /// 릴리스가 항상 존재하는 저장소를 쓴다. 실행:
    /// `cargo test --target x86_64-pc-windows-msvc -- --ignored`
    #[test]
    #[ignore = "네트워크 필요(라이브 테스트)"]
    fn live_fetch_tag_via_redirect() {
        let tag = fetch_tag_from(w!("github.com"), w!("/microsoft/terminal/releases/latest"));
        assert!(tag.is_some(), "Location 리다이렉트에서 태그를 얻지 못함");
        // 자기 저장소: 아직 릴리스가 없으면 None, 생기면 Some — 어느 쪽이든 패닉 없이 동작.
        let _ = fetch_latest_tag();
    }
}
