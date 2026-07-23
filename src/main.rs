// 릴리스 빌드에서는 콘솔 창 없이 실행한다(§13.3).
// 디버그 빌드에서는 콘솔을 유지하여 로그를 확인할 수 있다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[macro_use]
mod wide; // w! 매크로를 다른 모듈보다 먼저 선언해 크레이트 전역에서 쓰게 한다.

mod config;
mod hook;
mod ime;
mod input;
mod logging;
mod overlay;
mod single_instance;
mod state;
mod tray;
mod update;
mod win32;

use config::Config;
use hook::KeyboardHook;
use single_instance::SingleInstance;

/// 프로그램 진입점 (§5.2 main.rs, §10 프로세스 수명 주기).
///
/// 1. 중복 실행 방지 (named mutex)
/// 2. 설정 로드
/// 3. WH_KEYBOARD_LL 훅 설치
/// 4. 메시지 루프 실행
/// 5. 종료 시 훅 해제 (Drop)
fn main() {
    // 0. HiDPI 인식 설정 (오버레이를 또렷하게 렌더링하기 위해 창 생성 전에 호출).
    win32::set_dpi_aware();

    // 1. 중복 실행 방지 (§11)
    let _instance = match SingleInstance::acquire() {
        Ok(Some(instance)) => instance,
        Ok(None) => {
            logging::log("이미 다른 인스턴스가 실행 중입니다. 종료합니다.");
            return;
        }
        Err(code) => {
            logging::log(&format!("mutex 생성 실패: error {code}"));
            return;
        }
    };

    // 2. 설정 로드 (§9 초기 버전은 컴파일 타임 기본값)
    let config = Config::default();
    state::init(&config);

    // 3. 키보드 훅 설치 (§5.1, §6)
    let _hook = match KeyboardHook::install() {
        Ok(hook) => hook,
        Err(code) => {
            logging::log(&format!("키보드 훅 설치 실패: error {code}"));
            return;
        }
    };

    // 3-1. 전환 안내 HUD 오버레이 준비(실패해도 치명적이지 않음 — HUD 없이 동작).
    if let Err(code) = overlay::init() {
        logging::log(&format!("오버레이 초기화 실패: error {code} (HUD 없이 계속 실행)"));
    }

    // 3-2. IME 한/영 상태 리더 준비(주입 DLL). 실패해도 치명적이지 않으며(추정값 폴백),
    //      Teams 등 TSF/Chromium 앱에서 실제 한/영 상태를 정확히 읽기 위해 쓴다.
    if !ime::init() {
        logging::log("IME 리더 초기화 실패 (한/영 라벨은 추정값 폴백으로 동작)");
    }

    // 3-3. 시스템 트레이 아이콘(우클릭 메뉴 + 정보 다이얼로그). 실패해도 치명적이지 않음.
    if let Err(code) = tray::init() {
        logging::log(&format!("트레이 아이콘 초기화 실패: error {code} (트레이 없이 계속 실행)"));
    }

    logging::log("caps-hangul-rs 실행 중. 트레이 아이콘 우클릭 → Exit 또는 프로세스 종료.");

    // 3-4. 초기화로 작업 집합에 올라온 1회성 페이지(std/GDI 초기화 등)를 idle 진입 직전에
    //      비운다. 상주 트레이 앱은 대부분 idle 이라, 안 쓰는 페이지를 standby 로 내려
    //      보고되는 메모리(작업 집합)를 최소로 유지한다. 필요한 페이지는 자동으로 다시
    //      fault-in 되므로 정확성에는 영향이 없다(다음 상호작용이 미세하게만 느려질 수 있음).
    win32::trim_working_set();

    // 4. 메시지 루프 (§10.2) — idle 상태에서 CPU 사용률은 사실상 0%.
    win32::run_message_loop();

    // 5. 정리: 트레이 아이콘 제거 + IME 리더 자원 해제 + _hook / _instance 의 Drop (§10.3)
    tray::shutdown();
    ime::shutdown();
}
