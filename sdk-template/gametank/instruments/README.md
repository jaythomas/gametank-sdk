# Wavetable instruments

Define instruments in this folder and they'll be encoded as `.raw` binary files during build time.

From there they can be included in `wavetable.asm`.
For instance, `.incbin "../../instruments/sine.raw"`.
