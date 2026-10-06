# Inter 4.1, static text fonts

Unmodified files from the official [Inter 4.1 release](https://github.com/rsms/inter/releases/tag/v4.1),
`Inter-4.1.zip`, `extras/ttf/Inter-*.ttf` (text optical size, not Inter Display).
Archive SHA-256: `9883fdd4a49d4fb66bd8177ba6625ef9a64aa45899767dde3d36aa425756b11e`.
Retrieved 2026-10-05. The [official features reference](https://rsms.me/inter/) describes
nine weights, true italic and tabular digits. See `LICENSE.txt` (SIL OFL 1.1).

The per-file SHA-256 digests, expected PostScript names and weight/slant metadata
are declared once in `src/lib.rs::FONTS`. Every build validates them against the actual
font bytes. These assets are compiled into the application; no system installation or
network request is needed. Slint's shared renderer collection and the native HUD's
byte-backed CoreText glyph runs are checked separately by the typography QA fixture.

Slint 1.18 has no public OpenType feature-setting property. Numeric labels therefore
use measured fixed digit slots in Inter; they do not claim to enable `tnum`.
