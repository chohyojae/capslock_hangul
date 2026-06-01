# caps-hangul-rs

Windows 환경에서 macOS 의 Caps Lock 동작과 유사하게 동작하는 경량 Rust 유틸리티.

- **Caps Lock 짧게 누름** → 한/영 전환 (`VK_HANGUL`, 0x15)
- **Caps Lock 길게 누름** (기본 250ms 이상) → Caps Lock 대/소문자 토글 (`VK_CAPITAL`, 0x14)
- **전환 시 화면 중앙 안내 HUD** → macOS 처럼 한/영 전환 시 `한`/`A`, Caps Lock 토글 시
  `CAPS ON`/`CAPS OFF` 가 활성 모니터 가운데에 잠깐 떴다 사라진다 (HiDPI 대응)

AutoHotkey, PowerToys, .NET / Node.js 런타임 없이 단일 네이티브 `.exe` 로 동작하며,
백그라운드에 상주하면서 낮은 메모리/CPU 사용량을 유지한다.

자세한 설계는 [`rust_capslock_hangul_design_plan.md`](rust_capslock_hangul_design_plan.md) 참고.

## 요구 사항

- Windows 10 / 11 (x64, aarch64)
- 빌드 시: [Rust stable](https://rustup.rs/) 툴체인

## 빌드

이 프로젝트는 **x64 / aarch64 두 타겟**을 구동 대상으로 한다.
기본 빌드/실행/테스트는 호스트 타겟(보통 x64)을 사용하므로 표준 명령이 그대로 동작하고,
두 타겟을 한 번에 빌드할 때는 `cargo build-all` 별칭을 쓴다(`.cargo/config.toml` 정의).

```powershell
# 개발 중 (호스트 타겟) — 디버그는 콘솔 창 유지 + 로그 출력
cargo run
cargo build
cargo test

# 두 타겟 모두 빌드 (배포용)
cargo build-all                # 디버그
cargo build-all --release      # 릴리스 (콘솔 창 없음, 최적화)

# 한쪽 타겟만
cargo build-x64 --release      # x86_64 만
cargo build-aarch64 --release  # aarch64 만
```

타겟을 명시(`build-all` / `build-x64` / `build-aarch64`)하면 산출물 경로가
`target\release\` 가 아니라 **아키텍처별 폴더**로 바뀐다.

- x64: `target\x86_64-pc-windows-msvc\release\caps-hangul.exe`
- aarch64: `target\aarch64-pc-windows-msvc\release\caps-hangul.exe`

### 선행 조건

aarch64 타겟까지 빌드(`build-all` / `build-aarch64`)하려면 두 툴체인 타겟이 설치돼 있어야 한다.
clone 직후 한 번 실행한다.

```powershell
rustup target add x86_64-pc-windows-msvc aarch64-pc-windows-msvc
```

> aarch64 cross-link 에는 Visual Studio 의 **"MSVC v143 - ARM64 빌드 도구"** 컴포넌트가 필요하다.

## 테스트

```powershell
cargo test
```

순수 로직(짧게/길게 누름 판정 등) 단위 테스트가 포함되어 있다(설계 §15.1).

## 사용법 (§20)

1. `caps-hangul.exe` 를 원하는 디렉터리에 복사한다.
2. 실행한다. (콘솔 창 없이 백그라운드 상주)
3. 자동 실행이 필요하면 아래 스크립트를 사용하거나 `shell:startup` 에 바로가기를 등록한다.

| 동작 | 결과 |
| --- | --- |
| Caps Lock 짧게 누름 | 한/영 전환 |
| Caps Lock 길게 누름 (≥250ms) | Caps Lock 토글 |
| 그 외 모든 키 | 기존 동작 그대로 |

> 길게 누름(Caps Lock 토글)은 **임계 시간(250ms)을 넘기는 순간** 동작·안내가 뜨며,
> 그 이후 키를 언제 떼는지와 무관하다.

## 전환 안내 HUD

한/영 전환·Caps Lock 토글 시 **활성 모니터 중앙**에 반투명 둥근 박스로 안내가 떴다가
짧게 유지 후 페이드아웃한다.

| 동작 | 표시 |
| --- | --- |
| 한/영 전환 (짧게 누름) | `한` (한글) / `A` (영문) |
| Caps Lock 토글 (길게 누름) | `CAPS ON` / `CAPS OFF` |

- **표시 언어 판단(하이브리드)**: 전환 즉시 내부 추정값으로 라벨을 띄워 **지연 없이** 보여주고,
  잠시 후 실제 IME 변환 모드(`WM_IME_CONTROL`/`IMC_GETCONVERSIONMODE`)를 조회해 추정과 다르면 갱신한다.
  일시적 조회 오차로 깜빡이지 않도록 **동일 불일치가 연속 두 번** 확인될 때만 보정한다.
- **HiDPI**: 프로세스를 Per-Monitor-V2 DPI 인식으로 설정하고 모니터 DPI 에 맞춰
  박스/폰트 크기를 스케일해 또렷하게 렌더링한다.
- **비간섭**: 안내 창은 click-through·no-activate·topmost 레이어드 창이라
  포커스·마우스 입력을 가로채지 않는다.
- 한국어 IME 가 없는 앱 등 실제 상태를 알 수 없을 때는 추정값을 그대로 표시한다.
- 표시 시간·페이드·크기 등은 `src/overlay.rs` 상단 상수로 조절할 수 있다.

오버레이 초기화에 실패해도 프로그램은 HUD 없이 정상 동작한다.

## 시작 프로그램 등록 (§14)

```powershell
# 등록 (Startup 폴더에 바로가기 생성)
.\install-startup.ps1

# 해제
.\uninstall-startup.ps1
```

관리자 권한 없이 현재 사용자(`HKCU` / Startup 폴더) 기준으로 등록된다.

## 종료

초기 버전에서는 **작업 관리자**에서 `caps-hangul.exe` 프로세스를 종료한다.
향후 버전에서 tray icon 또는 `--quit-existing` 옵션을 제공할 예정이다(설계 §10.4).

## 문제 해결 (§20.4)

- **한/영 전환이 되지 않음**: Windows 입력 언어에 한국어 Microsoft IME 가 등록되어 있는지 확인.
  관리자 권한 앱에서만 동작하지 않으면 프로그램도 관리자 권한으로 실행해 본다.
- **Caps Lock 이 두 번 토글되는 듯함**: 프로그램이 중복 실행 중인지 확인(named mutex 로 방지됨).
- **키 입력이 느려짐**: 릴리스 빌드는 로그가 비활성화되어 있다. 다른 remapping 프로그램과의 충돌을 확인.

## 모듈 구성 (§5.2)

| 파일 | 역할 |
| --- | --- |
| `src/main.rs` | 진입점, 수명 주기 조립 |
| `src/win32.rs` | Win32 API 얇은 wrapper, 메시지 루프 |
| `src/hook.rs` | `WH_KEYBOARD_LL` 훅 설치/해제, `LowLevelKeyboardProc` |
| `src/input.rs` | `SendInput` 기반 키 합성 |
| `src/config.rs` | 설정값, 짧게/길게 누름 판정 로직 |
| `src/state.rs` | 전역 atomic 상태 |
| `src/single_instance.rs` | named mutex 기반 중복 실행 방지 |
| `src/overlay.rs` | 전환 안내 HUD(레이어드 창 렌더링, IME 상태 검증) |
| `src/logging.rs` | 디버그 빌드 전용 로깅 |

## 라이선스

[MIT License](LICENSE).
