"""Generates Glidedesk app and tray icons (run once; outputs are committed)."""
from PIL import Image, ImageDraw, ImageFilter
import os, sys

OUT = sys.argv[1] if len(sys.argv) > 1 else "icons"
os.makedirs(OUT, exist_ok=True)

def app_icon(size):
    s = size * 4  # supersample
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    r = int(s * 0.225)
    # vertical gradient teal -> indigo
    grad = Image.new("RGBA", (s, s))
    gd = ImageDraw.Draw(grad)
    top, bot = (20, 184, 166), (79, 70, 229)
    for y in range(s):
        t = y / (s - 1)
        c = tuple(int(top[i] + (bot[i] - top[i]) * t) for i in range(3)) + (255,)
        gd.line([(0, y), (s, y)], fill=c)
    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).rounded_rectangle([int(s*0.06), int(s*0.06), int(s*0.94), int(s*0.94)], r, fill=255)
    img.paste(grad, (0, 0), mask)
    d = ImageDraw.Draw(img)
    # two screens
    w = int(s * 0.10)
    for x0 in (0.18, 0.56):
        d.rounded_rectangle([int(s*x0), int(s*0.30), int(s*(x0+0.26)), int(s*0.56)], int(s*0.03), outline=(255,255,255,235), width=w//3)
    # glide arc + cursor
    d.arc([int(s*0.30), int(s*0.40), int(s*0.70), int(s*0.80)], 200, 340, fill=(255,255,255,255), width=int(s*0.035))
    cx, cy = s*0.62, s*0.56
    arrow = [(cx, cy), (cx + s*0.16, cy + s*0.09), (cx + s*0.085, cy + s*0.11), (cx + s*0.12, cy + s*0.19),
             (cx + s*0.09, cy + s*0.205), (cx + s*0.055, cy + s*0.125), (cx, cy + s*0.17)]
    d.polygon(arrow, fill=(255,255,255,255))
    return img.resize((size, size), Image.LANCZOS)

def tray_icon(size, color=(0, 0, 0, 255)):
    s = size * 4
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    lw = max(4, s // 12)
    d.rounded_rectangle([int(s*0.06), int(s*0.22), int(s*0.44), int(s*0.62)], s//14, outline=color, width=lw)
    d.rounded_rectangle([int(s*0.56), int(s*0.22), int(s*0.94), int(s*0.62)], s//14, outline=color, width=lw)
    d.arc([int(s*0.18), int(s*0.40), int(s*0.82), int(s*0.98)], 200, 340, fill=color, width=lw)
    return img.resize((size, size), Image.LANCZOS)

for px in (32, 64, 128, 256, 512):
    app_icon(px).save(f"{OUT}/{px}x{px}.png")
app_icon(256).save(f"{OUT}/128x128@2x.png")
big = app_icon(1024)
big.save(f"{OUT}/icon.png")
big.save(f"{OUT}/icon.icns")
app_icon(256).save(f"{OUT}/icon.ico", sizes=[(16,16),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])
# macOS menu bar: black template image (system tints it); Windows tray: white + dark variants
tray_icon(22).save(f"{OUT}/tray-template.png")
tray_icon(44).save(f"{OUT}/tray-template@2x.png")
tray_icon(32, (255, 255, 255, 255)).save(f"{OUT}/tray-light.png")
tray_icon(32, (32, 32, 32, 255)).save(f"{OUT}/tray-dark.png")
# "stopped" template: glyph with a diagonal slash (monochrome so macOS can tint it)
for px, name in ((22, "tray-template-stopped.png"), (44, "tray-template-stopped@2x.png")):
    t = tray_icon(px); dd = ImageDraw.Draw(t); dd.line([(1, px - 2), (px - 2, 1)], fill=(0, 0, 0, 255), width=max(2, px // 11)); t.save(f"{OUT}/{name}")
st = tray_icon(32, (255,255,255,255)); ImageDraw.Draw(st).ellipse([20,20,31,31], fill=(239,68,68,255)); st.save(f"{OUT}/tray-light-stopped.png")
st = tray_icon(32, (32,32,32,255)); ImageDraw.Draw(st).ellipse([20,20,31,31], fill=(220,38,38,255)); st.save(f"{OUT}/tray-dark-stopped.png")
print("icons written to", OUT)
