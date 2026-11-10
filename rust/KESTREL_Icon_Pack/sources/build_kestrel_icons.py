from pathlib import Path
from PIL import Image, ImageDraw, ImageFont
import numpy as np
from scipy import ndimage as ndi
import json, shutil, zipfile

ROOT = Path(__file__).resolve().parent
REBUILD = (ROOT / 'kestrel-generated-app.png').exists()
OUT = ROOT.parent.parent / 'KESTREL_Icon_Pack_Rebuilt' if REBUILD else ROOT / 'KESTREL_Icon_Pack'
MAIN = ROOT / 'kestrel-generated-app.png' if REBUILD else ROOT / 'generated_images/exec-1b56131b-bbf4-4400-a514-d25c829ae063.png'
TRAY = ROOT / 'kestrel-generated-tray.png' if REBUILD else ROOT / 'generated_images/exec-2b5c66ab-c75d-467c-a8e1-d3216dc937f4.png'
APP_SIZES = [16,20,24,32,40,48,64,96,128,192,256,512,1024]
TRAY_SIZES = [16,20,24,32,40,48,64]
STATES = ['idle','downloading','paused','complete','error']
THEMES = {'dark-taskbar': '#FFFFFF', 'light-taskbar': '#182434'}
for folder in ['app/png','app/windows','app/macos','tray','sources']:
    (OUT / folder).mkdir(parents=True, exist_ok=True)

def fitted(im, size, coverage):
    alpha = im.getchannel('A')
    box = alpha.point(lambda v: 255 if v > 160 else 0).getbbox()
    im = im.crop(box)
    im.thumbnail((round(size*coverage),round(size*coverage)),Image.Resampling.LANCZOS)
    result = Image.new('RGBA',(size,size))
    result.alpha_composite(im,((size-im.width)//2,(size-im.height)//2))
    return result

app = fitted(Image.open(MAIN).convert('RGBA'),1024,0.9)
app.save(OUT/'app/kestrel-master-1024.png')
for size in APP_SIZES:
    app.resize((size,size),Image.Resampling.LANCZOS).save(OUT/f'app/png/kestrel-{size}.png')
ico_sizes = [(s,s) for s in APP_SIZES if s <= 256]
app.save(OUT/'app/windows/kestrel.ico',format='ICO',sizes=ico_sizes)
app.save(OUT/'app/macos/kestrel.icns',format='ICNS')
shutil.copy2(MAIN,OUT/'sources/kestrel-generated-app.png')
shutil.copy2(TRAY,OUT/'sources/kestrel-generated-tray.png')

# Export the generated white silhouette as a clean monochrome alpha mask.
# Closing seals fine generated feather seams; connected-component filtering
# removes isolated antialias specks outside the intended silhouette.
arr = np.asarray(Image.open(TRAY).convert('RGBA'))
mask = (arr[:,:,:3].mean(axis=2)>150) & (arr[:,:,3]>150)
mask = ndi.binary_closing(mask,iterations=4)
mask = ndi.binary_opening(mask,iterations=2)
labels,_ = ndi.label(mask)
counts = np.bincount(labels.ravel()); counts[0]=0
mask = labels == counts.argmax()
holes = ndi.binary_fill_holes(mask) & ~mask
hl,n = ndi.label(holes)
for i in range(1,n+1):
    if np.count_nonzero(hl==i) < 1700:
        mask[hl==i] = True
alpha = Image.fromarray((mask*255).astype('uint8'))
base = Image.new('RGBA',alpha.size,'white'); base.putalpha(alpha)
base = fitted(base,1024,0.88)
base.save(OUT/'sources/kestrel-tray-silhouette.png')

def tray_icon(theme,state):
    fg = THEMES[theme]
    mark = Image.new('RGBA',base.size,fg)
    mark.putalpha(base.getchannel('A'))
    icon = Image.new('RGBA',(1024,1024))
    if state=='idle':
        icon.alpha_composite(mark)
        return icon
    # Keep a consistent mark and reserve the lower-right for a status badge.
    mark = mark.resize((850,850),Image.Resampling.LANCZOS)
    icon.alpha_composite(mark,(0,0))
    cx,cy,r = 754,754,242
    d = ImageDraw.Draw(icon)
    d.ellipse((cx-r-32,cy-r-32,cx+r+32,cy+r+32),fill=(0,0,0,0))
    colors = {'downloading':'#19D9EE','paused':'#FFC451','complete':'#20B978','error':'#E94D63'}
    d.ellipse((cx-r,cy-r,cx+r,cy+r),fill=colors[state])
    ink = '#10202E' if state in ['downloading','paused'] else '#FFFFFF'
    if state=='downloading':
        d.rounded_rectangle((cx-37,cy-140,cx+37,cy+25),radius=12,fill=ink)
        d.polygon([(cx-135,cy-5),(cx+135,cy-5),(cx,cy+133)],fill=ink)
    elif state=='paused':
        for x in [cx-105,cx+35]:
            d.rounded_rectangle((x,cy-118,x+70,cy+118),radius=12,fill=ink)
    elif state=='complete':
        d.line([(cx-125,cy),(cx-30,cy+95),(cx+135,cy-100)],fill=ink,width=68,joint='curve')
    else:
        d.rounded_rectangle((cx-34,cy-140,cx+34,cy+35),radius=13,fill=ink)
        d.ellipse((cx-36,cy+81,cx+36,cy+153),fill=ink)
    return icon

for theme in THEMES:
    folder=OUT/'tray'/theme
    folder.mkdir(parents=True,exist_ok=True)
    for state in STATES:
        icon=tray_icon(theme,state)
        icon.save(folder/f'kestrel-tray-{state}.ico',format='ICO',sizes=[(s,s) for s in TRAY_SIZES])
        for size in TRAY_SIZES:
            icon.resize((size,size),Image.Resampling.LANCZOS).save(folder/f'kestrel-tray-{state}-{size}.png')

manifest = {
    'brand':'KESTREL',
    'app_png_sizes':APP_SIZES,
    'windows_ico_frames':[s for s in APP_SIZES if s<=256],
    'tray_png_and_ico_sizes':TRAY_SIZES,
    'tray_themes':THEMES,
    'tray_states':{
        'idle':'Falcon alone; no active transfers',
        'downloading':'Cyan circle and down arrow; active transfers',
        'paused':'Amber circle and pause bars; work pending but paused',
        'complete':'Green circle and check; queue just finished',
        'error':'Red circle and exclamation; user attention required'
    },
    'note':'PNG sizes are antialiased exports from the masters, not individually hand-drawn pixel variants.'
}
(OUT/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')

readme='''# KESTREL icon pack

## App assets
- `app/windows/kestrel.ico`: a multi-resolution Windows icon containing 16, 20, 24, 32, 40, 48, 64, 96, 128, 192 and 256 px frames.
- `app/png/`: transparent PNG exports at 16, 20, 24, 32, 40, 48, 64, 96, 128, 192, 256, 512 and 1024 px.
- `app/macos/kestrel.icns`: macOS icon container.
- `app/kestrel-master-1024.png`: padded transparent master.

## Tray recommendation
Use the compact falcon silhouette for the notification area, with a status badge. The globe belongs in the full app icon; the simple silhouette is easier to read at tray sizes.

Choose `tray/dark-taskbar` for a white falcon on dark backgrounds, or `tray/light-taskbar` for a dark falcon on light backgrounds. Each theme includes 16, 20, 24, 32, 40, 48 and 64 px PNG exports and a multi-resolution ICO per state.

| State | Badge |
| --- | --- |
| Idle | None |
| Downloading | Cyan down arrow |
| Paused | Amber pause bars |
| Complete | Green check |
| Error | Red exclamation |

The badge shape communicates state in addition to color. Use one tray icon for the whole application.

Suggested aggregate-state priority: error requiring attention, then active downloads, then pending paused downloads, then recent completion, then idle. A recoverable automatic retry need not replace the downloading state with an error. Show Complete briefly after the queue finishes, then return to Idle. Do not report completion while downloads remain paused. Closing the window should leave transfers running only when the user has enabled background operation.

Suggested tooltip: `KESTREL — 3 downloads • 12.4 MB/s`.
Suggested context menu: Open KESTREL; Add URL; Pause all; Resume all; Speed limit; Open downloads folder; Settings; Quit. Disable actions that do not apply. If Quit would interrupt active transfers, offer to keep downloading or quit and preserve resumable work.

For Windows, load the correct frame for the current DPI rather than stretching a 16 px bitmap. Microsoft recommends providing both 16 and 32 px resources for notification icons and using LoadIconMetric for appropriate loading/scaling:
https://learn.microsoft.com/en-us/windows/win32/shell/notification-area

## Production notes
The colorful app artwork and tray base were generated with the built-in image tool, then exported programmatically. Tray alpha was normalized for monochrome use; status symbols were drawn as geometric badges. The PNG exports are resized masters, not individually hand-hinted pixel art. Files were checked for dimensions, transparency and ICO frames, and previewed against light/dark backgrounds. They have not been tested inside a running Windows application; test the chosen tray icon on actual taskbars and display scales before release.

`sources/` contains the generated masters, the normalized tray silhouette and the generation prompts. Run `sources/build_kestrel_icons.py` to reproduce the exports in a sibling `KESTREL_Icon_Pack_Rebuilt` folder. It requires Python, Pillow, NumPy and SciPy. The selected icons are delivered as PNG/ICO/ICNS, not editable vector artwork.
'''
(OUT/'README.md').write_text(readme)

# Exact-size layout preview: a deterministic contact sheet, not new artwork.
W,H=1600,1190
preview=Image.new('RGB',(W,H),'#101722')
d=ImageDraw.Draw(preview)
font_path='/usr/share/fonts/truetype/dejavu/'
def font(size,bold=False):
    return ImageFont.truetype(font_path+('DejaVuSans-Bold.ttf' if bold else 'DejaVuSans.ttf'),size)
def text(x,y,s,size=20,color='#BBC8D9',bold=False):
    d.text((x,y),s,font=font(size,bold),fill=color)
text(52,34,'KESTREL',42,'#F7FBFF',True)
text(54,92,'Desktop icon + notification-area states',22)
preview.paste(app.resize((312,312),Image.Resampling.LANCZOS),(45,145),app.resize((312,312),Image.Resampling.LANCZOS))
text(60,475,'APP ICON',18,'#8CA5BF',True)
text(400,155,'PNG exports at actual pixel size',21,'#FFFFFF',True)
x=404
for size in [128,96,64,48,32,24,20,16]:
    icon=Image.open(OUT/f'app/png/kestrel-{size}.png')
    preview.paste(icon,(x,315-size),icon)
    text(x,332,str(size),15)
    x+=size+33
d.rounded_rectangle((400,365,1536,485),radius=15,fill='#EFF3F8')
text(421,381,'Small exports on a light background',18,'#33465C',True)
x=425
for size in [64,48,32,24,20,16]:
    icon=Image.open(OUT/f'app/png/kestrel-{size}.png')
    preview.paste(icon,(x,472-size),icon)
    x+=size+40
text(52,540,'TRAY ICONS',24,'#F7FBFF',True)
text(52,578,'Same falcon identity, simplified for small sizes. Shape + color indicate state.',19)
for row,(theme,bg,fg) in enumerate([('dark-taskbar','#253143','#F4F8FF'),('light-taskbar','#F0F3F7','#182434')]):
    top=630+row*245
    d.rounded_rectangle((50,top,1550,top+218),radius=18,fill=bg)
    text(73,top+18,'DARK TASKBAR' if row==0 else 'LIGHT TASKBAR',16,fg,True)
    for i,state in enumerate(STATES):
        x=350+i*235
        icon=Image.open(OUT/f'tray/{theme}/kestrel-tray-{state}-64.png')
        preview.paste(icon,(x,top+47),icon)
        text(x-18,top+123,state.capitalize(),17,fg,True)
        small=Image.open(OUT/f'tray/{theme}/kestrel-tray-{state}-16.png')
        preview.paste(small,(x,top+170),small)
        text(x+27,top+168,'16 px',14,fg)
text(55,1145,'Transparent PNGs • multi-resolution Windows ICO • macOS ICNS • two tray themes • five states',17)
preview.save(ROOT/'KESTREL_Icon_Preview.png')
shutil.copy2(ROOT/'KESTREL_Icon_Preview.png',OUT/'KESTREL_Icon_Preview.png')
shutil.copy2(Path(__file__),OUT/'sources/build_kestrel_icons.py')

# Validate all PNG and ICO exports, including transparency and embedded sizes.
pngs=list((OUT/'app/png').glob('*.png'))+list((OUT/'tray').rglob('*.png'))
for path in pngs:
    im=Image.open(path)
    expected=int(path.stem.rsplit('-',1)[-1])
    assert im.size==(expected,expected),(path,im.size)
    assert im.mode=='RGBA',path
    assert im.getchannel('A').getextrema()==(0,255),path
    assert im.getpixel((0,0))[3]==0,path
for path in OUT.rglob('*.ico'):
    ico=Image.open(path)
    expected=ico_sizes if 'windows' in path.parts else [(s,s) for s in TRAY_SIZES]
    assert ico.ico.sizes()==set(expected),(path,ico.ico.sizes())
    for size in expected:
        assert ico.ico.getimage(size).size==size
assert Image.open(OUT/'app/macos/kestrel.icns').size==(1024,1024)
print(json.dumps({'png_exports_checked':len(pngs),'ico_files_checked':len(list(OUT.rglob('*.ico'))),'output':str(OUT)}))
