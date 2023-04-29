# FX test file naming convention
Effect 1
```
fx1_abc_d.ne5p

a = part select (0: off, 1: low, 2: upper)
b = control (0,1)
c = type (0: pan1, 1: pan2, 2: pan1&2, 3: wah, 4: rm, 5: trem1, 6: trem2, 7: trem1&2)
d = rate (0..10)
```

Effect 2
```
fx2_abc_d.ne5p

a = part select (0: off, 1: low, 2: upper)
b = deep (0,1)
c = type (0: flang, 1: choir1, 2: choir2, 3: vibe, 4: phas1, 5: phas2)
d = rate (0..10.5Hz)
```

Spkr/Comp
```
fx3_abc_d.ne5p

a = part select (0: off, 1: low, 2: upper)
b = drive on/off (0,1)
c = (0: none, 1: twin, 2: rotary, 3: comp, 4: small, 5: jc)
d = compression (0..10)
```

Delay
```
fx4_abc_d_e.ne5p

a = part select (0: off, 1: low, 2: upper)
b = ping pong (0,1)
c = feedback (0,1,2,3)
d = moisture (0..10)
e = tempo (20ms...750ms)
```

Reverb
```
fx5_a0c_d.ne5p

a = on/off (0: on, 1: off)
c = type (0: stage, 1: hall-soft, 2: hall, 3: room, 4: stage-soft)
d = moisture (0..10)
```