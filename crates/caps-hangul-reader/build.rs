//! 빌드 스크립트: 버전 정보(VERSIONINFO) 리소스를 헬퍼 exe 에 임베드한다.
//! → 파일 속성 "자세히" 탭에 파일 설명·파일 버전·제품 이름·저작권이 표시된다.
//!
//! 이 헬퍼는 (관리자 권한으로 실행 중인) 본체가 띄우는 자식 프로세스라 본체의 토큰을 상속하므로,
//! 별도의 requireAdministrator manifest 는 두지 않는다(버전 정보만 임베드).
//! embed-resource 는 Windows SDK 의 rc.exe 를 사용한다.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }

    // assets/caps_lock_icon.ico(워크스페이스 루트)를 헬퍼 exe 에도 임베드 → 본체와 동일한 아이콘.
    embed_version_info(
        "Caps Hangul injection helper (per-arch broker)",
        "caps-hangul-reader",
        "caps-hangul-reader.exe",
        "0x1L", // VFT_APP
        Some("../../assets/caps_lock_icon.ico"),
    );
}

/// VERSIONINFO 리소스(.rc)를 생성해 rc.exe 로 컴파일·링크한다.
/// 버전은 CARGO_PKG_VERSION 에서 동기화한다(`x.y.z` → `x,y,z,0`).
/// `icon_rel` 이 Some 이면 그 경로(CARGO_MANIFEST_DIR 기준 상대)의 .ico 를 함께 임베드한다.
fn embed_version_info(
    file_description: &str,
    internal_name: &str,
    original_filename: &str,
    file_type: &str,
    icon_rel: Option<&str>,
) {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut it = version.split('.');
    let major = it.next().unwrap_or("0");
    let minor = it.next().unwrap_or("0");
    let patch = it
        .next()
        .unwrap_or("0")
        .split(['-', '+'])
        .next()
        .unwrap_or("0");

    // 아이콘 리소스 줄(.ico). 가장 낮은 ID 의 아이콘이 탐색기/작업표시줄 앱 아이콘이 된다.
    // rc.exe 의 .rc 는 OUT_DIR 에 쓰이므로 상대 경로가 안 먹는다 → 절대 경로(+ '/')로 박는다.
    let icon_line = match icon_rel {
        Some(rel) => {
            let mut p = std::path::PathBuf::from(
                std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
            );
            for comp in rel.split('/') {
                match comp {
                    "" | "." => {}
                    ".." => {
                        p.pop();
                    }
                    c => p.push(c),
                }
            }
            println!("cargo:rerun-if-changed={}", p.display());
            let path = p.to_string_lossy().replace('\\', "/");
            format!("1 ICON \"{path}\"\n\n")
        }
        None => String::new(),
    };

    // rc.exe 가 UTF-8(©)을 읽도록 code_page 65001 선언. winver.h 의존 제거를 위해 숫자 리터럴만 사용.
    let rc = format!(
        "#pragma code_page(65001)\n\
\n\
{icon_line}1 VERSIONINFO\n\
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

    let no_macros: [&str; 0] = [];
    let _ = embed_resource::compile(&rc_path, no_macros);
}
