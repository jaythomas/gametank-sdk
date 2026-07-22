.section .const.wavetables, "a"
.align 256
.global sine_table
sine_table:
    .incbin "../../instruments/sine.raw"

.align 256
.global saw_table
saw_table:
    .incbin "../../instruments/saw.raw"

.align 256
.global tri_table
tri_table:
    .incbin "../../instruments/triangle.raw"

.align 256
.global square_table
square_table:
    .incbin "../../instruments/square.raw"

.align 256
.global pulse_table
pulse_table:
    .incbin "../../instruments/pulse.raw"
