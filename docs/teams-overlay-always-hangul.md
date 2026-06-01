# Microsoft Teams에서 오버레이가 항상 '한'으로 표시되는 현상 — 원인 분석 및 근본 해결 경로 조사

> 조사일: 2026-06-01 / 환경: Windows 11 Enterprise (26200)
> 스파이크 코드: [`examples/tsf_spike.rs`](../examples/tsf_spike.rs) (dev-dependency `windows` 0.61, 본체 빌드에 영향 없음)

## 1. 증상

- Caps Lock 단누름(한/영 전환) 시 오버레이가 항상 "한"만 표시됨
- 실제 한/영 전환은 정상 동작 (입력 모드 올바르게 전환됨)
- 작업표시줄 아이콘도 정상 변경됨
- Teams의 검색창, 메시지 창 등 모든 입력 필드에서 동일하게 발생
- (동일 계열: Electron/CEF/Chromium 기반 앱 — VS Code, Slack, Discord 등에서도 재현 가능)

## 2. 1차 원인: IMM32 vs TSF 아키텍처 불일치

`handle_language_toggle` (`src/overlay.rs`)는 IME 상태를 다음 경로로 조회한다.

```
query_ime_hangul()
  → ImmGetDefaultIMEWnd(foreground_hwnd)                       // IMM32: 기본 IME 창 핸들
  → SendMessageTimeout(ime_hwnd, WM_IME_CONTROL, IMC_GETCONVERSIONMODE, ...)
  → 반환값의 IME_CMODE_NATIVE 비트 → true=한글, false=영문
```

이 방식은 **IMM32(구형 IME API)** 를 사용한다. 반면 Teams는 Electron(Chromium) 기반으로 입력 처리에 **TSF(Text Services Framework, 신형 IME API)** 를 쓴다.

Chromium은 텍스트 입력을 TSF로 처리하고 IMM32는 **하위 호환 스텁**만 유지한다. 그 결과:

1. `ImmGetDefaultIMEWnd(teams_hwnd)` → IMM32 호환용 더미 IME 창 핸들 반환
2. 이 호환 IME 창은 실제 TSF 한/영 상태를 **반영하지 않음**
3. `IMC_GETCONVERSIONMODE` 응답이 항상 `0`(영문) 반환

따라서 `let new_hangul = !pre`(`overlay.rs`)가 항상 `!false = true`(한글)로 계산되어 라벨이 항상 "한"이 된다. 실제 `VK_HANGUL` 키 주입은 정상 동작하므로 전환·작업표시줄은 영향이 없다.

## 3. 기각된 임시방편

초기엔 "키 주입 후 ~50ms 뒤 IME를 재조회해 전·후를 비교, 같으면 내부 추정값 폴백"을 제안했으나 **기각**했다. IMM32라는 구형 API에 의존하는 이상 Chromium에서 정확한 값을 얻을 수 없고(재조회해도 여전히 0), 50ms 지연 + 추정 폴백은 근본 해결이 아니다.

핵심 요구사항을 다음과 같이 재정의했다:

> **Chromium/Electron에서도 동작하는, 포그라운드 앱의 실제 IME 한/영 상태 관측 채널이 있어야 한다.**

## 4. 근본 해결 조사: "외부 프로세스에서 IME 한/영 상태를 읽을 수 있는가?"

### 4.1 제약: TSF는 프로세스/스레드 로컬

TSF에서 변환 모드를 읽는 정석 경로:

```
ITfThreadMgr → QI(ITfCompartmentMgr)
  → GetCompartment(GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION)
  → GetValue() → IME_CMODE_NATIVE 비트
```

그러나 `ITfThreadMgr`는 **호출한 스레드/프로세스에 로컬**이다. 남의 프로세스의 thread manager를 얻는 공개 API가 없다. (IMM32가 cross-process로 됐던 건 레거시 윈도우 메시지 마샬링 덕분이고, TSF는 이를 의도적으로 제공하지 않는다.) Chromium이 TSF로 자기 상태를 잘 읽는 건 *자기 자신이 포커스 프로세스*이기 때문이며, 백그라운드 유틸리티인 우리에겐 그 전제가 성립하지 않는다.

후보로 남은 유일한 TSF-native cross-process API는 thread-id를 인자로 받는 `ITfLangBarMgr::GetThreadLangBarItemMgr`였다 → 스파이크 #1.

### 4.2 스파이크 #1 — `ITfLangBarMgr::GetThreadLangBarItemMgr` (cross-process langbar 읽기)

**방법**: `CLSID_TF_LangBarMgr` 인스턴스 생성 → 포그라운드 창의 thread-id로 `GetThreadLangBarItemMgr(tid)` 호출 → `EnumItems`로 `GUID_LBI_INPUTMODE` 버튼의 텍스트/상태("한"/"A") 읽기 시도.

**양성 대조(positive control)**: "스파이크 호출 자체가 틀려서 무조건 실패하는 것"을 배제하기 위해, 우리 스레드에 `ITfThreadMgr`를 `Activate`한 뒤 in-process 경로(`ITfThreadMgr` → QI → `ITfLangBarItemMgr` → `EnumItems`)로도 읽어봤다.

**결과** (실제 `ms-teams.exe` 포함):

| 호출 | 대상 | 결과 |
|---|---|---|
| `ITfThreadMgr.Activate` | 우리 스레드 | ✅ OK (client_id=32) |
| QI → `ITfLangBarItemMgr` → `EnumItems` | 우리 스레드 (**in-process**) | ✅ **OK, items=4** |
| `GetThreadLangBarItemMgr(0)` | 우리 스레드 | ❌ E_FAIL (0x80004005) |
| `GetThreadLangBarItemMgr(tid)` | **`ms-teams.exe`** | ❌ E_FAIL |
| `GetThreadLangBarItemMgr(tid)` | idea64 / Code / rustrover / explorer | ❌ E_FAIL |

**판정 — #1 폐기 (확정)**:

- 양성 대조 성공(in-process items=4) → **열거 코드·호출 규약은 정상**. "스파이크 버그" 가설 기각.
- 그런데 동작하는 유일한 경로(QI `ITfThreadMgr`)는 **본질적으로 in-process 전용**이다. `ITfThreadMgr`는 *우리 스레드*의 매니저이고, 남의 프로세스용 `ITfThreadMgr`는 얻을 수 없다.
- cross-process 후보였던 `GetThreadLangBarItemMgr`는 **자기 스레드(tid=0)조차 E_FAIL**, 실제 `ms-teams.exe`를 포함한 모든 외부 프로세스에서 E_FAIL.

> TSF langbar는 다른 프로세스의 IME 한/영 모드를 cross-process로 읽는 경로를 제공하지 않는다.

### 4.3 파생 실험 — "전역 상태라면 우리 자신 걸 읽으면 되지 않나?"

**동기(질문)**: 한/영이 시스템 전역 단일 값이라면, 남의 프로세스를 볼 필요 없이 우리 자신 프로세스의 in-process 상태(읽기 가능함이 4.2에서 입증됨)를 읽으면 그게 곧 Teams의 상태와 같을 것이다.

**선결 구조 확인**: 한/영(`IME_CMODE_NATIVE`)은 **입력 컨텍스트(스레드)별로 저장**된다. "모든 앱에 하나의 입력 방법" 설정은 단일 전역 값을 만드는 게 아니라, **앱이 포커스를 얻는 순간 직전 상태를 상속**하게 하여 전역처럼 *느껴지게* 할 뿐이다. → 전역이라 단정할 수 없음.

**방법**: 스파이크를 확장해, 매 틱마다 우리 자신 스레드의 langbar 입력모드 값(`OWN inputmode`)을 출력. 사용자가 Teams에서 한/영을 실제로 토글하며 입력.

**결과**: Teams 포커스 + 한/영 토글을 반복하는 내내,

```
OWN inputmode="<none>"   all=["언어"="" "수정"="" "키보드"="" "도움말"=""]
```

- `OWN inputmode`은 **한 번도 변하지 않음**.
- 우리 스레드엔 입력모드(`GUID_LBI_INPUTMODE`) 버튼이 **아예 생성되지 않음** (언어 바 골격 버튼 4개뿐).
- 시스템 언어가 한국어인데도 입력모드 항목이 없음 → "값 고정"이 아니라 **컨텍스트 자체가 없음**.

**판정 — 우리 자신 상태 읽기 폐기 (확정)**: 포커스를 한 번도 못 받는 백그라운드 프로세스는 TIP의 입력 컨텍스트를 인스턴스화하지 않아, 읽어낼 한/영 상태가 존재하지 않는다.

## 5. 아키텍처 결론 (실측 기반)

> 한/영 변환 모드는 **포커스를 가진 스레드의 입력 컨텍스트별 상태**다. "어디서든 읽을 수 있는 단일 전역 값"은 없고, 백그라운드 프로세스에서 읽어낼 대리 값도 없다. 전역感은 포커스 전환 시의 상속이 만드는 착시다.

이로써 다음 세 경로가 **막다른 길로 확정**됐다:

1. **IMM32 cross-process** — Chromium의 IMM32 스텁이 항상 0 반환 (1차 원인)
2. **TSF cross-process (langbar)** — `GetThreadLangBarItemMgr` E_FAIL (스파이크 #1)
3. **우리 자신 in-process 상태** — 백그라운드 프로세스엔 컨텍스트 없음 (4.3)
4. **순수 자가추적** — 우리가 토글의 유일 주체가 아님; 외부 토글(트레이 IME 버튼 클릭, 우-Alt, 타 전환기)로 드리프트하며 Chromium에선 재동기 불가

## 6. 남은 선택지 — 트레이드오프

| | 접근 | 정확도 | 비용 / 리스크 |
|---|---|---|---|
| **A** | **포커스 프로세스에 in-process TSF 리더 주입** — 글로벌 훅 DLL이 대상 프로세스 안에서 컨텍스트를 읽어 IPC로 전달 | ✅ **유일하게 보장된 정답** (상태가 사는 곳에서 직접 읽음) | 무겁고 침습적. 모든 프로세스에 DLL 로드, x64/arm64 비트 일치 필요, **백신 오탐 소지** |
| **B** | **시스템 트레이 입력 표시기 관측** — `SetWinEventHook`(변경 이벤트) + UIAutomation으로 "한"/"A" 읽기 | △ 권위 있는 진실(작업표시줄)을 읽음 | **스파이크로 검증 필요**. UI/로케일/버전 결합, 표시기 숨김 시 취약 |
| **C** | 자가추적 + 키보드 `VK_HANGUL` 이벤트까지 추적 | ✗ 트레이 클릭·프로그램적 변경은 사각 | 가벼움. 정확성 우선 기준에선 탈락 |

핵심: **A만 무조건 정확**이 보장된다(상태가 저장된 그 프로세스 안에서 읽으므로). B는 "될 것 같지만 검증 필요 + 돼도 취약", C는 "가볍지만 알면서 부정확".

## 7. 현재 상태 / 다음 단계

- #1(TSF langbar cross-process) 및 파생(자기 프로세스 읽기) **폐기 확정**.
- 다음 후보: **B 스파이크**(트레이 표시기 UIA 읽기 + 변경 이벤트). 검증 실패 시 **A**(주입) 설계 검토.
- 본체 코드(`src/overlay.rs`의 `query_ime_hangul` 기반 로직)는 아직 미변경. 해결 경로 확정 후 반영 예정.

## 8. 재현 방법

```powershell
cd D:\GitRepositories\capslock_hangul
cargo run --example tsf_spike
```

- 시작 시 self-check(Activate / in-process QI / `GetThreadLangBarItemMgr(0)`) 출력.
- 0.8초마다 `fg=<포그라운드 cross-process 시도> | OWN <우리 자신 in-process 읽기>` 출력.
- Teams/메모장 등으로 포커스를 옮기고 한/영을 토글하며 관찰.
- `windows` 크레이트는 **dev-dependency**로만 추가됨 → 배포 산출물 `caps-hangul`에는 영향 없음.

## 부록: 관련 코드 위치

| 파일 | 위치 | 내용 |
|---|---|---|
| `src/overlay.rs` | `query_ime_hangul` | IMM32 기반 IME 상태 조회 (1차 원인 지점) |
| `src/overlay.rs` | `handle_language_toggle` | 한/영 전환 처리 및 라벨 결정 (`!pre` 로직) |
| `src/hook.rs` | `PressKind::Short` 처리 | 오버레이로 전환 위임 |
| `examples/tsf_spike.rs` | 전체 | TSF cross-process / in-process 읽기 검증 스파이크 |
