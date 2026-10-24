import math, cairosvg
cx,cy,rx,ry,rot=256,318,212,58,-12
def pt(a):
    a=math.radians(a); r=math.radians(rot)
    x,y=rx*math.cos(a), ry*math.sin(a)
    return (cx+x*math.cos(r)-y*math.sin(r), cy+x*math.sin(r)+y*math.cos(r))
def arc(a0,a1,n=64):
    pts=[pt(a0+(a1-a0)*i/n) for i in range(n+1)]
    return 'M '+' L '.join(f'{x:.1f} {y:.1f}' for x,y in pts)
back=arc(180,360)   # upper half (y negative = up)
front=arc(-8,168)   # lower half, front
dot=pt(-8)
A='#F28C28'; D='#1B1D24'
svg=f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">
  <!-- ModelCraft app icon (original artwork, MIT): isometric part with a bore and an orbit arc,
       bold dark line glyph on an amber rounded square. -->
  <rect x="16" y="16" width="480" height="480" rx="104" fill="{A}"/>
  <g fill="none" stroke="{D}" stroke-linecap="round" stroke-linejoin="round">
    <path d="{back}" stroke-width="16"/>
    <path d="M256 106 L362 167 L362 289 L256 350 L150 289 L150 167 Z" fill="{A}" stroke="none"/>
    <path d="M256 106 L362 167 L362 289 L256 350 L150 289 L150 167 Z" stroke-width="30"/>
    <path d="M150 167 L256 228 L362 167 M256 228 L256 350" stroke-width="30"/>
    <ellipse cx="256" cy="167" rx="38" ry="22" stroke-width="20"/>
    <path d="{front}" stroke="{A}" stroke-width="42"/>
    <path d="{front}" stroke-width="20"/>
  </g>
  <circle cx="{dot[0]:.1f}" cy="{dot[1]:.1f}" r="25" fill="{D}"/>
  <circle cx="{dot[0]:.1f}" cy="{dot[1]:.1f}" r="9" fill="{A}"/>
</svg>
'''
open('modelcraft.svg','w').write(svg)
for s in (16,24,32,48,64,128,256,512):
    cairosvg.svg2png(bytestring=svg.encode(), write_to=f'modelcraft_{s}.png', output_width=s, output_height=s)
