.global audio_irq
.extern vol_table
.section .text

; Memory map (4KB = $0000 - $0FFF) as a Markdown table (matches linker.ld):
; | Range        | Size         | Purpose                  | Notes                                          |
; |--------------|--------------|---------------------------|------------------------------------------------|
; | $0000-$0040  | $0041 (65)   | Reserved                  | header byte + low reserved region              |
; | $0041-$00FF  | $00C0 (192)  | Zero page (voices + vars) | VOICE_BASE = $0041; 7 voices × 7 bytes + temps |
; | $0100-$01FF  | $0100 (256)  | CPU stack                | hardware stack                                 |
; | $0200-$02FF  | $0100 (256)  | Volume table              | vol_table (half-amplitude sine)                |
; | $0300-$0DFF  | $0B00 (2816) | Wavetables (11 × 256)     | placed by linker.ld's WAVE region              |
; | $0E00-$0FF9  | ~$01FA (506) | Code / other data (ARAM)  | Remaining program space before the vector table |
; | $0FFA-$0FFF  | 6            | Vector table              | NMI/RESET/IRQ vectors                          |
;
; Addresses are little-endian, and ranges are inclusive.

; Define the base address for the voices (zero page)
.set VOICE_BASE, 0x0041   ; zero-page base for voice control registers
.set VOICE_SIZE, 7        ; Each voice occupies 7 bytes
.set VOICE_COUNT, 7
.set VOICE_END, (VOICE_BASE + (VOICE_SIZE * VOICE_COUNT) - 1)  ; last byte used by voices (0x0071)

; Temporary ZP storage for IRQ
.set TEMP_SAMPLE, 0x0079  ; temporary storage for scaled sample
.set TEMP_RESULT1, 0x007a ; temporary storage for first vol_table result
.set TEMP_RESULT2, 0x007b ; temporary storage for second vol_table result

; Macro to define offsets for a voice
.macro DEFINE_VOICE voice_index
    .set VOICE_\voice_index\()_BASE, (VOICE_BASE + (VOICE_SIZE * \voice_index))
    .set VOICE_\voice_index\()_PHASE_L, (VOICE_\voice_index\()_BASE + 0)
    .set VOICE_\voice_index\()_PHASE_H, (VOICE_\voice_index\()_BASE + 1)
    .set VOICE_\voice_index\()_FREQ_L, (VOICE_\voice_index\()_BASE + 2)
    .set VOICE_\voice_index\()_FREQ_H, (VOICE_\voice_index\()_BASE + 3)
    .set VOICE_\voice_index\()_WAVEPTR_L, (VOICE_\voice_index\()_BASE + 4)
    .set VOICE_\voice_index\()_WAVEPTR_H, (VOICE_\voice_index\()_BASE + 5)
    .set VOICE_\voice_index\()_VOLUME, (VOICE_\voice_index\()_BASE + 6)
.endm

; Define 7 voices
DEFINE_VOICE 0
DEFINE_VOICE 1
DEFINE_VOICE 2
DEFINE_VOICE 3
DEFINE_VOICE 4
DEFINE_VOICE 5
DEFINE_VOICE 6

; Sample the next wavetable index and apply volume scaling to this tick's output sample
.macro PROCESS_VOICE_CORE n
    ; Advance phase accumulator by FREQ (16-bit addition)
    clc
    lda VOICE_\n\()_PHASE_L
    adc VOICE_\n\()_FREQ_L
    sta VOICE_\n\()_PHASE_L
    lda VOICE_\n\()_PHASE_H
    adc VOICE_\n\()_FREQ_H
    sta VOICE_\n\()_PHASE_H

    ; Get wavetable sample using phase_high as index into per-voice wavetable
    tay                    ; Y = phase high byte (table index)
    lda (VOICE_\n\()_WAVEPTR_L), y ; indirect indexed read from voice's wavetable

    ; Scale to 7-bit for volume scaling
    lsr a                  ; divide by 2
    sta TEMP_RESULT1       ; save scaled wavetable sample (s)

    ; index1 = s - volume
    sec
    sbc VOICE_\n\()_VOLUME
    sta TEMP_RESULT2       ; save index1
    tax
    lda vol_table, x       ; vol_table[s - v]
    tay                    ; cache in Y... cheaper than a second zero-page temp

    ; index2 = s + v, derived as 2s - index1
    ; No second volume read means no opportunity for the value to change under our noses
    lda TEMP_RESULT1       ; restore s
    asl a                  ; 2*s
    sec
    sbc TEMP_RESULT2       ; index2 = 2s - index1 = s + v
    tax
    lda vol_table, x       ; vol_table[s + v]
    sta TEMP_RESULT1       ; stash (s no longer needed)

    tya                    ; A = vol_table[s - v]
    sec
    sbc TEMP_RESULT1        ; A = vol_table[s-v] - vol_table[s+v]
.endm

audio_irq:
    PROCESS_VOICE_CORE 0
    clc
    adc #0x80              ; fold the center/silence value into voice 0's mix
    sta TEMP_SAMPLE

    PROCESS_VOICE_CORE 1
    clc
    adc TEMP_SAMPLE
    sta TEMP_SAMPLE

    PROCESS_VOICE_CORE 2
    clc
    adc TEMP_SAMPLE
    sta TEMP_SAMPLE

    PROCESS_VOICE_CORE 3
    clc
    adc TEMP_SAMPLE
    sta TEMP_SAMPLE

    PROCESS_VOICE_CORE 4
    clc
    adc TEMP_SAMPLE
    sta TEMP_SAMPLE

    PROCESS_VOICE_CORE 5
    clc
    adc TEMP_SAMPLE
    sta TEMP_SAMPLE

    PROCESS_VOICE_CORE 6
    clc
    adc TEMP_SAMPLE
    ; Output final mixed sample directly to the DAC instead
    ; of doing a round trip through the TEMP_SAMPLE
    sta 0x8040

    rti                    ; return from interrupt

; Simple main function that just waits
.section .text
.global _start
_start:
    sei                    ; disable interrupts during setup
    cld                    ; clear decimal mode
    
    ; Initialize stack pointer
    ldx #0xff
    txs
    
    ; Enable interrupts
    cli
    
main_loop:
    wai                    ; wait for interrupt
    jmp main_loop          ; loop forever

; Vector table (must be at $FFFA-$FFFF)
.section .vector_table, "a"
    .word audio_irq        ; NMI vector ($FFFA-$FFFB)
    .word _start           ; RESET vector ($FFFC-$FFFD)
    .word audio_irq        ; IRQ/BRK vector ($FFFE-$FFFF)
