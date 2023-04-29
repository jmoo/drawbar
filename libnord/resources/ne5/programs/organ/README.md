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
abcde_fffffffff.ne5p

a: preset 1/2 (1, 2)
b: model (0: b3, 1: b3+bass, 2: pipe, 3: vox, 4: farf)
c: rotary speed (0: slow, 1: fast)
d: rotary stop mode (0: off, 1: on)
e: draw bar positions (0..8)
```

Drawbar aliases
```
x = 8 physical / 0 real
y = 8 physical / 1 real
z = 5 physical / 1 real
w = 4 physical / 0 real
```

Drawbar contraints
```
Lower:
b3 -> 888888888
b3+bass -> 880000000
pipe -> 888888888
vox -> 888888808
farf -> 111111111

Upper:
b3 -> 888888888
b3+bass -> 888888888
pipe -> 888888888
vox -> 888888808
farf -> 111111111
```