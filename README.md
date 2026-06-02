# caps-hangul-rs

Windows 환경에서 macOS 의 Caps Lock 동작과 유사하게 동작하는 경량 Rust 유틸리티.

- **Caps Lock 짧게 누름** → 한/영 전환 (`VK_HANGUL`, 0x15)
- **Caps Lock 길게 누름** (기본 400ms 이상) → Caps Lock 대/소문자 토글 (`VK_CAPITAL`, 0x14)
- **전환 시 화면 중앙 안내 HUD** → macOS 처럼 한/영 전환 시 `한`/`A`, Caps Lock 토글 시
  `CAPS ON`/`CAPS OFF` 가 활성 모니터 가운데에 잠깐 떴다 사라진다 (HiDPI 대응)
- **Teams 등 TSF/Chromium 앱에서도 한/영 라벨이 정확** → 포커스 스레드에 잠깐 주입되는
  TSF 리더 DLL 로 실제 IME 변환 모드를 읽는다 (아래 [IME 한/영 상태 정확 조회](#ime-한영-상태-정확-조회-tsf-리더-dll) 참조)

AutoHotkey, PowerToys, .NET / Node.js 런타임 없이 단일 네이티브 `.exe`(+ 동봉 DLL)로 동작하며,
백그라운드에 상주하면서 낮은 메모리/CPU 사용량을 유지한다.

자세한 설계는 [`rust_capslock_hangul_design_plan.md`](rust_capslock_hangul_design_plan.md) 참고.

## 요구 사항

- Windows 10 / 11 (x64, aarch64)
- 빌드 시: [Rust stable](https://rustup.rs/) 툴체인

## 구성 산출물 (2-파일)

배포물은 **두 파일을 한 쌍**으로 동봉한다. 둘은 같은 폴더에 있어야 한다.

| 파일 | 역할 |
| --- | --- |
| `caps-hangul.exe` | 본체. 키보드 훅 + HUD 오버레이 (`windows-sys`) |
| `caps-hangul-tsf-<arch>.dll` | 포커스 스레드에 주입돼 IME 한/영 상태를 읽는 TSF 리더 (`windows`) |

`<arch>` 는 `x64` 또는 `arm64`. 본체는 실행 시 **자신과 같은 폴더**에서 자기 아키텍처에 맞는 DLL
(`caps-hangul-tsf-x64.dll` / `caps-hangul-tsf-arm64.dll`, 개발 빌드에서는 `caps_hangul_tsf.dll`)을
로드한다. DLL 이 없거나 주입이 막혀도 본체는 **추정값 폴백**으로 정상 동작한다(한/영 라벨만
부정확해질 수 있음).

> exe 와 DLL 은 반드시 **같은 아키텍처**로 짝지어야 한다(주입 대상 프로세스와 비트니스 일치 필요).
> `build.ps1` 이 아키텍처별로 자동 패키징한다.

## 빌드

본체 exe 와 주입 DLL 은 **하나의 Cargo 워크스페이스**의 두 멤버다(`default-members` 로 묶여
있어 표준 명령이 둘을 같은 출력 폴더에 함께 만든다).

### 선행 조건

clone 직후 한 번, 구동 대상 두 타겟을 추가한다.

```powershell
rustup target add x86_64-pc-windows-msvc aarch64-pc-windows-msvc
```

> aarch64 cross-link 에는 Visual Studio 의 **"MSVC v143 - ARM64 빌드 도구"** 컴포넌트가 필요하다.

### 개발 중 (호스트 타겟)

디버그 빌드는 콘솔 창을 유지하고 로그를 출력한다.

```powershell
cargo build    # exe + DLL 을 함께 빌드 (target\debug\ 에 나란히 떨어짐)
cargo run      # 본체 실행
cargo test     # 단위 테스트
```

`cargo build`/`cargo test` 는 `default-members` 덕분에 exe·DLL 을 **함께** 빌드한다. 반면
`cargo run` 은 본체 바이너리의 의존성 그래프만 빌드하므로(주입 DLL 은 본체의 의존성이 아님)
**DLL 을 만들지 않는다.** 따라서 IME 리더까지 시험하려면 `cargo build` 로 DLL 을 먼저 만든 뒤
`cargo run`(또는 `target\debug\caps-hangul.exe`)으로 실행한다. DLL 이 없으면 한/영 라벨은
추정값 폴백으로 동작한다.

### 릴리스 패키징 (배포용)

`build.ps1` 이 릴리스 빌드(콘솔 창 없음·최적화) 후 DLL 을 아키텍처 접미사로 리네이밍하고
설치 스크립트·문서까지 모아 자기완결 배포 폴더를 만든다.

```powershell
.\build.ps1                 # 현재 PC 아키텍처(보통 x64)만
.\build.ps1 -Arch arm64     # arm64 만
.\build.ps1 -Arch all       # x64 + arm64 모두
.\build.ps1 -Arch all -Zip  # 모두 빌드 후 각 폴더를 zip 압축
```

결과는 `dist\` 아래에 아키텍처별로 떨어진다.

```text
dist\caps-hangul-x64\
  caps-hangul.exe
  caps-hangul-tsf-x64.dll
  install-startup.ps1
  uninstall-startup.ps1
  README.md
  LICENSE
```

### 직접 cargo 로 빌드 (스크립트 없이)

`build.ps1` 없이 빌드하려면 **반드시 `--workspace`** 로 DLL 까지 함께 빌드한다.
(`.cargo\config.toml` 의 `cargo build-all` / `build-x64` / `build-aarch64` 별칭도 사용 가능.)

```powershell
cargo build --release --workspace --target x86_64-pc-windows-msvc
```

산출물:

- x64: `target\x86_64-pc-windows-msvc\release\` 의 `caps-hangul.exe` + `caps_hangul_tsf.dll`
- aarch64: `target\aarch64-pc-windows-msvc\release\` 의 `caps-hangul.exe` + `caps_hangul_tsf.dll`

배포 시 `caps_hangul_tsf.dll` 을 `caps-hangul-tsf-<arch>.dll` 로 리네이밍해 exe 옆에 둔다
(`build.ps1` 이 자동으로 해 주는 단계).

## 테스트

```powershell
cargo test
```

순수 로직(짧게/길게 누름 판정 등) 단위 테스트가 포함되어 있다(설계 §15.1).

## 사용법

1. 배포 폴더(`caps-hangul.exe` + `caps-hangul-tsf-<arch>.dll`)를 원하는 디렉터리에 복사한다.
2. `caps-hangul.exe` 를 실행한다. (콘솔 창 없이 백그라운드 상주)
3. 자동 실행이 필요하면 아래 스크립트를 사용하거나 `shell:startup` 에 바로가기를 등록한다.

| 동작 | 결과 |
| --- | --- |
| Caps Lock 짧게 누름 | 한/영 전환 |
| Caps Lock 길게 누름 (≥400ms) | Caps Lock 토글 |
| 그 외 모든 키 | 기존 동작 그대로 |

> 길게 누름(Caps Lock 토글)은 **임계 시간(400ms)을 넘기는 순간** 동작·안내가 뜨며,
> 그 이후 키를 언제 떼는지와 무관하다.

## 전환 안내 HUD

한/영 전환·Caps Lock 토글 시 **활성 모니터 중앙**에 반투명 둥근 박스로 안내가 떴다가
짧게 유지 후 페이드아웃한다.

| 동작 | 표시 |
| --- | --- |
| 한/영 전환 (짧게 누름) | `한` (한글) / `A` (영문) |
| Caps Lock 토글 (길게 누름) | `CAPS ON` / `CAPS OFF` |

- **표시 언어 판단**: 토글 키를 보내기 **직전에** 실제 한/영 상태를 읽고(아래 TSF 리더),
  `VK_HANGUL` 전송 후 결과(= `!이전상태`)로 **한 번에** 올바른 라벨을 띄운다. 매 토글마다
  진실을 새로 읽으므로 외부 토글(트레이 클릭·우-Alt 등)로도 라벨이 어긋나지 않는다.
- **HiDPI**: 프로세스를 Per-Monitor-V2 DPI 인식으로 설정하고 모니터 DPI 에 맞춰
  박스/폰트 크기를 스케일해 또렷하게 렌더링한다.
- **비간섭**: 안내 창은 click-through·no-activate·topmost 레이어드 창이라
  포커스·마우스 입력을 가로채지 않는다.
- 실제 상태를 읽을 수 없을 때(주입 불가/미초기화)는 내부 추정값을 그대로 표시한다.
- 표시 시간·페이드·크기 등은 `src/overlay.rs` 상단 상수로 조절할 수 있다.

오버레이 초기화에 실패해도 프로그램은 HUD 없이 정상 동작한다.

## IME 한/영 상태 정확 조회 (TSF 리더 DLL)

### 문제

Teams 같은 Chromium/Electron 계열 앱은 입력을 **TSF(Text Services Framework)** 로 처리하고
**IMM32 는 하위 호환 스텁**만 남겨 둔다. 그래서 외부 프로세스(우리 본체)가 IMM32
(`ImmGetDefaultIMEWnd` + `WM_IME_CONTROL`)로 한/영 상태를 물으면 **항상 0(영문)** 이 돌아와,
오버레이가 항상 "한"으로만 떴다(VS Code·Slack·Discord 등 동일 계열에서 재현).

핵심 제약: **한/영 변환 모드는 "포커스를 가진 스레드의 입력 컨텍스트별 상태"** 이고,
TSF 의 `ITfThreadMgr` 는 호출 스레드/프로세스 로컬이라 cross-process 로 읽는 공개 경로가 없다.
"단일 전역 값"은 없으며, 백그라운드 프로세스가 외부에서 가볍게 읽어낼 대리 값도 없다.

### 접근 비교

| | 접근 | 정확도 | 비고 |
| --- | --- | --- | --- |
| **채택** | **포커스 스레드 in-process 주입** — 진짜 포커스 창(다른 프로세스 가능)의 스레드에 리더 DLL 을 잠깐 주입, 그 안에서 TSF compartment 를 읽어 회신 | ✅ Teams 포함 정확 | 침습적(타 프로세스에 DLL 로드), 비트니스 일치 필요, 백신 오탐 소지, 샌드박스/상위 무결성 포커스엔 주입 불가 |
| 기각 | IMM32 cross-process | ✗ Chromium 에서 항상 0 | 1차 원인 |
| 기각 | TSF langbar cross-process (`GetThreadLangBarItemMgr`) | ✗ E_FAIL | cross-process 경로 없음 |
| 기각 | 본체 자신 in-process 상태 읽기 | ✗ 컨텍스트 없음 | 포커스를 못 받는 백그라운드 프로세스엔 입력 컨텍스트가 인스턴스화되지 않음 |

### 동작 (on-demand)

사용자가 한/영을 토글하는 **그 순간에만**:

1. 진짜 포커스 창을 `AttachThreadInput` + `GetFocus` 로 구하고(프로세스 경계 초월),
2. 그 스레드에 `WH_GETMESSAGE` 훅으로 리더 DLL(`caps-hangul-tsf`)을 잠깐 주입,
3. `WM_NULL` 을 보내 훅을 깨우면 DLL 이 in-process 로 변환 모드
   (`GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION` → `IME_CMODE_NATIVE` 비트)를 읽어
   named shared memory 에 쓰고 named event 로 신호,
4. 결과를 읽고 **즉시 훅을 해제**(상시 주입/상주 없음 — 호스트 부하·발자국 최소).

상시 전역 주입이 아니라 토글 시점에만 잠깐 주입·해제하므로 침습·오탐·안정성 위험이 작다.
주입 DLL 은 남의 프로세스 안에서 도는 코드라 훅 본문 전체를 `catch_unwind` 로 감싸
**어떤 경우에도 호스트를 죽이지 않는다**(릴리스 프로파일에서 `panic = "abort"` 를 의도적으로
빼 둔 이유 — `Cargo.toml` 참조).

### 한계 / 폴백

- **샌드박스/상위 무결성 포커스**(예: `SearchHost.exe`=AppContainer, 관리자 권한 창)에는 주입 불가
  → 이 경우 내부 추정값으로 폴백한다.
- **비트니스 일치** 필요(x64 대상엔 x64 DLL, arm64 엔 arm64 DLL).
- 전역 훅 주입은 **백신/EDR 오탐 소지**가 있으니 회사 환경 배포 시 고려한다.

## 시작 프로그램 등록

```powershell
# 등록 (Startup 폴더에 바로가기 생성)
.\install-startup.ps1

# 해제
.\uninstall-startup.ps1
```

관리자 권한 없이 현재 사용자(Startup 폴더) 기준으로 등록된다. `install-startup.ps1` 은 exe 옆에
짝 DLL 이 있는지도 확인해(없으면 경고) 한/영 라벨이 추정값 폴백으로만 동작하는 상황을 알려 준다.

## 종료

초기 버전에서는 **작업 관리자**에서 `caps-hangul.exe` 프로세스를 종료한다.
향후 버전에서 tray icon 또는 `--quit-existing` 옵션을 제공할 예정이다(설계 §10.4).

## 문제 해결

- **한/영 전환이 되지 않음**: Windows 입력 언어에 한국어 Microsoft IME 가 등록되어 있는지 확인.
  관리자 권한 앱에서만 동작하지 않으면 프로그램도 관리자 권한으로 실행해 본다.
- **Teams 등에서 한/영 라벨이 항상 "한"/부정확**: 짝 DLL(`caps-hangul-tsf-<arch>.dll`)이 exe 옆에
  있는지, 아키텍처가 맞는지 확인. 백신/EDR 이 DLL 주입을 차단하면 추정값 폴백으로 동작한다.
- **Caps Lock 이 두 번 토글되는 듯함**: 프로그램이 중복 실행 중인지 확인(named mutex 로 방지됨).
- **키 입력이 느려짐**: 릴리스 빌드는 로그가 비활성화되어 있다. 다른 remapping 프로그램과의 충돌을 확인.

## 모듈 구성

| 파일 | 역할 |
| --- | --- |
| `src/main.rs` | 진입점, 수명 주기 조립 |
| `src/win32.rs` | Win32 API 얇은 wrapper, 메시지 루프 |
| `src/hook.rs` | `WH_KEYBOARD_LL` 훅 설치/해제, `LowLevelKeyboardProc` |
| `src/input.rs` | `SendInput` 기반 키 합성 |
| `src/config.rs` | 설정값, 짧게/길게 누름 판정 로직 |
| `src/state.rs` | 전역 atomic 상태 |
| `src/single_instance.rs` | named mutex 기반 중복 실행 방지 |
| `src/overlay.rs` | 전환 안내 HUD(레이어드 창 렌더링) |
| `src/ime.rs` | IME 한/영 상태 리더 본체 측(DLL 로드·주입·IPC) |
| `src/logging.rs` | 디버그 빌드 전용 로깅 |
| `crates/caps-hangul-tsf/` | 포커스 스레드에 주입되는 TSF 리더 DLL |

## 라이선스

[MIT License](LICENSE).
