// 릴리스 빌드에서는 콘솔 창 없이 실행한다(§13.3).
// 디버그 빌드에서는 콘솔을 유지하여 로그를 확인할 수 있다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod hook;
mod input;
mod logging;
mod single_instance;
mod state;
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

    logging::log("caps-hangul-rs 실행 중. 종료하려면 프로세스를 종료하세요.");

    // 4. 메시지 루프 (§10.2) — idle 상태에서 CPU 사용률은 사실상 0%.
    win32::run_message_loop();

    // 5. _hook / _instance 의 Drop 으로 훅 해제 및 mutex 정리 (§10.3)
}
