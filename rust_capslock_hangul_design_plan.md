# Rust 기반 Windows Caps Lock 한/영 전환 유틸리티 설계 계획서

## 1. 문서 개요

### 1.1 목적

본 문서는 Windows 환경에서 macOS의 Caps Lock 동작과 유사하게 다음 기능을 제공하는 Rust 기반 백그라운드 유틸리티의 설계 계획을 정의한다.

- Caps Lock 키를 **짧게 누르면 한/영 전환** 수행
- Caps Lock 키를 **길게 누르면 Caps Lock 대/소문자 잠금 토글** 수행
- AutoHotkey, PowerToys, 별도 스크립트 런타임 없이 **단일 네이티브 실행 파일**로 동작
- 백그라운드 상주 프로그램으로서 **낮은 메모리 사용량과 낮은 CPU 사용률** 유지

### 1.2 대상 플랫폼

- OS: Windows 10 (LTSC 2019 및 이후 빌드) / Windows 11 (LTSC 2024 및 이후 빌드)
- Architecture: x64, aarch64
- Language: Rust
- API: Win32 API
- Runtime dependency: 없음 또는 최소화

### 1.3 주요 설계 방향

본 프로그램은 Windows 저수준 키보드 훅을 이용하여 Caps Lock 입력을 감지하고, 키를 누른 시간을 기준으로 다른 입력을 합성하여 전송한다.

핵심 Win32 API는 다음과 같다.

- `SetWindowsHookExW` with `WH_KEYBOARD_LL`
- `LowLevelKeyboardProc`
- `SendInput`
- `GetMessageW` / `TranslateMessage` / `DispatchMessageW`
- `UnhookWindowsHookEx`

---

## 2. 기능 요구사항

### 2.1 필수 기능

#### FR-001. Caps Lock 짧게 누름 감지

사용자가 Caps Lock 키를 짧게 눌렀다 떼면 프로그램은 Windows 한/영 전환 입력을 발생시켜야 한다.

- 기본 기준 시간: `250ms` 미만
- 기본 전송 키: `VK_HANGUL` / `VK_KANA` 계열 가상 키 코드 `0x15`
- 원래 Caps Lock 입력은 OS 및 애플리케이션으로 전달되지 않아야 한다.

#### FR-002. Caps Lock 길게 누름 감지

사용자가 Caps Lock 키를 일정 시간 이상 누른 뒤 떼면 프로그램은 Caps Lock 토글 입력을 발생시켜야 한다.

- 기본 기준 시간: `250ms` 이상
- 기본 전송 키: `VK_CAPITAL` 가상 키 코드 `0x14`
- 사용자는 기존 Caps Lock 키처럼 대문자 잠금 상태를 켜거나 끌 수 있어야 한다.

#### FR-003. 백그라운드 상주

프로그램은 백그라운드에서 상주하면서 키보드 이벤트를 처리해야 한다.

- 콘솔 창 없이 실행 가능해야 한다.
- 사용자가 명시적으로 종료하기 전까지 동작해야 한다.
- idle 상태 CPU 사용률은 사실상 0%에 가까워야 한다.

#### FR-004. 단일 실행 파일 배포

프로그램은 별도 런타임 설치 없이 단일 `.exe` 파일로 배포 가능해야 한다.

- AutoHotkey 런타임 불필요
- .NET Runtime 불필요
- Node.js / Electron 불필요

---

## 3. 비기능 요구사항

### 3.1 성능

- 키 입력 처리 지연은 사용자가 체감하지 못할 수준이어야 한다.
- 훅 콜백 내부에서는 파일 I/O, 네트워크 I/O, 복잡한 연산을 수행하지 않는다.
- 훅 콜백은 가능한 한 빠르게 반환해야 한다.

### 3.2 메모리

- 목표 idle Working Set: 가능한 한 낮게 유지
- Rust 표준 라이브러리 사용은 허용하되, 대형 프레임워크 사용은 피한다.
- GUI 프레임워크, async runtime, 로그 프레임워크 등은 기본 구성에서 제외한다.

### 3.3 안정성

- 프로그램 종료 시 설치한 키보드 훅을 정상 해제해야 한다.
- 합성 입력이 다시 훅에 들어오는 경우를 고려해야 한다.
- 훅 설치 실패 시 명확한 오류 코드를 반환하거나 이벤트 로그/파일 로그에 기록할 수 있어야 한다.

### 3.4 보안

- 관리자 권한 없이 동작하는 것을 기본 목표로 한다.
- 키 입력 내용을 저장하거나 외부로 전송하지 않는다.
- Caps Lock 이벤트 외의 키 입력은 기록하지 않는다.
- 회사 환경 배포를 고려하여 소스 코드 리뷰가 쉬운 작은 구조를 유지한다.

---

## 4. 제외 범위

초기 버전에서는 다음 기능을 제외한다.

- 복잡한 GUI 설정 화면
- Electron, Tauri, WPF 등 GUI 프레임워크 기반 UI
- 클라우드 동기화
- 키 입력 로깅
- 사용자별 프로파일 관리
- 키별 복잡한 remapping DSL
- 다국어 UI
- 자동 업데이트 기능

단, 향후 확장 기능으로 tray icon, 설정 파일, 시작 프로그램 등록 기능은 고려할 수 있다.

---

## 5. 시스템 아키텍처

### 5.1 전체 구조

```text
+--------------------------------------------------+
| Rust Native Background Process                   |
|                                                  |
|  +--------------------------------------------+  |
|  | Win32 Message Loop                         |  |
|  | - GetMessageW                              |  |
|  | - TranslateMessage                         |  |
|  | - DispatchMessageW                         |  |
|  +----------------------+---------------------+  |
|                         |                        |
|                         v                        |
|  +--------------------------------------------+  |
|  | Low-Level Keyboard Hook                    |  |
|  | - SetWindowsHookExW(WH_KEYBOARD_LL)        |  |
|  | - LowLevelKeyboardProc                     |  |
|  +----------------------+---------------------+  |
|                         |                        |
|                         v                        |
|  +--------------------------------------------+  |
|  | Caps Lock Event Handler                    |  |
|  | - KeyDown time record                      |  |
|  | - KeyUp elapsed time calculation           |  |
|  | - Short press / long press decision        |  |
|  +----------------------+---------------------+  |
|                         |                        |
|                         v                        |
|  +--------------------------------------------+  |
|  | Input Injector                             |  |
|  | - SendInput(VK_HANGUL)                     |  |
|  | - SendInput(VK_CAPITAL)                    |  |
|  +--------------------------------------------+  |
+--------------------------------------------------+
```

### 5.2 모듈 구성

권장 모듈 구성은 다음과 같다.

```text
src/
  main.rs
  win32.rs
  hook.rs
  input.rs
  config.rs
  state.rs
  single_instance.rs   # 선택 사항
  logging.rs           # 선택 사항
```

#### `main.rs`

- 프로그램 진입점
- 중복 실행 방지 초기화
- 설정 로드
- 키보드 훅 설치
- 메시지 루프 실행
- 종료 시 훅 해제

#### `win32.rs`

- Win32 API wrapper
- `windows-sys` 또는 `windows` crate의 unsafe 호출을 얇게 감싼다.
- unsafe 영역을 한정하여 나머지 코드의 안전성을 높인다.

#### `hook.rs`

- `WH_KEYBOARD_LL` 훅 설치/해제
- `LowLevelKeyboardProc` 구현
- Caps Lock 이벤트 여부 판별

#### `input.rs`

- `SendInput` 기반 키 입력 합성
- `VK_HANGUL` 입력 전송
- `VK_CAPITAL` 입력 전송
- 합성 입력 재진입 방지 처리

#### `config.rs`

- 설정값 정의
- 초기 버전에서는 컴파일 타임 기본값 사용 가능
- 향후 TOML/JSON 설정 파일 로드 기능 추가 가능

#### `state.rs`

- Caps Lock 눌림 상태
- KeyDown 시각
- synthetic input 상태 플래그
- atomic 변수 또는 mutex 기반 상태 관리

---

## 6. 핵심 동작 시퀀스

### 6.1 짧게 누름 시퀀스

```text
User presses Caps Lock
  -> LowLevelKeyboardProc receives WM_KEYDOWN
  -> vkCode == VK_CAPITAL 확인
  -> 현재 시각 저장
  -> 원래 Caps Lock KeyDown 차단: return non-zero

User releases Caps Lock
  -> LowLevelKeyboardProc receives WM_KEYUP
  -> 현재 시각 - KeyDown 시각 계산
  -> elapsed < threshold
  -> SendInput(VK_HANGUL down/up)
  -> 원래 Caps Lock KeyUp 차단: return non-zero
```

### 6.2 길게 누름 시퀀스

```text
User presses Caps Lock
  -> LowLevelKeyboardProc receives WM_KEYDOWN
  -> vkCode == VK_CAPITAL 확인
  -> 현재 시각 저장
  -> 원래 Caps Lock KeyDown 차단

User holds Caps Lock for threshold or longer

User releases Caps Lock
  -> LowLevelKeyboardProc receives WM_KEYUP
  -> elapsed >= threshold
  -> SendInput(VK_CAPITAL down/up)
  -> 원래 Caps Lock KeyUp 차단
```

### 6.3 다른 키 입력 처리

```text
User presses any non-Caps Lock key
  -> LowLevelKeyboardProc receives event
  -> vkCode != VK_CAPITAL
  -> CallNextHookEx 호출
  -> 원래 입력 흐름 유지
```

---

## 7. 상태 관리 설계

### 7.1 상태 변수

초기 구현에서는 전역 static atomic 상태를 사용할 수 있다.

```rust
static CAPS_DOWN: AtomicBool;
static CAPS_DOWN_TIME_MS: AtomicU64;
static INJECTING: AtomicBool;
```

### 7.2 상태 의미

#### `CAPS_DOWN`

- 현재 Caps Lock 키가 물리적으로 눌린 상태인지 여부
- 키 반복 입력 처리 방지에 사용

#### `CAPS_DOWN_TIME_MS`

- Caps Lock KeyDown이 처음 감지된 시각
- KeyUp 시점에서 elapsed time 계산에 사용

#### `INJECTING`

- 프로그램이 `SendInput`으로 합성 입력을 보내는 중인지 여부
- 합성된 `VK_CAPITAL` 입력이 다시 훅 콜백에 들어올 때 재처리하지 않기 위한 플래그

### 7.3 시간 측정

- 단조 증가 시간이 필요하므로 가능하면 `Instant` 계열 사용을 고려한다.
- 단, Win32 callback과 static 상태 조합에서는 `Instant`를 전역에 직접 저장하기보다 millisecond tick 값을 저장하는 설계가 단순하다.
- 후보:
  - `GetTickCount64`
  - `QueryPerformanceCounter`
  - Rust `Instant` + 별도 상태 구조

권장 초기 구현은 `GetTickCount64` 기반이다.

---

## 8. 키 입력 합성 설계

### 8.1 한/영 전환

한/영 전환은 기본적으로 다음 virtual-key를 전송한다.

```text
VK_HANGUL = 0x15
```

Windows 헤더 및 문서상 `VK_KANA`, `VK_HANGUL`, `VK_HANGEUL`은 동일한 값 `0x15` 계열로 사용된다.

### 8.2 Caps Lock 토글

Caps Lock 토글은 다음 virtual-key를 전송한다.

```text
VK_CAPITAL = 0x14
```

### 8.3 `SendInput` 호출 방식

키 입력은 down/up 쌍으로 전송한다.

```text
INPUT #1: KEYDOWN for target virtual key
INPUT #2: KEYUP for target virtual key
```

### 8.4 재진입 방지

`SendInput(VK_CAPITAL)`로 발생시킨 Caps Lock 입력은 다시 `WH_KEYBOARD_LL` 훅에서 관찰될 수 있다. 따라서 다음 정책을 적용한다.

```text
if INJECTING == true:
    return CallNextHookEx(...)
```

또는 `KBDLLHOOKSTRUCT.flags`의 injected flag를 확인하여 합성 입력을 구분하는 보조 정책을 추가할 수 있다.

---

## 9. 설정 설계

### 9.1 초기 기본값

초기 버전에서는 설정 파일 없이 다음 값을 코드 상수로 둔다.

```text
LONG_PRESS_THRESHOLD_MS = 250
SHORT_PRESS_ACTION = VK_HANGUL
LONG_PRESS_ACTION = VK_CAPITAL
```

### 9.2 향후 설정 파일

향후 버전에서는 실행 파일과 같은 디렉터리에 다음 파일을 둘 수 있다.

```text
caps-hangul.toml
```

예시:

```toml
long_press_threshold_ms = 250
short_press_vk = "VK_HANGUL"
long_press_vk = "VK_CAPITAL"
start_minimized = true
show_tray_icon = false
```

단, 메모리 사용량 최소화를 중시하는 경우 초기 버전에서는 설정 파일 파서를 포함하지 않는 것이 좋다.

---

## 10. 프로세스 수명 주기

### 10.1 시작

```text
main()
  -> 단일 인스턴스 확인
  -> 설정 로드
  -> WH_KEYBOARD_LL 훅 설치
  -> 메시지 루프 진입
```

### 10.2 실행 중

```text
GetMessageW loop
  -> keyboard hook callback 처리
  -> Caps Lock 이벤트만 가로채기
  -> 나머지 이벤트는 그대로 전달
```

### 10.3 종료

```text
종료 메시지 수신
  -> UnhookWindowsHookEx 호출
  -> 리소스 정리
  -> 프로세스 종료
```

### 10.4 종료 방법

초기 버전에서는 다음 중 하나를 사용할 수 있다.

- 작업 관리자에서 종료
- 별도 `--quit-existing` 명령 지원
- hidden window + custom message 기반 종료
- tray icon 추가 후 메뉴에서 종료

운영 편의성을 고려하면 v1.1 이후 tray icon 또는 `--quit-existing` 옵션을 추가하는 것이 좋다.

---

## 11. 중복 실행 방지

### 11.1 필요성

프로그램이 여러 개 실행되면 Caps Lock 이벤트가 중복 처리되어 한/영 전환 또는 Caps Lock 토글이 비정상적으로 발생할 수 있다.

### 11.2 설계

Windows named mutex를 사용한다.

```text
Global\CapsHangulRustMutex
```

또는 사용자 세션 단위 실행을 원하면 다음처럼 `Local` namespace를 사용할 수 있다.

```text
Local\CapsHangulRustMutex
```

일반 사용자 환경에서는 `Local` namespace를 우선 고려한다.

---

## 12. 오류 처리 및 로깅

### 12.1 오류 처리 대상

- 훅 설치 실패
- 메시지 루프 실패
- `SendInput` 실패
- mutex 생성 실패
- 설정 파일 파싱 실패

### 12.2 초기 로깅 정책

초기 버전에서는 메모리와 의존성을 줄이기 위해 복잡한 로깅 프레임워크를 사용하지 않는다.

선택 가능한 방식:

1. 디버그 빌드에서만 stderr 출력
2. 릴리스 빌드에서는 Windows Event Log 또는 간단한 파일 로그 사용
3. `--debug` 옵션이 있을 때만 로그 활성화

기본 릴리스 빌드에서는 로그를 비활성화하는 것을 권장한다.

---

## 13. 빌드 및 배포 설계

### 13.1 권장 crate

메모리 사용량을 최소화하려면 `windows-sys` 사용을 우선 고려한다.

```toml
[dependencies]
windows-sys = { version = "0.59", features = [
  "Win32_Foundation",
  "Win32_UI_WindowsAndMessaging",
  "Win32_UI_Input_KeyboardAndMouse",
  "Win32_System_Threading"
] }
```

`windows` crate는 타입 안정성과 사용성이 좋지만, 최소 footprint 관점에서는 `windows-sys`가 더 단순할 수 있다.

### 13.2 릴리스 프로파일

```toml
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
```

### 13.3 콘솔 창 숨김

GUI subsystem을 사용하여 콘솔 창 없이 실행한다.

```rust
#![windows_subsystem = "windows"]
```

단, 디버깅 중에는 해당 속성을 임시로 제거하거나 feature flag로 제어한다.

### 13.4 배포 산출물

```text
caps-hangul.exe
README.md
LICENSE
```

선택 산출물:

```text
caps-hangul.toml
install-startup.ps1
uninstall-startup.ps1
```

---

## 14. 시작 프로그램 등록 설계

### 14.1 수동 등록

사용자가 다음 경로에 바로가기를 배치한다.

```text
shell:startup
```

### 14.2 PowerShell 설치 스크립트

향후 설치 스크립트에서 Startup 폴더에 바로가기를 생성할 수 있다.

```powershell
$Startup = [Environment]::GetFolderPath('Startup')
$ShortcutPath = Join-Path $Startup 'Caps Hangul.lnk'
$TargetPath = Join-Path $PSScriptRoot 'caps-hangul.exe'

$WshShell = New-Object -ComObject WScript.Shell
$Shortcut = $WshShell.CreateShortcut($ShortcutPath)
$Shortcut.TargetPath = $TargetPath
$Shortcut.WorkingDirectory = $PSScriptRoot
$Shortcut.Save()
```

### 14.3 레지스트리 Run 등록

대안으로 다음 위치에 등록할 수 있다.

```text
HKCU\Software\Microsoft\Windows\CurrentVersion\Run
```

초기 버전에서는 관리 권한이 필요 없는 `HKCU` 또는 Startup 폴더 방식을 권장한다.

---

## 15. 테스트 계획

### 15.1 단위 테스트

순수 로직은 단위 테스트 가능하도록 분리한다.

대상:

- elapsed time 기준 short/long 판단
- 설정값 파싱
- 상태 전이

예시 테스트 케이스:

```text
elapsed = 100ms, threshold = 250ms -> ShortPress
elapsed = 250ms, threshold = 250ms -> LongPress
elapsed = 500ms, threshold = 250ms -> LongPress
```

### 15.2 수동 기능 테스트

#### TC-001. 짧게 누름

```text
Given 프로그램이 실행 중이고 한국어 IME가 활성화되어 있음
When Caps Lock을 짧게 눌렀다 뗌
Then 한/영 입력 상태가 전환되어야 함
And Caps Lock LED 또는 Caps Lock 상태는 바뀌지 않아야 함
```

#### TC-002. 길게 누름

```text
Given 프로그램이 실행 중임
When Caps Lock을 250ms 이상 누른 뒤 뗌
Then Caps Lock 상태가 토글되어야 함
And 한/영 입력 상태는 바뀌지 않아야 함
```

#### TC-003. 일반 키 입력 영향 없음

```text
Given 프로그램이 실행 중임
When A, B, Ctrl+C, Alt+Tab 등 일반 키를 입력함
Then 기존 동작과 동일해야 함
```

#### TC-004. 빠른 반복 입력

```text
Given 프로그램이 실행 중임
When Caps Lock을 빠르게 여러 번 누름
Then 한/영 전환이 입력 횟수만큼 안정적으로 수행되어야 함
```

#### TC-005. 종료 후 원상 복귀

```text
Given 프로그램이 실행 중임
When 프로그램을 종료함
Then Caps Lock 키는 Windows 기본 동작으로 복귀해야 함
```

### 15.3 호환성 테스트

- Windows 10 x64
- Windows 11 x64
- 한국어 Microsoft IME
- 영어 US keyboard layout
- 원격 데스크톱 세션
- 관리자 권한 앱과 일반 권한 앱 간 동작 차이

---

## 16. 위험 요소 및 대응 방안

### 16.1 합성 입력 재처리

#### 위험

`SendInput(VK_CAPITAL)`로 발생시킨 입력이 다시 훅에 들어와 무한 반복 또는 중복 토글이 발생할 수 있다.

#### 대응

- `INJECTING` atomic flag 사용
- `KBDLLHOOKSTRUCT.flags`의 injected 여부 확인
- `VK_CAPITAL` 합성 시에만 특별 처리

### 16.2 권한 경계 문제

#### 위험

일반 권한 프로세스가 관리자 권한 프로세스의 입력 처리에 영향을 주지 못하는 경우가 있을 수 있다.

#### 대응

- 기본은 일반 권한 실행
- 관리자 권한 앱에서도 동일 동작이 필요한 경우 프로그램도 관리자 권한으로 실행하도록 문서화
- 회사 환경에서는 보안 정책과 충돌 가능성 검토

### 16.3 입력 언어/IME 차이

#### 위험

`VK_HANGUL` 전송이 일부 IME 또는 키보드 레이아웃에서 기대와 다르게 동작할 수 있다.

#### 대응

- 기본값은 `VK_HANGUL`
- 대체 모드로 `Win + Space` 또는 `Alt + Shift` 전송 옵션을 향후 제공
- 설정 파일에서 short press action을 선택 가능하게 확장

### 16.4 훅 콜백 지연

#### 위험

훅 콜백이 지연되면 시스템 키 입력 전체에 영향을 줄 수 있다.

#### 대응

- 콜백 내 로직 최소화
- 동적 메모리 할당 지양
- I/O 금지
- 로깅 금지 또는 비동기 큐로 분리

---

## 17. 개발 단계 계획

### Phase 1. 최소 기능 프로토타입

목표:

- `WH_KEYBOARD_LL` 훅 설치
- Caps Lock KeyDown/KeyUp 감지
- short/long 판단
- `SendInput(VK_HANGUL)` 및 `SendInput(VK_CAPITAL)` 구현

산출물:

- 콘솔 기반 디버그 실행 파일
- 수동 테스트 결과

### Phase 2. 안정화

목표:

- 재진입 방지
- 중복 실행 방지
- 종료 처리 개선
- 릴리스 빌드 최적화

산출물:

- 콘솔 창 없는 릴리스 exe
- 기본 README

### Phase 3. 운영 편의성 추가

목표:

- Startup 등록 스크립트
- `--install-startup`
- `--uninstall-startup`
- `--quit-existing`

산출물:

- 배포 zip
- 설치/삭제 문서

### Phase 4. 선택 확장

목표:

- tray icon
- 설정 파일 지원
- threshold 변경
- short press action 선택
- pause/resume 기능

산출물:

- v1.1 또는 v2.0 릴리스

---

## 18. 권장 초기 구현 사양

초기 버전은 다음 사양으로 구현한다.

```text
Project name: caps-hangul-rs
Language: Rust stable
Crate: windows-sys
Subsystem: windows
Hook: WH_KEYBOARD_LL
Short press threshold: 250ms
Short press action: VK_HANGUL(0x15)
Long press action: VK_CAPITAL(0x14)
Single instance: Local named mutex
Config file: 없음
Tray icon: 없음
Logging: debug build only
```

---

## 19. 예비 코드 구조

### 19.1 `Cargo.toml` 예시

```toml
[package]
name = "caps-hangul-rs"
version = "0.1.0"
edition = "2021"

[dependencies]
windows-sys = { version = "0.59", features = [
  "Win32_Foundation",
  "Win32_UI_WindowsAndMessaging",
  "Win32_UI_Input_KeyboardAndMouse",
  "Win32_System_Threading"
] }

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
```

### 19.2 의사 코드

```rust
initialize_single_instance_mutex();
load_config_or_default();
install_keyboard_hook();

while get_message() {
    translate_message();
    dispatch_message();
}

uninstall_keyboard_hook();
```

### 19.3 훅 콜백 의사 코드

```rust
keyboard_proc(code, w_param, l_param) {
    if code != HC_ACTION {
        return call_next_hook();
    }

    event = parse_keyboard_event(l_param);

    if injecting {
        return call_next_hook();
    }

    if event.vk_code != VK_CAPITAL {
        return call_next_hook();
    }

    if event.is_key_down() {
        if !caps_down {
            caps_down = true;
            caps_down_time = now_ms();
        }
        return block_original_input();
    }

    if event.is_key_up() {
        elapsed = now_ms() - caps_down_time;
        caps_down = false;

        if elapsed >= threshold {
            inject_key(VK_CAPITAL);
        } else {
            inject_key(VK_HANGUL);
        }

        return block_original_input();
    }

    return call_next_hook();
}
```

---

## 20. 운영 문서 초안

### 20.1 설치

1. `caps-hangul.exe`를 원하는 디렉터리에 복사한다.
2. 실행한다.
3. 자동 실행이 필요하면 `shell:startup`에 바로가기를 등록한다.

### 20.2 사용법

- Caps Lock 짧게 누름: 한/영 전환
- Caps Lock 길게 누름: Caps Lock 토글

### 20.3 종료

초기 버전에서는 작업 관리자에서 종료한다.
향후 버전에서는 tray icon 또는 `--quit-existing` 옵션을 제공한다.

### 20.4 문제 해결

#### 한/영 전환이 되지 않음

- Windows 입력 언어에 한국어 Microsoft IME가 등록되어 있는지 확인한다.
- 관리자 권한 앱에서만 동작하지 않는 경우 프로그램을 관리자 권한으로 실행해본다.
- 다른 키보드 remapping 프로그램과 충돌하는지 확인한다.

#### Caps Lock이 두 번 토글되는 것처럼 보임

- 합성 입력 재진입 방지 로직이 정상 동작하는지 확인한다.
- 프로그램이 중복 실행 중인지 확인한다.

#### 프로그램 실행 후 키 입력이 느려짐

- 디버그 로그가 켜져 있는지 확인한다.
- 훅 콜백 내부에서 I/O 또는 과도한 연산을 수행하지 않는지 확인한다.

---

## 21. 최종 요약

본 프로그램은 Windows 저수준 키보드 훅과 `SendInput`을 이용하여 macOS와 유사한 Caps Lock 한/영 전환 경험을 제공하는 경량 Rust 유틸리티로 설계한다.

초기 버전에서는 다음 원칙을 따른다.

- 기능은 Caps Lock short/long press 처리에 집중한다.
- UI와 설정 기능은 최소화한다.
- 메모리와 CPU 사용량을 낮게 유지한다.
- AutoHotkey나 외부 런타임 없이 단일 네이티브 exe로 배포한다.
- 회사 환경에서 소스 검토와 보안 검토가 쉬운 작은 코드베이스를 유지한다.
