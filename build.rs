//! 빌드 스크립트: 릴리스 exe 에 `requireAdministrator` manifest 를 임베드한다.
//!
//! 왜 필요한가 (UIPI / User Interface Privilege Isolation):
//!   작업 관리자처럼 관리자 권한(high integrity)으로 실행되는 창이 *포커스를 가진 동안*,
//!   medium integrity 프로세스는
//!     1) 저수준 키보드 훅(WH_KEYBOARD_LL)이 아예 호출되지 않고,
//!     2) SendInput 으로 그 창에 입력을 주입할 수도 없다.
//!   둘 다 원인은 같다 — 우리 프로세스의 integrity level 이 대상 창보다 낮음.
//!   따라서 코드 로직이 아니라 *실행 권한*만 올리면 elevated 앱에서도 동작한다.
//!
//! 왜 릴리스에서만 임베드하는가:
//!   requireAdministrator manifest 가 박힌 exe 는 medium 권한 셸에서 CreateProcess 로
//!   바로 실행할 수 없어(ERROR_ELEVATION_REQUIRED, 740) `cargo run`/디버그 실행이 깨진다.
//!   관리자 요구는 배포 속성이므로, 콘솔/창 서브시스템 분기(main.rs 의 windows_subsystem)와
//!   똑같이 릴리스 빌드에만 적용한다. 디버그는 종전대로 medium 으로 실행/디버깅한다.
//!
//! 남는 한계(정상): UAC 동의창·잠금화면 등 System integrity / secure desktop 에서는
//!   관리자 권한이어도 동작하지 않는다(OS 보안 경계).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Windows 타깃이 아니면(예: 문서/린트 환경) 아무것도 하지 않는다.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }

    // 디버그 빌드에는 manifest 를 넣지 않는다(위 주석 참조).
    let profile = std::env::var("PROFILE").unwrap_or_default();
    if profile != "release" {
        return;
    }

    use embed_manifest::manifest::ExecutionLevel;
    use embed_manifest::{embed_manifest, new_manifest};

    embed_manifest(
        new_manifest("CapsHangul").requested_execution_level(ExecutionLevel::RequireAdministrator),
    )
    .expect("requireAdministrator manifest 임베드 실패");
}
