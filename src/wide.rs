//! 컴파일 타임 UTF-16 문자열 리터럴 (힙 할당 없는 wide 문자열).
//!
//! Win32 의 W-API 는 널 종료 UTF-16(`*const u16`)을 받는다. 런타임에 `Vec<u16>` 로
//! 변환(`encode_utf16().collect()`)하면 **호출마다 힙 할당**이 생기는데, 문자열이
//! 컴파일 타임에 고정이면 그럴 이유가 없다. [`w!`] 매크로는 리터럴/상수를 `.rdata` 의
//! `static [u16; N]` 로 박아 두고 그 포인터를 돌려준다(할당 0, `'static` 수명이라
//! 클래스명처럼 등록 후에도 살아 있어야 하는 포인터에도 더 안전하다).
//!
//! 입력이 런타임 `String`(예: `format!` 결과/경로)인 경우엔 const 화가 불가능하므로
//! 종전대로 각 모듈의 `wide()`/`encode_wide()` 를 쓴다.

/// `&str` 의 UTF-16 코드 유닛 개수(널 종료 제외). const 평가 가능.
pub const fn utf16_len(s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    let mut n = 0;
    while i < b.len() {
        let c = b[i];
        if c < 0x80 {
            i += 1;
            n += 1;
        } else if c < 0xE0 {
            i += 2;
            n += 1;
        } else if c < 0xF0 {
            i += 3;
            n += 1;
        } else {
            i += 4;
            n += 2; // BMP 밖 → 서로게이트 페어(2 유닛)
        }
    }
    n
}

/// `&str` → 널 종료 UTF-16 배열(`N == utf16_len(s) + 1`). const 평가 가능.
///
/// 입력은 항상 유효한 UTF-8(`&str` 보장)이므로 continuation 바이트 접근은 범위 안이다.
pub const fn utf16_arr<const N: usize>(s: &str) -> [u16; N] {
    let b = s.as_bytes();
    let mut out = [0u16; N];
    let mut i = 0; // 입력 바이트 인덱스
    let mut o = 0; // 출력 유닛 인덱스
    while i < b.len() {
        let c0 = b[i] as u32;
        let cp;
        if c0 < 0x80 {
            cp = c0;
            i += 1;
        } else if c0 < 0xE0 {
            cp = ((c0 & 0x1F) << 6) | (b[i + 1] as u32 & 0x3F);
            i += 2;
        } else if c0 < 0xF0 {
            cp = ((c0 & 0x0F) << 12) | ((b[i + 1] as u32 & 0x3F) << 6) | (b[i + 2] as u32 & 0x3F);
            i += 3;
        } else {
            cp = ((c0 & 0x07) << 18)
                | ((b[i + 1] as u32 & 0x3F) << 12)
                | ((b[i + 2] as u32 & 0x3F) << 6)
                | (b[i + 3] as u32 & 0x3F);
            i += 4;
        }
        if cp < 0x10000 {
            out[o] = cp as u16;
            o += 1;
        } else {
            let v = cp - 0x10000;
            out[o] = (0xD800 + (v >> 10)) as u16;
            out[o + 1] = (0xDC00 + (v & 0x3FF)) as u16;
            o += 2;
        }
    }
    out // 남은 마지막 칸은 0 → 널 종료
}

/// 컴파일 타임 널 종료 UTF-16 리터럴/상수 → `*const u16` (힙 할당 없음, `'static`).
///
/// `$s` 는 const 평가 가능한 `&str`(문자열 리터럴 또는 `const NAME: &str`)이어야 한다.
/// 런타임 값을 넘기면 const 컨텍스트에서 컴파일 에러가 난다(의도된 가드).
#[macro_export]
macro_rules! w {
    ($s:expr) => {{
        const N: usize = $crate::wide::utf16_len($s) + 1;
        static ARR: [u16; N] = $crate::wide::utf16_arr::<N>($s);
        ARR.as_ptr()
    }};
}

#[cfg(test)]
mod tests {
    /// ASCII / 한글(BMP) / 혼합이 기대한 UTF-16 시퀀스로 변환되고 널 종료되는지.
    #[test]
    fn encodes_and_null_terminates() {
        // "A한" → [0x0041, 0xD55C, 0x0000]
        let p = w!("A한");
        let s = unsafe { std::slice::from_raw_parts(p, 3) };
        assert_eq!(s, &[0x0041u16, 0xD55C, 0x0000]);
    }

    #[test]
    fn matches_runtime_encode_utf16() {
        for lit in ["", "Segoe UI", "Malgun Gothic", "CAPS OFF", "한", "E&xit"] {
            let want: Vec<u16> = lit.encode_utf16().chain(std::iter::once(0)).collect();
            let p = match lit {
                "" => w!(""),
                "Segoe UI" => w!("Segoe UI"),
                "Malgun Gothic" => w!("Malgun Gothic"),
                "CAPS OFF" => w!("CAPS OFF"),
                "한" => w!("한"),
                "E&xit" => w!("E&xit"),
                _ => unreachable!(),
            };
            let got = unsafe { std::slice::from_raw_parts(p, want.len()) };
            assert_eq!(got, want.as_slice(), "mismatch for {lit:?}");
        }
    }
}
