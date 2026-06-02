//! 빌드 스크립트:
//!   (1) 모든 빌드 — 버전 정보(VERSIONINFO) 리소스를 exe 에 임베드.
//!       → 파일 속성 "자세히" 탭에 파일 설명·파일 버전·제품 이름·저작권이 표시된다.
//!   (2) 릴리스 빌드 — `requireAdministrator` manifest 를 임베드.
//!       → 작업 관리자 등 관리자 권한(high integrity) 앱에서도 동작(UIPI 우회).
//!
//! 왜 manifest 가 필요한가 (UIPI / User Interface Privilege Isolation):
//!   관리자 권한으로 실행되는 창이 *포커스를 가진 동안*, medium integrity 프로세스는
//!     1) 저수준 키보드 훅(WH_KEYBOARD_LL)이 아예 호출되지 않고,
//!     2) SendInput 으로 그 창에 입력을 주입할 수도 없다.
//!   둘 다 원인은 같다 — 우리 프로세스의 integrity level 이 대상 창보다 낮음.
//!   따라서 코드 로직이 아니라 *실행 권한*만 올리면 elevated 앱에서도 동작한다.
//!
//! 왜 manifest 는 릴리스에서만 임베드하는가:
//!   requireAdministrator manifest 가 박힌 exe 는 medium 권한 셸에서 CreateProcess 로
//!   바로 실행할 수 없어(ERROR_ELEVATION_REQUIRED, 740) `cargo run`/디버그 실행이 깨진다.
//!   관리자 요구는 배포 속성이므로, 콘솔/창 서브시스템 분기(main.rs 의 windows_subsystem)와
//!   똑같이 릴리스 빌드에만 적용한다. 디버그는 종전대로 medium 으로 실행/디버깅한다.
//!
//! 왜 버전 정보는 manifest 와 따로 임베드하는가:
//!   버전 정보(RT_VERSION)는 실행 권한과 무관하므로 디버그/릴리스 모두에 넣는다.
//!   또 manifest(embed-manifest, 순수 Rust)와 리소스 종류가 달라(RT_MANIFEST vs RT_VERSION)
//!   서로 충돌하지 않는다 — 이미 검증된 manifest 경로를 건드리지 않으려고 별도(embed-resource)로
//!   처리한다. embed-resource 는 Windows SDK 의 rc.exe(MSVC 빌드에 이미 동봉)를 사용한다.
//!
//! 남는 한계(정상): UAC 동의창·잠금화면 등 System integrity / secure desktop 에서는
//!   관리자 권한이어도 동작하지 않는다(OS 보안 경계).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Windows 타깃이 아니면(예: 문서/린트 환경) 아무것도 하지 않는다.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }

    // --- (1) 버전 정보: 모든 빌드 ---
    embed_version_info(
        "Caps Lock Han/Eng toggle utility",
        "caps-hangul",
        "caps-hangul.exe",
        "0x1L", // VFT_APP
    );

    // --- (2) requireAdministrator manifest: 릴리스만 (위 주석 참조) ---
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

/// VERSIONINFO 리소스(.rc)를 생성해 rc.exe 로 컴파일·링크한다.
/// 회사명·저작권·제품명은 공통, 파일 설명/내부명/원본 파일명/파일 타입은 산출물별로 받는다.
/// 버전은 CARGO_PKG_VERSION 에서 동기화한다(`x.y.z` → `x,y,z,0`).
fn embed_version_info(
    file_description: &str,
    internal_name: &str,
    original_filename: &str,
    file_type: &str,
) {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut it = version.split('.');
    let major = it.next().unwrap_or("0");
    let minor = it.next().unwrap_or("0");
    // 0.1.0-rc1 같은 pre-release/build 접미사는 떼고 숫자만 남긴다.
    let patch = it
        .next()
        .unwrap_or("0")
        .split(['-', '+'])
        .next()
        .unwrap_or("0");

    // rc.exe 가 UTF-8(©)을 읽도록 code_page 65001 을 선언한다. winver.h 의 명명 상수 대신
    // 숫자 리터럴만 써서 #include 를 없앤다(SDK INCLUDE 경로 의존 제거 → 어디서 빌드해도 안정).
    let rc = format!(
        "#pragma code_page(65001)\n\
\n\
1 VERSIONINFO\n\
 FILEVERSION {major},{minor},{patch},0\n\
 PRODUCTVERSION {major},{minor},{patch},0\n\
 FILEFLAGSMASK 0x3fL\n\
 FILEFLAGS 0x0L\n\
 FILEOS 0x40004L\n\
 FILETYPE {file_type}\n\
 FILESUBTYPE 0x0L\n\
BEGIN\n\
    BLOCK \"StringFileInfo\"\n\
    BEGIN\n\
        BLOCK \"040904b0\"\n\
        BEGIN\n\
            VALUE \"CompanyName\", \"chohyojae\"\n\
            VALUE \"FileDescription\", \"{file_description}\"\n\
            VALUE \"FileVersion\", \"{major}.{minor}.{patch}.0\"\n\
            VALUE \"InternalName\", \"{internal_name}\"\n\
            VALUE \"LegalCopyright\", \"Copyright \u{00A9} 2026 chohyojae\"\n\
            VALUE \"OriginalFilename\", \"{original_filename}\"\n\
            VALUE \"ProductName\", \"Caps Hangul\"\n\
            VALUE \"ProductVersion\", \"{major}.{minor}.{patch}.0\"\n\
        END\n\
    END\n\
    BLOCK \"VarFileInfo\"\n\
    BEGIN\n\
        VALUE \"Translation\", 0x409, 1200\n\
    END\n\
END\n"
    );

    let rc_path = std::path::Path::new(&out_dir).join("version.rc");
    std::fs::write(&rc_path, rc).expect("version.rc 쓰기 실패");

    // 매크로 없음. 빈 배열은 embed-resource 2.x/3.x 양쪽 시그니처와 호환된다.
    let no_macros: [&str; 0] = [];
    let _ = embed_resource::compile(&rc_path, no_macros);
}
