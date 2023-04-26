# Program naming convention

### Center panel tests
```
abc_d_abc_d_x_y_z.ne5p
([lower][upper][transpose][split][part-volume]).ne5p

# lower/upper
a = part instrument (n, o, p, s)
b = sustain (0,1)
c = control (0,1)
d = octave (-5,0,5 or 6 or 7?)

# global
x = transpose (-6..6)
y = split (0:off, 1:c3, 2:f3, 3:c4, 4:f4, 5c5, 6:f5, 7:upper)
z - part volume (-50..50)
```

### Part mix tests
```
mix a b.ne5p
a = lower (0..50)
b = upper (0..50)
```

### Gain tests
```
gain a.ne5p

a = gain (0..10)
```

### FX panel tests
```
fx1 abc d.ne5p

a = part select (0: low, 1: up)
b = control (0,1)
c = type (0: pan1, 1: pan2, 2: pan1&2, 3: wah, 4: rm, 5: trem1, 6: trem2, 7: trem1&2)
d = rate (0..10)
```

```
fx2 abc d.ne5p

a = part select (0: low, 1: up)
b = deep (0,1)
c = type (0: flang, 1: choir1, 2: choir2, 3: vibe, 4: phas1, 5: phas2)
d = rate (0..10.5Hz)
```

```
fx3 abc d.ne5p

a = part select (0: low, 1: up)
b = drive on/off (0,1)
c = (0: none, 1: twin, 2: rotary, 3: comp, 4: small, 5: jc)
d = compression (0..10)
```

```
fx4 abc d e.ne5p

a = part select (0: low, 1: up)
b = ping pong (0,1)
c = feedback (0,1,2,3)
d = dry/wet (0..10)
e = tempo (20ms...750ms)
```

