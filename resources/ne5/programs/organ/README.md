# Organ test file naming convention

Type-A (model: b3)
```
abcdefghhhhhhhhh.ne5p

a: preset 1/2 (1, 2)
b: drawb live
c: perc on/off
d: perc third on/off
e: perc speed (0: off, 1: soft, 2: fast, 3: both)
f: vib/chorus on/off
g: vib/chorus type (0: v2, 1: c2, 2: v3, 3: c3, 4: v1, 5: c1)
h: draw bar positions (0..8)
```

Type-B
```
abcd_fffffffff.ne5p

a: preset 1/2 (1, 2)
b: model (0: b3, 1: b3+bass, 2: pipe, 3: vox, 4: farf)
c: rotary speed (0: slow, 1: fast)
d: rotary stop mode (0: off, 1: on)
f: draw bar positions (0..8)
```

Type-C
```
abcd_efghi.ne5p
a: preset 1/2 (1, 2)
b: model (0: b3, 1: b3+bass, 2: pipe, 3: vox, 4: farf)
c: rotary speed (0: slow, 1: fast)
d: rotary stop mode (0: off, 1: on)
e: perc on/off
f: perc third on/off
g: perc speed (0: off, 1: soft, 2: fast, 3: both)
h: vib/chorus on/off
i: vib/chorus type (0: v2, 1: c2, 2: v3, 3: c3, 4: v1, 5: c1)
```

Type-D
```
abcde_fghijxxxxxxxxx_fghijxxxxxxxxx.ne5p

Global
a: selected preset 1/2 (1, 2)
b: model (0: b3, 1: b3+bass, 2: pipe, 3: vox, 4: farf)
c: rotary speed (0: slow, 1: fast)
d: rotary stop mode (0: off, 1: on)
e: drawb live

Upper/Lower
f: perc on/off
g: perc third on/off
h: perc speed (0: off, 1: soft, 2: fast, 3: both)
i: vib/chorus on/off
j: vib/chorus type (0: v2, 1: c2, 2: v3, 3: c3, 4: v1, 5: c1)
x: drawbar positions
```

Drawbar positions
```
0..8 = 0..8 physical / 0..8 real

r = 8 physical / 1 real
q = 7 physical / 1 real
p = 6 physical / 1 real
o = 5 physical / 1 real
n = 4 physical / 1 real
m = 3 physical / 1 real
l = 2 physical / 1 real
k = 1 physical / 1 real
j = 0 physical / 1 real

i = 8 physical / 0 real
h = 7 physical / 0 real
g = 6 physical / 0 real
f = 5 physical / 0 real
e = 4 physical / 0 real
d = 3 physical / 0 real
c = 2 physical / 0 real
b = 1 physical / 0 real
a = 0 physical / 0 real
```

Drawbar contraints
```
Lower:
b3 -> 888888888
b3+bass -> 880000000    # Bar numbers are inverted (8 <-> 0)
pipe -> 888888888
vox -> 888888808
farf -> 111111111       # Only stored as 0's and 1's, physical bar needs be to >= 5 trigger a 1

Upper:
b3 -> 888888888
b3+bass -> 888888888
pipe -> 888888888
vox -> 888888808
farf -> 111111111 
```

Effects constraints
```
b3 -> linked upper/lower, except perc and vib toggles
b3 bass -> linked upper/lower, except perc and vib toggles
pipe -> no effects
vox -> no perc, v1/v2/v3 only (no chorus), linked upper/lower except vib toggle
farf -> no perc, v1/v2/c2/c3 only, linked upper/lower except vib toggle

```