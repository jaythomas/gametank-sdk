.global audio_loop
.extern gain_table
.section .text

; Memory map (4KB = $0000 - $0FFF) as a Markdown table (matches linker.ld):
; | Range        | Size         | Purpose                   | Notes                                           |
; |--------------|--------------|---------------------------|-------------------------------------------------|
; | $0000-$0040  | $0041 (65)   | Reserved                  | header byte + low reserved region               |
; | $0041-$00FF  | $00C0 (192)  | Zero page (phases + vars) | PHASE_BASE = $0041; 7 voices × 2 bytes + temps  |
; | $0100-$01FF  | $0100 (256)  | CPU stack                 | hardware stack                                  |
; | $0200-$05FF  | $0400 (1024) | Gain levels (16 × 64)     | gain_table; placed by linker.ld's GAIN region   |
; | $0600-$0DFF  | $0800 (2048) | Wavetables (8 × 256)      | placed by linker.ld's WAVE region               |
; | $0E00-$0FF9  | $01FA (506)  | Code / other data (ARAM)  | Includes the voices' patchable parameters       |
; | $0FFA-$0FFF  | 6            | Vector table              | NMI/RESET/IRQ vectors                           |
;
; Addresses are little-endian, and ranges are inclusive.
;
;
; ============================ How a sample is built ==========================
;
; Each voice contributes `gain[level][wavetable[phase] >> 2]` to a running sum,
; and the DAC gets that sum once every voice has been folded in.
;
; Volume is the *address* of one of sixteen gain tables rather than a level, so
; scaling a sample is a single indexed load instead of arithmetic. Each table
; has 64 entries, so four of them are interleaved in every 256-byte block: the
; sample is masked with $FC (a multiple of 4, the same two cycles as a shift)
; to index the block, and the address's low byte (0-3) picks which of the four
; tables the load lands on. The tables are laid out so that a half-written
; address still lands on a valid one (see "Voice parameters live in the code"
; below), which makes volume changes safe without any locking.
;
; Gain page entries are in units of 1/4 DAC LSB, biased by +128 to keep them
; unsigned so they can be summed with plain ADC. The accumulator is 11 bits -
; its low byte in ACC_LO and its high byte parked in X - so all seven voices
; are summed *exactly* and the result is quantized to 8 bits only once, at the
; output. Rounding each voice separately (as the previous design did) made
; several voices playing the same note reinforce each other's rounding error
; instead of averaging it out, which is what coarsened the mix.
;
; The accumulator starts at -384 rather than 0, which folds two constants into
; the sum for free: it cancels the seven +128 entry biases and it adds the DAC's
; centre offset, so the final value needs only a shift, not any adjustment.
;   out = (sum - 7*128 + 512) / 4 = (sum - 384) / 4
;
; Gains are scaled so four voices at maximum volume reach full scale. Seven loud
; voices can therefore overshoot, which the finaliser resolves by saturating.
; Dividing by seven instead would make that impossible but would cost ~8.5dB
; that uncorrelated voices never actually use, and wraparound - the alternative
; to saturating - inverts the waveform, which is far worse than clipping it.

; Zero-page phase accumulators, 2 bytes per voice (low byte = fraction, high
; byte = wavetable index).
.set PHASE_BASE, 0x0041
.set VOICE_COUNT, 7

; Temporary ZP storage for the mix
.set ACC_LO, 0x004F       ; low byte of the 11-bit mix accumulator (high byte lives in X)

; Accumulator seed: -384, as a 16-bit two's complement value.
.set ACC_INIT_LO, 0x80
.set ACC_INIT_HI, 0xfe

.set DAC, 0x8040

; Power-on parameters: every voice is silent (gain level 0) and reads its own
; wavetable slot at the matching index.
.set GAIN_DEFAULT, 0x0200
.set WAVE_BASE, 0x0600

; ====================== Voice parameters live in the code =====================
;
; A voice's frequency, wavetable and gain are the operands of the instructions
; that use them (`adc #freq`, `lda wave,y`, `lda gain,y`), and the CPU patches
; them in place. Immediate and absolute-indexed operands cost fewer cycles than
; the zero-page and indirect ones they replace, and this is the hottest loop on
; the console. ARAM is plain RAM, so the code is writable.
;
; Every voice block below is exactly 31 bytes, the first starting at $0E03
; (right after `wai` and `ldx`), with the patchable operands at fixed offsets -
; the SDKs compute their addresses from these constants:
;
;   block + 4   frequency low byte   (operand of `adc #`)
;   block + 10  frequency high byte  (operand of `adc #`)
;   block + 15  wavetable address, 16-bit little-endian
;   block + 21  gain-table address,  16-bit little-endian
;
; Torn writes are harmless: a wavetable address always has a zero low byte and
; the gain addresses differ only in a low byte of 0-3 and a high byte of 2-5, so
; any mix of a stale and a fresh byte still lands on a real table. A volume
; change that lands mid-sample costs at most one sample at a different volume
; rather than a pop.

; Advance a voice's phase accumulator by its frequency increment, leaving the
; phase's new high byte in A.
.macro ADVANCE_PHASE n
    clc
    lda PHASE_BASE + 2 * \n
    adc #0                           ; frequency low byte (patched)
    sta PHASE_BASE + 2 * \n
    lda PHASE_BASE + 2 * \n + 1
    adc #0                           ; frequency high byte (patched)
    sta PHASE_BASE + 2 * \n + 1
.endm

; Scale this voice's raw sample (in A) by its volume and fold it into the mix.
; `acc` is the running total's low byte - the first voice passes an immediate
; instead, which seeds the accumulator as a side effect of its own add.
.macro ACCUMULATE_VOICE n, acc
    and #0xfc                        ; 6-bit index, interleaved 4 levels wide
    tay
    lda GAIN_DEFAULT, y              ; contribution, biased, in 1/4 DAC LSB (gain address patched)
    clc
    adc \acc
    sta ACC_LO
    bcc voice_\n\()_no_carry
    inx                              ; X is the accumulator's high byte
voice_\n\()_no_carry:
.endm

.macro PROCESS_VOICE n, acc
    ADVANCE_PHASE \n
    tay                              ; Y = phase high byte = wavetable index
    lda WAVE_BASE + 0x100 * \n, y    ; (wavetable address patched)
    ACCUMULATE_VOICE \n, \acc
.endm

; One sample is built per ACP interrupt, but the interrupt is never actually
; taken: interrupts stay masked, so `wai` just stalls the core and resumes at
; the next instruction when the sample timer fires. That skips the interrupt
; entry and `rti`, a dozen cycles of pure overhead per sample.
audio_loop:
    wai
    ; Seed the accumulator with -384; see the header comment for why. Only the
    ; high half needs setting up here - voice 0 adds itself to the low half's
    ; seed directly rather than to a stored running total.
    ldx #ACC_INIT_HI

voice_0_start:
    PROCESS_VOICE 0, #ACC_INIT_LO
voice_1_start:
    PROCESS_VOICE 1, ACC_LO
voice_2_start:
    PROCESS_VOICE 2, ACC_LO
voice_3_start:
    PROCESS_VOICE 3, ACC_LO
voice_4_start:
    PROCESS_VOICE 4, ACC_LO
voice_5_start:
    PROCESS_VOICE 5, ACC_LO
voice_6_start:
    PROCESS_VOICE 6, ACC_LO

    ; The mix now holds (sum - 384) in quarter-LSB units, so the DAC wants it
    ; shifted down by two - saturated rather than wrapped if the voices
    ; overshot in either direction. The high byte (0-3) supplies the top two
    ; bits of the result from a table, which is cheaper than shifting it in.
    cpx #4
    bcs audio_clip           ; X outside 0-3: the mix over- or undershot
    lda ACC_LO
    lsr a
    lsr a
    ora hi_bits, x
    sta DAC
    jmp audio_loop
audio_clip:
    txa
    bmi audio_clip_low       ; negative: the mix undershot
    lda #0xff
    bne audio_clip_store
audio_clip_low:
    lda #0x00
audio_clip_store:
    sta DAC
    jmp audio_loop

.section .rodata
.balign 4
hi_bits:
    .byte 0x00, 0x40, 0x80, 0xc0

.section .text
spurious_interrupt:
    rti

.global _start
_start:
    sei                    ; interrupts stay masked for good; see audio_loop
    cld                    ; clear decimal mode
    jmp audio_loop

; Vector table (must be at $FFFA-$FFFF)
.section .vector_table, "a"
    .word spurious_interrupt ; NMI vector ($FFFA-$FFFB)
    .word _start             ; RESET vector ($FFFC-$FFFD)
    .word spurious_interrupt ; IRQ/BRK vector ($FFFE-$FFFF)
