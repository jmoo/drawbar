# Sample synth test file naming convention
```
abc_dd_eee_fggg.ne5p

a = part (0: lower, 1: upper)
b = dynamics (0,1,2,3)
c = filter vel (on/off)
d = sample_id (01..F9)
e = attack (0: 0.5ms -> 127: 45s)

f/g = decay release
f = type (d: decay, s: sustain, r: release)
g = time (3ms..50s) ~39s max for release, ~43s max for decay
    (0: 3ms decay -> 64: sustain -> 127: 43s release)
```