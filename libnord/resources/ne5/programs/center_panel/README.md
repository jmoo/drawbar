### Center panel test file naming convention

NOTICE: The default transpose value is 1 but with transpose disabled. transpose_enabled can be true while transpose is 0.
When this happens, the transpose light will be off.

```
abc_d_abct_d_x_y_z_z.ne5p

# lower/upper
a = part instrument (n, o, p, s, x: off)
b = sustain (0,1)
c = control (0,1)
d = octave (-5,0,5 or 6 or 7?)

# global
t = transpose enabled 
x = transpose (-6..6)
y = split (0:off, 1:c3, 2:f3, 3:c4, 4:f4, 5c5, 6:f5, 7:upper)
z - part mix lower (0..50)
z - part mix upper (0..50)
```