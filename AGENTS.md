# AGENTS.md

이 저장소에서 작업할 때 따르는 규칙.

## 의존성 (dependencies)

- **신규 의존성을 추가할 때는 항상 최신 안정 버전을 먼저 검색·확인한 뒤** 그 버전을 사용한다.
  - 확인 방법: `cargo search <crate>` 또는 [crates.io](https://crates.io/) 에서 최신 버전 조회,
    혹은 `cargo add <crate>` (최신 버전을 자동으로 박아 준다).
- **`Cargo.toml` 의 버전은 항상 정확한 3자리 semver(`major.minor.patch`)로 표기**한다.
  1자리·2자리 축약(`"3"`, `"1.5"`)은 쓰지 않는다.
  - ✅ `embed-resource = "3.0.9"`, `embed-manifest = "1.5.0"`, `windows-sys = "0.61.2"`
  - ❌ `embed-resource = "3"`, `embed-manifest = "1.5"`
- 버전을 올릴 때는 `Cargo.toml` 표기와 `Cargo.lock` 을 함께 맞춘다:
  `cargo update -p <crate> --precise <major.minor.patch>` 로 lock 을 정확히 고정한 뒤
  `Cargo.toml` 의 표기도 같은 3자리로 갱신한다.

## 빌드 / 검증

- 본체 디버그 인스턴스가 `target\debug\` 에서 상주 실행 중일 수 있으므로, 빌드·검증은
  파일 잠금 충돌을 피하도록 **격리 타깃**으로 한다:
  `cargo build [--release] --target x86_64-pc-windows-msvc`.
