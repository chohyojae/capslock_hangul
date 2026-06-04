# -*- coding: utf-8 -*-
"""카툰 스타일 CAPS LOCK 키캡 아이콘 생성기.

- 진짜 투명 배경(alpha=0), 격자/체커보드 없음
- 두꺼운 검정 테두리의 통통한 3D 키캡 (카툰/스티커 느낌)
- 어두운 슬레이트 키 + 시안 "Caps Lock" (Comic Sans MS Bold)
- 4배 슈퍼샘플링 후 축소하여 부드러운 외곽선
"""
from PIL import Image, ImageDraw, ImageFont
import os

BASE = 1024          # 최종 크기
S = 4                # 슈퍼샘플 배율
W = BASE * S         # 작업 캔버스

# ---- 색상 ----
OUTLINE   = (222, 115, 86, 255)   # 두꺼운 테두리 (코랄)
BASE_FILL = (210, 196, 162, 255)  # 키 옆면/바닥 (어두운 크림/탄 — 셰이딩)
TOP_FILL  = (250, 245, 228, 255)  # 키 윗면 (아이보리)
GLOSS     = (255, 255, 255, 90)   # 윗면 광택
TEXT_FILL = (20, 94, 108, 255)    # 깊은 청록(틸) 글자 — 더 어둡게
TEXT_STRK = (222, 115, 86, 255)   # 글자 테두리 (키 테두리와 통일)

LOCAL_APPDATA = os.environ.get("LOCALAPPDATA", r"C:\Users\Default\AppData\Local")
FONT_PATH = os.path.join(LOCAL_APPDATA, "Microsoft", "Windows", "Fonts", "CherryBombOne-Regular.ttf")


def s(v):
    return int(round(v * S))


def rr(draw, box, radius, fill=None, outline=None, width=0):
    draw.rounded_rectangle(
        [s(box[0]), s(box[1]), s(box[2]), s(box[3])],
        radius=s(radius), fill=fill, outline=outline, width=s(width),
    )


def main():
    img = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    OUT_W = 26      # 바닥/턱 테두리 두께(1024 기준) — 두껍게
    OUT_W_TOP = 17  # 윗면 테두리 두께 — 더 얇게

    # 좌표(1024 기준)
    # 바닥 슬래브를 윗면보다 좌우(SIDE)·아래(LIP)로 더 크게 → 좌/우 얇은 림 + 아래 두꺼운 턱
    b_left, b_right = 100, 924
    base_top, base_bot = 214, 864     # 바닥 슬래브
    SIDE = 24                         # 좌/우 림 두께
    t_left, t_right = b_left + SIDE, b_right - SIDE
    top_top, top_bot = 176, 778       # 윗면 (위로 올려 입체감)
    base_rad, top_rad = 86, 70

    # 1) 바닥 슬래브: 채움 + 테두리
    rr(d, (b_left, base_top, b_right, base_bot), base_rad,
       fill=BASE_FILL, outline=OUTLINE, width=OUT_W)

    # 2) 윗면: 채움 + 테두리 (바닥 위에 겹쳐 그려 좌/우 림 + 앞쪽 턱이 보임)
    rr(d, (t_left, top_top, t_right, top_bot), top_rad,
       fill=TOP_FILL, outline=OUTLINE, width=OUT_W_TOP)

    # 3) 윗면 광택: 별도 레이어에서 그린 뒤 알파 합성(옅은 반사 streak)
    gloss = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    gd = ImageDraw.Draw(gloss)
    gd.rounded_rectangle(
        [s(t_left + 64), s(top_top + 40), s(t_left + (t_right - t_left) * 0.50), s(top_top + 146)],
        radius=s(52), fill=GLOSS,
    )
    img = Image.alpha_composite(img, gloss)
    d = ImageDraw.Draw(img)

    # 4) 텍스트 "Caps" / "Lock" 두 줄
    lines = ["Caps", "Lock"]
    # 윗면 안쪽 가용 폭/높이에 맞춰 폰트 크기 결정 (둘 다 만족하는 최대 크기)
    avail_w = s(t_right - t_left - 2 * OUT_W_TOP - 40)
    avail_h = s((top_bot - top_top) - 116)
    stroke = s(7)
    fs = s(192)
    while fs > 10:
        font = ImageFont.truetype(FONT_PATH, fs)
        boxes = [d.textbbox((0, 0), t, font=font, stroke_width=stroke) for t in lines]
        heights = [b[3] - b[1] for b in boxes]
        widest = max(b[2] - b[0] for b in boxes)
        gap = int(max(heights) * 0.18)
        total_h = sum(heights) + gap
        if widest <= avail_w and total_h <= avail_h:
            break
        fs -= 4

    face_cx = s((t_left + t_right) / 2)
    face_cy = s((top_top + top_bot) / 2) - s(4)
    y = face_cy - total_h / 2
    for t, b, h in zip(lines, boxes, heights):
        w = b[2] - b[0]
        # bbox 오프셋을 빼서 글리프가 (face_cx, y)에 딱 맞도록
        d.text((face_cx - w / 2 - b[0], y - b[1]), t, font=font,
               fill=TEXT_FILL, stroke_width=stroke, stroke_fill=TEXT_STRK)
        y += h + gap

    import sys
    out_path = sys.argv[1] if len(sys.argv) > 1 else r"assets/caps_lock_icon_preview.png"
    out = img.resize((BASE, BASE), Image.LANCZOS)
    out.save(out_path)
    print("saved", out_path, out.size)


if __name__ == "__main__":
    main()
