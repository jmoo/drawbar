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

