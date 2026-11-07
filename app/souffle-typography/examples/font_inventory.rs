fn main() {
    println!(
        "| File | Family / subfamily | PostScript | Weight | Italic | SHA-256 |\n| --- | --- | --- | ---: | --- | --- |"
    );
    for font in souffle_typography::FONTS {
        souffle_typography::validate_font(font, font.bytes).unwrap();
        let family = souffle_typography::internal_name(font.bytes, 16)
            .or_else(|_| souffle_typography::internal_name(font.bytes, 1))
            .unwrap();
        let subfamily = souffle_typography::internal_name(font.bytes, 17)
            .or_else(|_| souffle_typography::internal_name(font.bytes, 2))
            .unwrap();
        println!(
            "| `{}` | {} / {} | `{}` | {} | {} | `{}` |",
            font.file,
            family,
            subfamily,
            font.postscript,
            font.weight.number(),
            font.slant.italic(),
            font.sha256
        );
    }
    println!("\n| Role | Size (px / pt) | Weight | Letter spacing |\n| --- | ---: | ---: | ---: |");
    for style in souffle_typography::STYLES {
        println!(
            "| `{}` | {} | {} | {} |",
            style.name,
            style.size,
            style.weight.number(),
            style.spacing
        );
    }
}
