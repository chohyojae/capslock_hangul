# Microsoft Teams에서 오버레이가 항상 '한'으로 표시되는 현상 — 원인 분석 및 근본 해결 경로 조사

> 조사일: 2026-06-01 / 환경: Windows 11 Enterprise (26200)
> 스파이크 코드:
> - cross-process 읽기 검증: [`examples/tsf_spike.rs`](../examples/tsf_spike.rs) (#1)
> - in-process 주입 읽기 검증: [`spikes/`](../spikes/) 워크스페이스 (#2/#2b, 본체와 완전 분리)

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

### 4.4 스파이크 #2 — in-process 주입 읽기 (DLL 주입)

#1 의 결론("cross-process 읽기 경로는 없다")을 받아들이고, A 의 핵심 미지수를 직접 검증했다:
**상태가 사는 프로세스 *안에서* 실행되면 읽을 수 있는가?**

**방법**: `WH_GETMESSAGE` 스레드-지정 훅으로 작은 cdylib(`tsf_probe_dll`)을 포그라운드 스레드에
주입. 주입된 코드가 그 스레드에서 `ITfThreadMgr`→QI `ITfCompartmentMgr`→
`GetCompartment(GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION)`→`GetValue()`(VT_I4, `IME_CMODE_NATIVE`
비트)로 변환 모드를 읽어 named shared memory 로 회신. 드라이버(`inject_driver`)가 매 틱 출력.

**1차 결과**:

| 대상 | vt | 결과 |
|---|---|---|
| 대조군 `explorer.exe` | 3 (VT_I4) | ✅ native 가 한/영 토글을 **정확히 추종** → 주입·읽기 메커니즘 정상 |
| `ms-teams.exe` 최상위 창 스레드(tid=7072) | **0 (VT_EMPTY)** | ⚠️ 주입은 성공(hr=0)하나 그 스레드엔 변환 모드 컨텍스트가 **없음** |

**해석**: 새 Teams 는 WebView2 기반. 최상위 창(`ms-teams.exe`)은 셸일 뿐이고, 실제 포커스 입력은
자식 `msedgewebview2.exe` 스레드에 산다. 우리가 **잘못된 스레드**를 본 것 — IMM32 가 0 을 반환한
근본 이유도 동일(포커스 입력 스레드가 아닌 셸 창을 조회).

### 4.5 스파이크 #2b — 포커스 스레드 추적 후 주입 (해법 확정)

**방법**: 최상위 창이 아니라 **진짜 포커스 창**을 `AttachThreadInput`+`GetFocus` 로 구하고(프로세스
경계를 넘어 추적), 그 창의 스레드에 주입해 읽는다.

**결과** (각 창에서 한/영을 번갈아 실문자 입력하며 관찰):

| 포커스 대상 | 클래스 | vt | native 추종 |
|---|---|---|---|
| `explorer.exe` 주소창 | `Edit` | 3 | ✅ 정확 |
| `Notepad.exe` (포커스 스레드 t33032 ≠ fg t22556) | `RichEditD2DPT` | 3 | ✅ 정확 |
| **`msedgewebview2.exe`** (fg=`ms-teams.exe` t7072 → focus t4836) | `Chrome_WidgetWin_1` | **3** | ✅ **정확** |
| `SearchHost.exe` | `CoreWindow` | — | ❌ 주입 실패(AppContainer) |

**판정 — A 실현성 확정 (실측)**:

> 포커스 창의 스레드(다른 프로세스라도)에 주입해 in-process 로 읽으면 **Teams 의 실제 한/영
> 상태를 100% 정확히** 관측한다(`vt=3`, `hr=0`, 토글 완전 추종).

cross-process·자기프로세스·자가추적이 모두 실패한 문제를, A 가 처음으로 정확히 해결한다.
부수 효과: 매 토글 시점에 진실을 새로 읽으므로 **외부 토글(트레이 클릭·우-Alt 등) 드리프트 문제도
원천 소멸**한다(저장해 둔 상태가 없으니 어긋날 것도 없다 — §6 의 가장 큰 약점 해소).

**관측된 한계**:
- **샌드박스/상위 무결성 포커스**(예: `SearchHost.exe`=AppContainer, 관리자 권한 창)에는 주입 불가
  → 폴백 필요.
- **비트니스 일치** 필요(x64 대상엔 x64 DLL, arm64 엔 arm64 DLL). 본 프로젝트는 두 타겟을 이미 빌드.
- 전역 훅 주입은 **백신/EDR 오탐** 소지 → 배포 시 고려.

## 5. 아키텍처 결론 (실측 기반)

> 한/영 변환 모드는 **포커스를 가진 스레드의 입력 컨텍스트별 상태**다. "어디서든 읽을 수 있는 단일 전역 값"은 없고, 백그라운드 프로세스에서 읽어낼 대리 값도 없다. 전역感은 포커스 전환 시의 상속이 만드는 착시다.

이로써 다음 세 경로가 **막다른 길로 확정**됐다:

1. **IMM32 cross-process** — Chromium의 IMM32 스텁이 항상 0 반환 (1차 원인)
2. **TSF cross-process (langbar)** — `GetThreadLangBarItemMgr` E_FAIL (스파이크 #1)
3. **우리 자신 in-process 상태** — 백그라운드 프로세스엔 컨텍스트 없음 (4.3)
4. **순수 자가추적** — 우리가 토글의 유일 주체가 아님; 외부 토글(트레이 IME 버튼 클릭, 우-Alt, 타 전환기)로 드리프트하며 Chromium에선 재동기 불가

**단, 상태가 사는 프로세스 *안에서* 실행되면(=A, 포커스 스레드 주입) 읽을 수 있음이 §4.4–4.5 에서
실측 확정됐다.** 위 4개는 "외부에서 가볍게" 읽으려는 시도였고, A 는 그 전제를 버리고 상태가 사는
곳으로 코드를 옮겨 정확히 읽는다.

## 6. 남은 선택지 — 트레이드오프

| | 접근 | 정확도 | 비용 / 리스크 |
|---|---|---|---|
| **A** | **포커스 스레드에 in-process TSF 리더 주입** — 진짜 포커스 창(다른 프로세스 가능)의 스레드에 훅 DLL 주입, 그 안에서 compartment 를 읽어 shared memory 로 회신 | ✅ **실측 검증됨**(§4.5) — Teams 포함 100% 정확 | 침습적(다른 프로세스에 DLL 로드), x64/arm64 비트 일치 필요, **백신 오탐 소지**, 샌드박스/상위 무결성 포커스엔 주입 불가 |
| **B** | **시스템 트레이 입력 표시기 관측** — `SetWinEventHook`(변경 이벤트) + UIAutomation으로 "한"/"A" 읽기 | △ 권위 있는 진실(작업표시줄)을 읽음 | A 가 더 직접·정확하므로 **보류**. UI/로케일/버전 결합, 표시기 숨김 시 취약 |
| **C** | 자가추적 + 키보드 `VK_HANGUL` 이벤트까지 추적 | ✗ 트레이 클릭·프로그램적 변경은 사각 | 가벼움. 정확성 우선 기준에선 탈락 |

핵심: **A 가 실측으로 정확성이 입증된 유일한 경로**다(§4.4–4.5). on-demand(토글 시점에만 포커스
스레드에 잠깐 주입) 모델이면, "모든 프로세스에 상주" 같은 최악의 침습은 피하면서 정확성을 얻는다.
주입이 막히는 포커스(AppContainer/상위 무결성)에 한해 폴백이 필요하다.

## 7. 현재 상태 / 다음 단계

- #1(TSF langbar cross-process)·자기프로세스 읽기 **폐기**, #2/#2b(주입+포커스 스레드 읽기)로
  **A 실현성 확정**. B(트레이 UIA)는 A 가 더 정확·직접적이라 보류.
- 다음: **A 생산 구현 설계/반영**. 결정·구현 포인트:
  - **주입 모델**: on-demand — 사용자가 한/영을 토글하는 그 순간에만 포커스 스레드에 잠깐 주입·읽고
    필요 시 해제. 상시 전역 주입보다 침습/오탐/안정성 위험이 작고, 매 토글마다 진실을 새로 읽으므로 충분.
  - **라벨 타이밍**: 토글 직전 실제 상태 S 를 읽고 `!S` 표시(설정 비동기 갱신 레이스 회피). 기존
    `handle_language_toggle` 구조와 동일, 조회 채널만 IMM32 → 주입 리더로 교체.
  - **폴백**: 주입 불가 포커스(AppContainer/상위 무결성)면 내부 추정값으로 표시.
  - **배포 패키징**: 단일 exe(DLL 임베드 후 런타임 추출) vs 2파일(exe+dll 동봉) — **결정 필요**.
  - **크래시 안전**: 주입 DLL 은 호스트(타 프로세스)에서 돌므로 **절대 패닉 금지**(`catch_unwind`/방어적).
  - **비트니스**: x64·arm64 각 DLL 필요(본체가 이미 듀얼 타겟 빌드).
- 본체 코드(`src/overlay.rs`의 `query_ime_hangul` 기반 로직)는 아직 미변경.

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
| `examples/tsf_spike.rs` | 전체 | TSF cross-process / 자기프로세스 읽기 검증 스파이크 (#1) |
| `spikes/tsf_probe_dll/` | 전체 | 포커스 스레드에 주입돼 in-process 로 변환 모드를 읽는 DLL (#2) |
| `spikes/inject_driver/` | 전체 | 포커스 스레드 추적 + 주입 + 결과 회신 검증 드라이버 (#2b) |
