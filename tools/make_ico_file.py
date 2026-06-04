# -*- coding: utf-8 -*-
"""1024px 마스터 PNG에서 멀티 사이즈 Windows ICO 생성.

각 사이즈를 마스터에서 LANCZOS로 직접 렌더한 뒤 프레임으로 임베드한다
(Pillow 내부 리샘플링에 맡기지 않고 품질을 직접 제어).

사용법:
    python tools/make_ico.py                                  # 기본 경로 사용
    python tools/make_ico.py assets/caps_lock_icon.png        # 소스 지정
    python tools/make_ico.py <src.png> <dst.ico>              # 소스/출력 지정
"""
import sys
from PIL import Image

SRC = sys.argv[1] if len(sys.argv) > 1 else r"assets/caps_lock_icon.png"
DST = sys.argv[2] if len(sys.argv) > 2 else r"assets/caps_lock_icon.ico"
SIZES = [16, 24, 32, 48, 64, 128, 256]   # 윈도우 표준 아이콘 사이즈


def main():
    master = Image.open(SRC).convert("RGBA")
    if master.size[0] < max(SIZES):
        raise SystemExit(f"마스터가 너무 작음: {master.size} (>= {max(SIZES)} 필요)")

    # 각 사이즈를 마스터에서 LANCZOS로 직접 렌더
    by_size = {s: master.resize((s, s), Image.LANCZOS) for s in SIZES}

    big = max(SIZES)
    base = by_size[big]                                   # 256 프레임
    appended = [by_size[s] for s in SIZES if s != big]    # 나머지 프레임
    # sizes 의 각 항목과 정확히 일치하는 프레임을 제공하면
    # Pillow 는 재리샘플 없이 그 프레임을 그대로 임베드한다.
    base.save(DST, format="ICO",
              sizes=[(s, s) for s in SIZES], append_images=appended)
    print("saved", DST, "sizes", SIZES)


if __name__ == "__main__":
    main()
