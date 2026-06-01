# caps-hangul-rs

Windows 환경에서 macOS 의 Caps Lock 동작과 유사하게 동작하는 경량 Rust 유틸리티.

- **Caps Lock 짧게 누름** → 한/영 전환 (`VK_HANGUL`, 0x15)
- **Caps Lock 길게 누름** (기본 250ms 이상) → Caps Lock 대/소문자 토글 (`VK_CAPITAL`, 0x14)

AutoHotkey, PowerToys, .NET / Node.js 런타임 없이 단일 네이티브 `.exe` 로 동작하며,
백그라운드에 상주하면서 낮은 메모리/CPU 사용량을 유지한다.

자세한 설계는 [`rust_capslock_hangul_design_plan.md`](rust_capslock_hangul_design_plan.md) 참고.

## 요구 사항

- Windows 10 / 11 (x64, aarch64)
- 빌드 시: [Rust stable](https://rustup.rs/) 툴체인

## 빌드

```powershell
# 디버그 빌드 (콘솔 창 유지 + 로그 출력)
cargo build

# 릴리스 빌드 (콘솔 창 없음, 최적화)
cargo build --release
```

산출물: `target\release\caps-hangul.exe`

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
| `src/logging.rs` | 디버그 빌드 전용 로깅 |

## 라이선스

[MIT License](LICENSE).
