.global audio_loop
.extern gain_table
.section .text

; Memory map (4KB = $0000 - $0FFF) as a Markdown table (matches linker.ld):
; | Range        | Size         | Purpose                   | Notes                                           |
; |--------------|--------------|---------------------------|-------------------------------------------------|
; | $0000-$0040  | $0041 (65)   | Reserved                  | header byte + low reserved region               |
; | $0041-$00FF  | $00BF (191)  | Zero page (phases + vars) | PHASE_BASE = $0041; 7 voices × 2 bytes + temps  |
; | $0100-$06FF  | $0600 (1536) | Gain levels (24 × 64)     | gain_table; placed by linker.ld's GAIN region   |
; | $0700-$0EFF  | $0800 (2048) | Wavetables (8 × 256)      | placed by linker.ld's WAVE region               |
; | $0F00-$0FF9  | $00FA (250)  | Code / other data (ARAM)  | Includes the voices' patchable parameters       |
; | $0FFA-$0FFF  | 6            | Vector table              | NMI/RESET/IRQ vectors                           |
;
; Addresses are little-endian, and ranges are inclusive. There is no stack
; region: interrupts are masked for good and the loop never pushes, so page 1
; carries gain tables rather than sitting idle.
;
;
; ============================ How a sample is built ==========================
;
; Each voice contributes `gain[level][wavetable[phase]]` to a running sum,
; and the DAC gets that sum once every voice has been folded in.
;
; Volume is the index of one of 24 gain tables rather than a level, so
; scaling a sample is a single indexed load instead of arithmetic. Each table
; has 64 entries, so four of them are interleaved in every 256-byte block: the
; sample indexes the block directly and the address's low byte (0-3) picks
; which of the four tables the load lands on. That only works because every
; sample is a multiple of 4 - the SDKs clear the low two bits of each wavetable
; byte as they copy it into audio RAM, which buys back the `and #$fc` this loop
; would otherwise pay seven times a sample. Nothing is lost: the mask was
; already applied here, so samples have always been 6-bit at playback. The tables
; are laid out so that a half-written address still lands on a valid one
; which makes volume changes safe without any locking.
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
.set GAIN_DEFAULT, 0x0100
.set WAVE_BASE, 0x0700

; ====================== Voice parameters live in the code =====================
;
; A voice's frequency, wavetable and gain are the operands of the instructions
; that use them (`adc #freq`, `lda wave,y`, `lda gain,y`), and the CPU patches
; them in place. Immediate and absolute-indexed operands cost fewer cycles than
; the zero-page and indirect ones they replace, and this is the hottest loop on
; the console. ARAM is plain RAM, so the code is writable.
;
; Voices 1-6 are exactly 29 bytes each, back to back from $0F0B. Voice 0 is
; split in two (see "Carrying the rounding error" below), and the half that
; holds its patchable operands, 22 bytes, comes right after voice 6 at $0FB9.
; Every block has its patchable operands at fixed offsets - the SDKs compute
; their addresses from these constants:
;
;   block + 4   frequency low byte   (operand of `adc #`)
;   block + 10  frequency high byte  (operand of `adc #`)
;   block + 15  wavetable address, 16-bit little-endian
;   block + 19  gain-table address,  16-bit little-endian
;
; Torn writes are harmless: a wavetable address always has a zero low byte and
; the gain addresses differ only in a low byte of 0-3 and a high byte of 1-6, so
; any mix of a stale and a fresh byte still lands on a real table. A volume
; change that lands mid-sample costs at most one sample at a different volume
; rather than a pop.

; ===================== Carrying the rounding error =====================
;
; The DAC takes the top 8 bits of the 10-bit mix, so the bottom two are dropped.
; Dropping them unconditionally leaves quiet signals badly distorted: a voice a
; few LSBs tall is rendered as a coarse staircase, and adjacent volume levels
; land on irregular steps rather than the smooth curve the tables are built for.
;
; The fix is to not throw the error away. `lsr a` twice leaves the last bit it
; shifted out - the mix's half-LSB bit - in the carry flag, and the next
; sample's first `adc` takes that carry in for free, adding 0 or a quarter LSB
; to the new mix. That is a one-bit error feedback: the dropped half-LSB rounds
; into the following sample instead of vanishing. It is exact in the long run
; and costs no cycles of its own.
;
; For the carry to survive until that `adc`, nothing may touch it in between,
; and a voice's phase advance does. So voice 0 is software-pipelined: its phase
; advance and table lookups run at the *end* of one pass (parked in Y, which
; nothing else uses), and its `adc` opens the next. Y costs a `tay` and a
; `tya`; the `clc` that voice no longer needs pays half of that back.

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
    tay                              ; 6-bit index, interleaved 4 levels wide
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
    ; seed directly rather than to a stored running total. Voice 0's sample was
    ; looked up at the end of the last pass and waits in Y, and the carry is the
    ; previous mix's dropped half-LSB; see "Carrying the rounding error".
    ldx #ACC_INIT_HI
    tya
    adc #ACC_INIT_LO
    sta ACC_LO
    bcc voice_0_no_carry
    inx
voice_0_no_carry:

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

voice_0_start:
    ; Voice 0 for the *next* pass: its gain-scaled sample goes into Y.
    ADVANCE_PHASE 0
    tay
    lda WAVE_BASE, y
    tay
    lda GAIN_DEFAULT, y
    tay

    ; The mix now holds (sum - 384) in quarter-LSB units, so the DAC wants it
    ; shifted down by two - saturated rather than wrapped if the voices
    ; overshot in either direction. The high byte (0-3) supplies the top two
    ; bits of the result from a table, which is cheaper than shifting it in.
    cpx #4
    bcs audio_clip           ; X outside 0-3: the mix over- or undershot
    lda ACC_LO
    lsr a                    ; leaves the dropped half-LSB bit in the carry,
    lsr a                    ; which the next pass's first adc takes in
    ora hi_bits, x
audio_store:
    sta DAC
    jmp audio_loop
audio_clip:
    txa
    asl a                    ; carry = sign: the mix undershot if set
    lda #0x00
    sbc #0x00                ; 0x00 if undershot (carry set), else 0xff
    bra audio_store

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
    ldy #0x80              ; voice 0's waiting sample: silence (gain bias 128)
    jmp audio_loop

; Vector table (must be at $FFFA-$FFFF)
.section .vector_table, "a"
    .word spurious_interrupt ; NMI vector ($FFFA-$FFFB)
    .word _start             ; RESET vector ($FFFC-$FFFD)
    .word spurious_interrupt ; IRQ/BRK vector ($FFFE-$FFFF)
