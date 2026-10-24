# SPDX-License-Identifier: GPL-3.0-or-later
# Review aid: tiles work/stills/*.png into work/stills/sheet.png (3 per row, half size).
import glob, sys
from PIL import Image
fs = sorted(glob.glob('work/stills/still_*.png'), key=lambda f: float(f.split('_')[-1][:-5]))
ims = [Image.open(f).resize((480, 300), Image.LANCZOS) for f in fs]
W = Image.new('RGB', (480 * 3, 300 * ((len(ims) + 2) // 3)))
for i, im in enumerate(ims): W.paste(im, ((i % 3) * 480, (i // 3) * 300))
W.save('work/stills/sheet.png')
print(len(ims), 'stills')
